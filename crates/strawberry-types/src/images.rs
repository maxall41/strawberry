use anyhow::{Result, anyhow, bail, ensure};
use image::RgbaImage;
use ndarray::{Array5, Axis, Slice, indices};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

/// A hyperstack dimension. X and Y number the tiles of a mosaic; pixels stay inside each image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dimension {
    /// Tile column, for mosaics that are stitched together.
    X,
    /// Tile row, for mosaics that are stitched together.
    Y,
    /// Depth slice.
    Z,
    /// Channel; the viewer shows one at a time, and the Compositor tool merges them.
    C,
    /// Time point.
    T,
}

impl Dimension {
    /// Every dimension, in the order they are presented to the user.
    pub const ALL: [Self; 5] = [Self::C, Self::Z, Self::T, Self::X, Self::Y];

    /// Return the letter that names this dimension.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::C => "C",
            Self::T => "T",
        }
    }

    /// Return the name this dimension is shown to the user by, such as "Channel".
    #[must_use]
    pub const fn get_name(self) -> &'static str {
        match self {
            Self::X => "Tile column",
            Self::Y => "Tile row",
            Self::Z => "Depth",
            Self::C => "Channel",
            Self::T => "Time",
        }
    }

    /// Return the storage axis of this dimension; planes are stored in T, Z, C, Y, X order.
    const fn axis(self) -> Axis {
        Axis(match self {
            Self::T => 0,
            Self::Z => 1,
            Self::C => 2,
            Self::Y => 3,
            Self::X => 4,
        })
    }
}

/// The number of planes along each hyperstack dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shape {
    pub t: usize,
    pub z: usize,
    pub c: usize,
    pub y: usize,
    pub x: usize,
}

impl Shape {
    /// The shape of a hyperstack holding a single plane.
    pub const SINGLE_PLANE: Self = Self {
        t: 1,
        z: 1,
        c: 1,
        y: 1,
        x: 1,
    };

    /// Return the number of planes along a dimension.
    #[must_use]
    pub const fn extent(self, dimension: Dimension) -> usize {
        match dimension {
            Dimension::X => self.x,
            Dimension::Y => self.y,
            Dimension::Z => self.z,
            Dimension::C => self.c,
            Dimension::T => self.t,
        }
    }

    /// Return a copy with one dimension's extent replaced.
    #[must_use]
    pub const fn with_extent(mut self, dimension: Dimension, extent: usize) -> Self {
        match dimension {
            Dimension::X => self.x = extent,
            Dimension::Y => self.y = extent,
            Dimension::Z => self.z = extent,
            Dimension::C => self.c = extent,
            Dimension::T => self.t = extent,
        }
        self
    }

    /// Convert storage-order extents into a shape.
    const fn from_dims(dims: (usize, usize, usize, usize, usize)) -> Self {
        Self {
            t: dims.0,
            z: dims.1,
            c: dims.2,
            y: dims.3,
            x: dims.4,
        }
    }

    /// Return the extents in storage order, as `ndarray` expects them.
    const fn dims(self) -> (usize, usize, usize, usize, usize) {
        (self.t, self.z, self.c, self.y, self.x)
    }

    /// Return the number of X, Y, Z, T positions; the channels at a position count once.
    #[must_use]
    pub const fn position_count(self) -> usize {
        self.t
            .saturating_mul(self.z)
            .saturating_mul(self.y)
            .saturating_mul(self.x)
    }

    /// Return whether the shape holds more than one image, counting each channel separately.
    #[must_use]
    pub const fn holds_several_images(self) -> bool {
        self.position_count() > 1 || self.c > 1
    }

    /// Return whether coordinates address a plane of this shape.
    #[must_use]
    pub const fn contains(self, coordinates: Coordinates) -> bool {
        coordinates.c < self.c && self.contains_position(coordinates.position())
    }

    /// Return whether a position lies inside this shape, whatever its channel count.
    #[must_use]
    pub const fn contains_position(self, position: Position) -> bool {
        position.t < self.t && position.z < self.z && position.y < self.y && position.x < self.x
    }

    /// Check that every position holding annotations lies inside this shape.
    ///
    /// # Errors
    /// Returns an error counting the positions outside it, naming their `annotations`.
    pub fn ensure_contains_positions(
        self,
        positions: impl IntoIterator<Item = Position>,
        annotations: &str,
    ) -> Result<()> {
        let outside = positions
            .into_iter()
            .filter(|&position| !self.contains_position(position))
            .count();
        ensure!(
            outside == 0,
            "{annotations} on {outside} image(s) lie outside the loaded images"
        );
        Ok(())
    }

    /// Return the number of planes, or `None` if it exceeds the address space.
    #[must_use]
    pub fn get_plane_count(self) -> Option<usize> {
        Dimension::ALL
            .into_iter()
            .try_fold(1usize, |count, dimension| {
                count.checked_mul(self.extent(dimension))
            })
    }

    /// Iterate over the coordinates of every plane in storage order.
    pub fn coordinates(self) -> impl Iterator<Item = Coordinates> {
        indices(self.dims()).into_iter().map(Coordinates::from)
    }

    /// Iterate over every position in storage order.
    pub fn positions(self) -> impl Iterator<Item = Position> {
        self.with_extent(Dimension::C, self.c.min(1))
            .coordinates()
            .map(Coordinates::position)
    }

    /// Return the shape of a complete grid of plane coordinates.
    ///
    /// # Errors
    /// Returns an error naming the first repeated or missing plane, or if no coordinates are given.
    pub fn from_coordinates(coordinates: &[Coordinates]) -> Result<Self> {
        ensure!(!coordinates.is_empty(), "No input images provided");
        let shape = coordinates.iter().fold(Self::default(), |shape, plane| {
            Dimension::ALL.into_iter().fold(shape, |shape, dimension| {
                let extent = plane.get(dimension).saturating_add(1);
                shape.with_extent(dimension, shape.extent(dimension).max(extent))
            })
        });
        // Enumerating the grid multiplies its extents, which malformed placements can overflow.
        let plane_count = shape
            .get_plane_count()
            .ok_or_else(|| anyhow!("The image placements span too many images ({shape})"))?;
        let mut placed = HashSet::with_capacity(coordinates.len());
        for plane in coordinates {
            ensure!(
                placed.insert(*plane),
                "Two images are placed at {}",
                plane.describe(shape)
            );
        }
        if plane_count != placed.len()
            && let Some(missing) = shape.coordinates().find(|plane| !placed.contains(plane))
        {
            bail!("No image for {}", missing.describe(shape));
        }
        Ok(shape)
    }

    /// Return a position's slice index in annotation files, counting depth slices through time
    /// points, or `None` if the shape does not contain the position.
    ///
    /// Annotation files describe a single image stack, so the position's tile is not numbered.
    #[must_use]
    pub fn get_slice_index(self, position: Position) -> Option<usize> {
        if !self.contains_position(position) {
            return None;
        }
        position.t.checked_mul(self.z)?.checked_add(position.z)
    }

    /// Return the position an annotation file's slice index refers to on the tile of `tile`.
    ///
    /// Without several time points, indices past the last slice continue along Z.
    #[must_use]
    pub fn get_slice_position(self, index: usize, tile: Position) -> Position {
        let z_extent = self.z.max(1);
        let (z, t) = if self.t > 1 {
            (
                index.checked_rem(z_extent).unwrap_or_default(),
                index.checked_div(z_extent).unwrap_or_default(),
            )
        } else {
            (index, 0)
        };
        Position { t, z, ..tile }
    }
}

impl Display for Shape {
    /// Describe the extents of the dimensions that hold several planes, such as "C 2 × Z 30".
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let spanned: Vec<String> = Dimension::ALL
            .into_iter()
            .filter(|&dimension| self.extent(dimension) > 1)
            .map(|dimension| format!("{} {}", dimension.label(), self.extent(dimension)))
            .collect();
        if spanned.is_empty() {
            write!(f, "1 image")
        } else {
            write!(f, "{}", spanned.join(" × "))
        }
    }
}

/// The location of one plane in a hyperstack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Coordinates {
    pub t: usize,
    pub z: usize,
    pub c: usize,
    pub y: usize,
    pub x: usize,
}

impl Coordinates {
    /// Return the index along a dimension.
    #[must_use]
    pub const fn get(self, dimension: Dimension) -> usize {
        match dimension {
            Dimension::X => self.x,
            Dimension::Y => self.y,
            Dimension::Z => self.z,
            Dimension::C => self.c,
            Dimension::T => self.t,
        }
    }

    /// Change the index along a dimension.
    pub const fn set(&mut self, dimension: Dimension, index: usize) {
        match dimension {
            Dimension::X => self.x = index,
            Dimension::Y => self.y = index,
            Dimension::Z => self.z = index,
            Dimension::C => self.c = index,
            Dimension::T => self.t = index,
        }
    }

    /// Return the position these coordinates lie at, ignoring the channel.
    #[must_use]
    pub const fn position(self) -> Position {
        Position {
            t: self.t,
            z: self.z,
            y: self.y,
            x: self.x,
        }
    }

    /// Split into the coordinates outside `dimensions` and the offset along them.
    fn split(self, dimensions: &[Dimension]) -> (Self, Self) {
        let mut origin = self;
        let mut offset = Self::default();
        for &dimension in dimensions {
            origin.set(dimension, 0);
            offset.set(dimension, self.get(dimension));
        }
        (origin, offset)
    }

    /// Return the one-based index along each dimension that `shape` spans, in presentation order.
    pub fn spanned(self, shape: Shape) -> impl Iterator<Item = (Dimension, usize)> {
        Dimension::ALL
            .into_iter()
            .filter(move |&dimension| shape.extent(dimension) > 1)
            .map(move |dimension| (dimension, self.get(dimension).saturating_add(1)))
    }

    /// Describe the plane for messages, such as "C2 Z3 T1".
    #[must_use]
    pub fn describe(self, shape: Shape) -> String {
        let mut parts: Vec<String> = self
            .spanned(shape)
            .map(|(dimension, number)| format!("{}{number}", dimension.label()))
            .collect();
        if parts.is_empty() {
            parts = [Dimension::C, Dimension::Z, Dimension::T]
                .map(|dimension| {
                    format!(
                        "{}{}",
                        dimension.label(),
                        self.get(dimension).saturating_add(1)
                    )
                })
                .into();
        }
        parts.join(" ")
    }
}

impl From<(usize, usize, usize, usize, usize)> for Coordinates {
    /// Convert storage-order indices into coordinates.
    fn from(index: (usize, usize, usize, usize, usize)) -> Self {
        Self {
            t: index.0,
            z: index.1,
            c: index.2,
            y: index.3,
            x: index.4,
        }
    }
}

/// An X, Y, Z, T location, shared by every channel there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Position {
    pub t: usize,
    pub z: usize,
    pub y: usize,
    pub x: usize,
}

impl Position {
    /// Return whether two positions lie on the same mosaic tile.
    #[must_use]
    pub const fn shares_tile_with(self, other: Self) -> bool {
        self.y == other.y && self.x == other.x
    }

    /// Return the coordinates of one channel at this position.
    #[must_use]
    pub const fn with_channel(self, c: usize) -> Coordinates {
        Coordinates {
            t: self.t,
            z: self.z,
            c,
            y: self.y,
            x: self.x,
        }
    }
}

/// How several loaded images are placed in a hyperstack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageArrangement {
    /// Place the images as the channels of one image.
    Layers,
    /// Place the images along Z.
    Stack,
    /// Place each image at its own coordinates.
    Hyperstack(Vec<Coordinates>),
}

impl ImageArrangement {
    /// Return the shape `count` images form in this arrangement.
    ///
    /// # Errors
    /// Returns an error if the coordinates do not cover `count` images or form a complete grid.
    pub fn get_shape(&self, count: usize) -> Result<Shape> {
        Shape::from_coordinates(&self.coordinates(count)?)
    }

    /// Return the coordinates of each of `count` images.
    fn coordinates(&self, count: usize) -> Result<Vec<Coordinates>> {
        let along = |dimension| {
            (0..count)
                .map(|index| {
                    let mut coordinates = Coordinates::default();
                    coordinates.set(dimension, index);
                    coordinates
                })
                .collect()
        };
        Ok(match self {
            Self::Layers => along(Dimension::C),
            Self::Stack => along(Dimension::Z),
            Self::Hyperstack(coordinates) => {
                ensure!(
                    coordinates.len() == count,
                    "Expected coordinates for {count} images, received {}",
                    coordinates.len()
                );
                coordinates.clone()
            }
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ImageCollection {
    /// Planes in T, Z, C, Y, X order.
    planes: Array5<Arc<RgbaImage>>,
}

impl ImageCollection {
    /// Place loaded images in a hyperstack.
    ///
    /// # Errors
    /// Returns an error if no images are given, their coordinates do not form a complete grid,
    /// or the channels at one position differ in size.
    pub fn arrange(images: Vec<Arc<RgbaImage>>, arrangement: &ImageArrangement) -> Result<Self> {
        let coordinates = arrangement.coordinates(images.len())?;
        let shape = Shape::from_coordinates(&coordinates)?;
        let mut placed: HashMap<_, _> = coordinates.into_iter().zip(images).collect();
        let collection = Self::from_fn(shape, |coordinates| {
            placed
                .remove(&coordinates)
                .ok_or_else(|| anyhow!("No image for {}", coordinates.describe(shape)))
        })?;
        for position in collection.positions() {
            let mut channels = collection.channels_at(position);
            if let Some(first) = channels.next() {
                ensure!(
                    channels.all(|channel| channel.dimensions() == first.dimensions()),
                    "All layers of an image must have the same dimensions"
                );
            }
        }
        Ok(collection)
    }

    /// Build a collection of `shape` from the plane at each coordinate.
    fn from_fn(
        shape: Shape,
        plane: impl FnMut(Coordinates) -> Result<Arc<RgbaImage>>,
    ) -> Result<Self> {
        let planes = shape.coordinates().map(plane).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            planes: Array5::from_shape_vec(shape.dims(), planes)?,
        })
    }

    /// Return the number of planes along each dimension.
    #[must_use]
    pub fn shape(&self) -> Shape {
        Shape::from_dims(self.planes.dim())
    }

    /// Return whether the collection holds no planes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }

    /// Iterate over every position in storage order.
    pub fn positions(&self) -> impl Iterator<Item = Position> + use<> {
        self.shape().positions()
    }

    /// Return the plane at the given coordinates.
    #[must_use]
    pub fn plane(&self, coordinates: Coordinates) -> Option<&Arc<RgbaImage>> {
        self.planes.get((
            coordinates.t,
            coordinates.z,
            coordinates.c,
            coordinates.y,
            coordinates.x,
        ))
    }

    /// Iterate over the channels at one position in channel order.
    pub fn channels_at(&self, position: Position) -> impl Iterator<Item = &Arc<RgbaImage>> {
        (0..self.shape().c).filter_map(move |c| self.plane(position.with_channel(c)))
    }

    /// Iterate mutably over every plane.
    pub fn planes_mut(&mut self) -> impl ExactSizeIterator<Item = &mut Arc<RgbaImage>> {
        self.planes.iter_mut()
    }

    /// Return the part of the channel axis holding `channel`, empty if the channel is missing.
    fn get_channel_slice(&self, channel: usize) -> Slice {
        let end = channel
            .saturating_add(1)
            .min(self.planes.len_of(Dimension::C.axis()));
        Slice::from(channel.min(end)..end)
    }

    /// Iterate over one channel's plane at every position; empty if the channel is missing.
    pub fn channel(&self, channel: usize) -> impl Iterator<Item = &Arc<RgbaImage>> {
        self.planes
            .slice_axis(Dimension::C.axis(), self.get_channel_slice(channel))
            .into_iter()
    }

    /// Iterate mutably over one channel's plane at every position; empty if the channel is missing.
    pub fn channel_mut(&mut self, channel: usize) -> impl Iterator<Item = &mut Arc<RgbaImage>> {
        let slice = self.get_channel_slice(channel);
        self.planes
            .slice_axis_mut(Dimension::C.axis(), slice)
            .into_iter()
    }

    /// Return the coordinates where each group along `dimensions` starts, in storage order.
    fn group_origins(shape: Shape, dimensions: &[Dimension]) -> impl Iterator<Item = Coordinates> {
        dimensions
            .iter()
            .fold(shape, |shape, &dimension| {
                shape.with_extent(dimension, shape.extent(dimension).min(1))
            })
            .coordinates()
    }

    /// Split into one sub-collection spanning `dimensions` for every combination of the other
    /// coordinates in storage order.
    #[must_use]
    pub fn groups(&self, dimensions: &[Dimension]) -> Vec<Self> {
        Self::group_origins(self.shape(), dimensions)
            .map(|origin| {
                let planes = self.planes.slice_each_axis(|axis| {
                    let Some(dimension) = Dimension::ALL
                        .into_iter()
                        .find(|dimension| dimension.axis() == axis.axis)
                    else {
                        return Slice::from(..);
                    };
                    if dimensions.contains(&dimension) {
                        Slice::from(..)
                    } else {
                        let start = origin.get(dimension);
                        Slice::from(start..start.saturating_add(1))
                    }
                });
                Self {
                    planes: planes.to_owned(),
                }
            })
            .collect()
    }

    /// Replace the groups returned by `groups`, which may change their extents along
    /// `dimensions` as long as every group keeps the same shape.
    ///
    /// # Errors
    /// Returns an error if the group count is wrong, groups differ in shape, a group spans other
    /// dimensions, or a group is empty.
    pub fn replace_groups(&mut self, dimensions: &[Dimension], groups: Vec<Self>) -> Result<()> {
        let shape = self.shape();
        let origins: Vec<_> = Self::group_origins(shape, dimensions).collect();
        ensure!(
            groups.len() == origins.len(),
            "Expected {} image groups, received {}",
            origins.len(),
            groups.len()
        );
        let Some(group_shape) = groups.first().map(Self::shape) else {
            return Ok(());
        };
        ensure!(
            groups.iter().all(|group| group.shape() == group_shape),
            "Every image group must have the same shape"
        );
        for dimension in Dimension::ALL {
            let extent = group_shape.extent(dimension);
            if dimensions.contains(&dimension) {
                ensure!(extent > 0, "Every image group must keep at least one image");
            } else {
                ensure!(
                    extent == 1,
                    "Image groups may only span the dimensions they were grouped by"
                );
            }
        }
        let new_shape = dimensions.iter().fold(shape, |shape, &dimension| {
            shape.with_extent(dimension, group_shape.extent(dimension))
        });
        let groups: HashMap<_, _> = origins.into_iter().zip(groups).collect();
        *self = Self::from_fn(new_shape, |coordinates| {
            let (origin, offset) = coordinates.split(dimensions);
            groups
                .get(&origin)
                .and_then(|group| group.plane(offset))
                .cloned()
                .ok_or_else(|| anyhow!("No image for {}", coordinates.describe(new_shape)))
        })?;
        Ok(())
    }

    /// Return the planes along one dimension for every combination of the other coordinates.
    #[must_use]
    pub fn lanes(&self, dimension: Dimension) -> Vec<Vec<Arc<RgbaImage>>> {
        self.groups(&[dimension])
            .into_iter()
            .map(|group| group.planes.into_iter().collect())
            .collect()
    }

    /// Replace the lanes returned by `lanes`; every lane must keep the same, non-zero length.
    ///
    /// # Errors
    /// Returns an error if the lane count is wrong or lanes differ in length.
    pub fn replace_lanes(
        &mut self,
        dimension: Dimension,
        lanes: Vec<Vec<Arc<RgbaImage>>>,
    ) -> Result<()> {
        let groups = lanes
            .into_iter()
            .map(|lane| {
                let shape = Shape::SINGLE_PLANE.with_extent(dimension, lane.len());
                Ok(Self {
                    planes: Array5::from_shape_vec(shape.dims(), lane)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.replace_groups(&[dimension], groups)
    }
}
