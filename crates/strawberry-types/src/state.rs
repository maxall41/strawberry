use crate::{BgraColor, Position, RgbColor, Shape};
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::collections::{BTreeMap, HashMap, HashSet, btree_map};
use std::path::PathBuf;
use std::{
    fmt::{self, Display, Formatter},
    num::ParseIntError,
    ops::Deref,
    sync::Arc,
};
use strawberry_processing::segmentation::{FeatureConfig, ForestConfig, InferenceConfig};
use strawberry_processing::threshold::AutoThresholdMethod;
use strawberry_processing::{local_maxima::ThresholdType, morphology::MorphologyOperation};

// Counter-specific types
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountData {
    pub position: (u64, u64), // X, Y
    pub size: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CounterMode {
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CounterTool {
    #[default]
    Place,
    Delete,
    LassoDelete,
    Review,
}

// Individual tool states
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositorState {
    pub is_enabled: bool,
    #[serde(alias = "enabled_layers")]
    pub enabled_channels: Vec<bool>,
    /// Number of channels in the last processed images; runtime-only.
    #[serde(skip)]
    pub channel_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContrastState {
    pub min: f32,
    pub max: f32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GaussianBlurState {
    pub sigma: f32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DifferenceOfGaussiansState {
    pub small_sigma: f32,
    pub large_sigma: f32,
    pub is_enabled: bool,
}

impl Default for DifferenceOfGaussiansState {
    /// Create disabled difference-of-Gaussians settings with default blur scales.
    fn default() -> Self {
        Self {
            small_sigma: 1.0,
            large_sigma: 2.0,
            is_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrayscaleState {
    pub is_enabled: bool,
}

type PathSegment = Vec<(u64, u64)>;
type Path = Vec<PathSegment>;

/// Paths traced on one position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PositionTraces {
    pub paths: Vec<Path>,
    pub current_path_segments: Vec<PathSegment>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NerveTracerState {
    pub is_enabled: bool,
    /// Traces are saved only to dedicated trace files, never to pipeline archives.
    #[serde(skip)]
    pub traces: PerPosition<PositionTraces>,
    #[serde(skip)]
    pub first_point: Option<(u64, u64)>,
    #[serde(alias = "selected_layer", alias = "selected_image_index")]
    pub selected_channel: usize,
    /// Shape of the images being traced, which numbers trace positions in files.
    #[serde(skip)]
    pub shape: Shape,
}

impl NerveTracerState {
    /// Return the committed paths traced on the selected position.
    #[must_use]
    pub fn get_paths(&self) -> &[Path] {
        self.traces
            .get_selected()
            .map_or(&[], |traces| traces.paths.as_slice())
    }

    /// Return the uncommitted segments traced on the selected position.
    #[must_use]
    pub fn get_current_path_segments(&self) -> &[PathSegment] {
        self.traces
            .get_selected()
            .map_or(&[], |traces| traces.current_path_segments.as_slice())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalMaximaState {
    pub min_distance: u32,
    pub threshold: ThresholdType,
    pub exclude_border: bool,
    pub is_enabled: bool,
}

impl Default for LocalMaximaState {
    /// Create disabled local-maxima settings with border exclusion enabled.
    fn default() -> Self {
        Self {
            min_distance: 5,
            threshold: ThresholdType::Absolute(0.0),
            exclude_border: true,
            is_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MorphologyState {
    pub operation: MorphologyOperation,
    pub element_size: u32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectSizeFilterMode {
    #[default]
    RemoveSmallerThan,
    RemoveLargerThan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSizeFilterState {
    pub is_enabled: bool,
    pub mode: ObjectSizeFilterMode,
    pub size: u32,
}

impl Default for ObjectSizeFilterState {
    /// Create disabled object-size filtering settings with a default size cutoff.
    fn default() -> Self {
        Self {
            is_enabled: false,
            mode: ObjectSizeFilterMode::default(),
            size: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollingBallState {
    pub ball_radius: u32,
    pub is_enabled: bool,
}

impl Default for RollingBallState {
    /// Create disabled rolling-ball settings with the default radius.
    fn default() -> Self {
        Self {
            ball_radius: 50,
            is_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SobelState {
    pub threshold_value: f32,
    pub use_threshold: bool,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThresholdState {
    pub threshold_value: u8,
    pub is_enabled: bool,
    pub auto_threshold: bool,
    pub auto_method: AutoThresholdMethod,
}

// TOML table keys are strings, including numeric count-type IDs.
/// Deserialize numeric or string count-type keys, rejecting duplicate IDs.
fn deserialize_count_type_map<'de, D, V>(
    deserializer: D,
) -> std::result::Result<HashMap<u16, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    let entries = HashMap::<serde_value::Value, V>::deserialize(deserializer)?;
    let mut result = HashMap::with_capacity(entries.len());
    for (key, value) in entries {
        let id = match key {
            serde_value::Value::String(text) => text.parse().map_err(D::Error::custom)?,
            numeric => u16::deserialize(numeric).map_err(D::Error::custom)?,
        };
        if result.insert(id, value).is_some() {
            return Err(D::Error::custom("Duplicate count type ID"));
        }
    }
    Ok(result)
}

/// Values kept separately for each X, Y, Z, T position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerPosition<T> {
    // Tool state is cloned for every published snapshot, so the map is shared; each value is
    // shared too, so editing one position copies only that position's value.
    positions: Arc<BTreeMap<Position, Arc<T>>>,
    selected: Position,
}

impl<T> Default for PerPosition<T> {
    /// Create an empty collection with the first position selected.
    fn default() -> Self {
        Self {
            positions: Arc::default(),
            selected: Position::default(),
        }
    }
}

impl<T: Clone + Default> PerPosition<T> {
    /// Return the selected position.
    #[must_use]
    pub const fn get_selected_position(&self) -> Position {
        self.selected
    }

    /// Select the position that subsequent edits apply to.
    pub const fn select(&mut self, position: Position) {
        self.selected = position;
    }

    /// Return the selected position's value, if one has been recorded.
    #[must_use]
    pub fn get_selected(&self) -> Option<&T> {
        self.positions.get(&self.selected).map(Arc::as_ref)
    }

    /// Return the selected position's value, creating an empty one if needed.
    pub fn get_selected_mut(&mut self) -> &mut T {
        self.get_position_mut(self.selected)
    }

    /// Return a position's value, creating an empty one if needed.
    pub fn get_position_mut(&mut self, position: Position) -> &mut T {
        Arc::make_mut(
            Arc::make_mut(&mut self.positions)
                .entry(position)
                .or_default(),
        )
    }

    /// Iterate over recorded positions and their values in frame-major order.
    pub fn iter(&self) -> impl Iterator<Item = (Position, &T)> {
        self.positions
            .iter()
            .map(|(&position, value)| (position, value.as_ref()))
    }

    /// Iterate over the recorded positions on the selected position's mosaic tile, in
    /// frame-major order.
    pub fn iter_selected_tile(&self) -> impl Iterator<Item = (Position, &T)> {
        let selected = self.selected;
        self.iter()
            .filter(move |(position, _)| position.shares_tile_with(selected))
    }

    /// Iterate mutably over every recorded position's value.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        Arc::make_mut(&mut self.positions)
            .values_mut()
            .map(Arc::make_mut)
    }

    /// Replace the values of the given positions, keeping every other position's value.
    pub fn replace_positions(&mut self, values: impl IntoIterator<Item = (Position, T)>) {
        let positions = Arc::make_mut(&mut self.positions);
        for (position, value) in values {
            positions.insert(position, Arc::new(value));
        }
    }

    /// Remove every position's value, keeping the selected position.
    pub fn clear(&mut self) {
        self.positions = Arc::default();
    }
}

/// Counts placed on, or detected in, one position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PositionCounts {
    pub manual: HashMap<u16, Vec<CountData>>,
    pub auto: Vec<CountData>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterState {
    pub is_enabled: bool,
    #[serde(skip)]
    pub counts: PerPosition<PositionCounts>,
    /// Shape of the images being counted, which numbers count positions in files.
    #[serde(skip)]
    pub shape: Shape,
    pub mode: CounterMode,
    pub active_tool: Option<CounterTool>,
    pub active_count_type: Option<u16>,
    #[serde(deserialize_with = "deserialize_count_type_map")]
    pub count_type_visibility: HashMap<u16, bool>,
    pub review_current_index: usize,
    #[serde(deserialize_with = "deserialize_count_type_map")]
    pub count_type_names: HashMap<u16, String>,
}

impl CounterState {
    /// Return the selected position's manual counts of one type.
    #[must_use]
    pub fn get_manual_counts(&self, count_type: u16) -> &[CountData] {
        self.counts
            .get_selected()
            .and_then(|counts| counts.manual.get(&count_type))
            .map_or(&[], Vec::as_slice)
    }

    /// Return the counts detected in the selected position.
    #[must_use]
    pub fn get_auto_counts(&self) -> &[CountData] {
        self.counts
            .get_selected()
            .map_or(&[], |counts| counts.auto.as_slice())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecomposerState {
    pub is_enabled: bool,
    #[serde(alias = "enabled_layers")]
    pub enabled_channels: Vec<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WatershedState {
    pub sigma: f32,
    pub min_distance: u32,
    pub is_enabled: bool,
}

/// A brush radius validated when reading messages or saved pipeline settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BrushRadius(u32);

impl BrushRadius {
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 128;

    /// Return the validated brush radius in pixels.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for BrushRadius {
    /// Create a brush with the minimum supported radius.
    fn default() -> Self {
        Self(1)
    }
}

impl TryFrom<u32> for BrushRadius {
    type Error = String;

    /// Validate a pixel radius against the supported brush range.
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!(
                "Brush radius must be in {}..={}",
                Self::MIN,
                Self::MAX
            ))
        }
    }
}

impl From<BrushRadius> for u32 {
    /// Extract the brush radius as an unsigned integer.
    fn from(radius: BrushRadius) -> Self {
        radius.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClassId(u64);

impl ClassId {
    /// Wrap a numeric segmentation class identifier.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    /// Return the numeric segmentation class identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Display for ClassId {
    /// Format the class identifier as a decimal number.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<ClassId> for String {
    /// Convert the class identifier to its decimal string representation.
    fn from(id: ClassId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<String> for ClassId {
    type Error = ParseIntError;
    /// Parse a decimal string into a class identifier.
    fn try_from(id: String) -> Result<Self, Self::Error> {
        id.parse().map(Self)
    }
}

pub type StrokeId = usize;
/// Index of an input image retained by the segmentation tool.
pub type SampleId = usize;
pub type AnnotationsBySample = HashMap<SampleId, BTreeMap<StrokeId, AnnotationStroke>>;
pub const MAX_CLASSES: usize = 256;

/// Ordered by ID for stable class presentation and model class mappings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SegmentationClasses(BTreeMap<ClassId, SegmentationClass>);

impl SegmentationClasses {
    /// # Errors
    /// Returns an error if class count, names, or name uniqueness are invalid.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (2..=MAX_CLASSES).contains(&self.len()),
            "Define between 2 and {MAX_CLASSES} classes"
        );
        let mut names = HashSet::new();
        for class in self.values() {
            ensure!(!class.name.trim().is_empty(), "Class names cannot be empty");
            ensure!(
                names.insert(class.name.trim()),
                "Duplicate class name: {}",
                class.name
            );
        }
        Ok(())
    }

    /// Reject empty class names and names used by another class.
    fn check_name(&self, id: ClassId, name: &str) -> Result<()> {
        ensure!(!name.is_empty(), "Class names cannot be empty");
        ensure!(
            self.iter()
                .all(|(&other, class)| other == id || class.name != name),
            "Duplicate class name: {name}"
        );
        Ok(())
    }

    /// # Errors
    /// Returns an error for a duplicate ID, invalid name, or class limit violation.
    pub fn insert(&mut self, id: ClassId, mut class: SegmentationClass) -> Result<()> {
        ensure!(!self.contains_key(&id), "Class ID already exists");
        ensure!(self.len() < MAX_CLASSES, "Maximum of {MAX_CLASSES} classes");
        class.name = class.name.trim().into();
        self.check_name(id, &class.name)?;
        self.0.insert(id, class);
        Ok(())
    }

    /// # Errors
    /// Returns an error if the class is unknown or the name is empty or duplicated.
    pub fn rename(&mut self, id: ClassId, name: &str) -> Result<()> {
        let name = name.trim();
        self.check_name(id, name)?;
        name.clone_into(
            &mut self
                .0
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("Unknown class"))?
                .name,
        );
        Ok(())
    }

    /// # Errors
    /// Returns an error if the class is unknown or fewer than two classes would remain.
    pub fn remove(&mut self, id: ClassId) -> Result<()> {
        ensure!(self.len() > 2, "Keep at least two classes");
        ensure!(self.0.remove(&id).is_some(), "Unknown class");
        Ok(())
    }

    /// # Errors
    /// Returns an error if the class is unknown.
    pub fn set_color(&mut self, id: ClassId, color: RgbColor) -> Result<()> {
        self.0
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Unknown class"))?
            .color = color;
        Ok(())
    }

    /// Return mutable annotations for the specified class, if it exists.
    pub fn annotations_mut(&mut self, id: ClassId) -> Option<&mut AnnotationsBySample> {
        self.0.get_mut(&id).map(|class| &mut class.annotations)
    }

    /// Clone the class definitions with all sample annotations removed.
    #[must_use]
    pub fn without_annotations(&self) -> Self {
        Self(
            self.iter()
                .map(|(&id, class)| {
                    (
                        id,
                        SegmentationClass {
                            name: class.name.clone(),
                            color: class.color,
                            annotations: HashMap::new(),
                        },
                    )
                })
                .collect(),
        )
    }

    /// # Errors
    /// Returns an error if no unused name can be generated.
    pub fn unused_name(&self) -> Result<String> {
        let start = self.len().saturating_add(1);
        let end = start.saturating_add(self.len());
        (start..=end)
            .map(|number| format!("Class {number}"))
            .find(|name| self.values().all(|class| class.name != *name))
            .ok_or_else(|| anyhow::anyhow!("No unused class name"))
    }
}

impl TryFrom<BTreeMap<ClassId, SegmentationClass>> for SegmentationClasses {
    type Error = anyhow::Error;

    /// Trim class names and validate the supplied class collection.
    fn try_from(mut classes: BTreeMap<ClassId, SegmentationClass>) -> Result<Self> {
        for class in classes.values_mut() {
            class.name = class.name.trim().into();
        }
        let classes = Self(classes);
        classes.validate()?;
        Ok(classes)
    }
}

impl<'de> Deserialize<'de> for SegmentationClasses {
    /// Deserialize and validate an ordered segmentation class collection.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

impl Deref for SegmentationClasses {
    type Target = BTreeMap<ClassId, SegmentationClass>;
    /// Expose the ordered class map for read-only access.
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for SegmentationClasses {
    type Item = (ClassId, SegmentationClass);
    type IntoIter = btree_map::IntoIter<ClassId, SegmentationClass>;
    /// Consume the collection and iterate over classes in identifier order.
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a SegmentationClasses {
    type Item = (&'a ClassId, &'a SegmentationClass);
    type IntoIter = btree_map::Iter<'a, ClassId, SegmentationClass>;
    /// Iterate over borrowed classes in identifier order.
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentationClass {
    pub name: String,
    pub color: RgbColor,
    #[serde(skip)]
    pub annotations: AnnotationsBySample,
}
impl SegmentationClass {
    /// Return the class color in BGRA layout with the requested opacity.
    #[must_use]
    pub const fn bgra(&self, alpha: u8) -> BgraColor {
        self.color.with_alpha(alpha).to_bgra()
    }

    /// Count this class’s annotation strokes across all samples.
    pub fn annotation_count(&self) -> usize {
        self.annotations.values().map(BTreeMap::len).sum()
    }
}
pub const CLASS_PALETTE: [RgbColor; 8] = [
    RgbColor::new(230, 70, 80),
    RgbColor::new(40, 220, 100),
    RgbColor::new(75, 145, 245),
    RgbColor::new(245, 190, 55),
    RgbColor::new(185, 110, 235),
    RgbColor::new(45, 205, 210),
    RgbColor::new(245, 135, 65),
    RgbColor::new(230, 120, 175),
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotationKind {
    Paint(ClassId),
    Eraser,
}
impl Default for AnnotationKind {
    /// Select the foreground class for painting by default.
    fn default() -> Self {
        Self::Paint(ClassId::new(1))
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentationOutputMode {
    #[default]
    ClassMask,
    ClassColors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationStroke {
    pub visible: bool,
    pub radius: BrushRadius,
    pub points: Arc<Vec<(u32, u32)>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetImportMode {
    Replace,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetOperation {
    Saving,
    Loading,
    Merging,
}

impl DatasetOperation {
    /// Return whether this operation loads or merges a dataset.
    #[must_use]
    pub const fn is_importing(self) -> bool {
        matches!(self, Self::Loading | Self::Merging)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Processing, visibility, and runtime job flags vary independently"
)]
pub struct AutomaticSegmentationState {
    pub is_enabled: bool,
    pub training: bool,
    pub show_overlay: bool,
    pub overlay_opacity: u8,
    pub use_threshold: bool,
    pub threshold_value: f32,
    pub classes: SegmentationClasses,
    pub next_class_id: u64,
    pub hidden_classes: Vec<ClassId>,
    pub preview_selected: bool,
    pub output_mode: SegmentationOutputMode,
    pub output_class: ClassId,
    pub active_class: ClassId,
    pub brush: AnnotationKind,
    pub brush_radius: BrushRadius,
    #[serde(alias = "selected_layer")]
    pub selected_channel: usize,
    #[serde(skip)]
    pub channel_count: usize,
    #[serde(skip)]
    pub selected_position: Position,
    pub features: FeatureConfig,
    pub forest: ForestConfig,
    pub inference: InferenceConfig,
    #[serde(skip)]
    pub model_path: Option<PathBuf>,
    #[serde(skip)]
    pub dataset_samples: usize,
    #[serde(skip)]
    pub dataset_path: Option<PathBuf>,
    #[serde(skip)]
    pub dataset_dirty: bool,
    #[serde(skip)]
    pub dataset_operation: Option<DatasetOperation>,
    #[serde(skip)]
    pub erasers: AnnotationsBySample,
    #[serde(skip)]
    pub current_sample: Option<SampleId>,
    #[serde(skip)]
    pub selected_stroke: Option<StrokeId>,
    #[serde(skip)]
    pub busy: bool,
    #[serde(skip)]
    pub model_ready: bool,
    #[serde(skip)]
    pub oob_error: Option<f64>,
    #[serde(skip)]
    pub model_revision: u64,
}

impl AutomaticSegmentationState {
    /// Count paint and eraser strokes across all samples.
    pub fn annotation_count(&self) -> usize {
        self.classes
            .values()
            .map(SegmentationClass::annotation_count)
            .chain(self.erasers.values().map(BTreeMap::len))
            .sum()
    }

    /// Iterate over a sample’s paint and eraser strokes with their identities.
    pub fn sample_annotations(
        &self,
        sample: SampleId,
    ) -> impl Iterator<Item = (AnnotationKind, StrokeId, &AnnotationStroke)> {
        self.classes
            .iter()
            .flat_map(move |(&id, class)| {
                class
                    .annotations
                    .get(&sample)
                    .into_iter()
                    .flat_map(move |strokes| {
                        strokes.iter().map(move |(&stroke, annotation)| {
                            (AnnotationKind::Paint(id), stroke, annotation)
                        })
                    })
            })
            .chain(self.erasers.get(&sample).into_iter().flat_map(|strokes| {
                strokes
                    .iter()
                    .map(|(&stroke, annotation)| (AnnotationKind::Eraser, stroke, annotation))
            }))
    }

    /// Iterate over annotations belonging to the current sample.
    pub fn current_annotations(
        &self,
    ) -> impl Iterator<Item = (AnnotationKind, StrokeId, &AnnotationStroke)> {
        self.current_sample
            .into_iter()
            .flat_map(|sample| self.sample_annotations(sample))
    }

    /// Collect a sample’s annotations in stroke identifier order.
    #[must_use]
    pub fn ordered_annotations(
        &self,
        sample: SampleId,
    ) -> BTreeMap<StrokeId, (AnnotationKind, &AnnotationStroke)> {
        self.sample_annotations(sample)
            .map(|(kind, id, stroke)| (id, (kind, stroke)))
            .collect()
    }

    /// Find a sample’s stroke among eraser and class annotations.
    #[must_use]
    pub fn stroke(&self, sample: SampleId, id: StrokeId) -> Option<&AnnotationStroke> {
        self.erasers
            .get(&sample)
            .and_then(|strokes| strokes.get(&id))
            .or_else(|| {
                self.classes
                    .values()
                    .find_map(|class| class.annotations.get(&sample)?.get(&id))
            })
    }

    /// Return mutable access to a sample’s eraser or class stroke.
    pub fn stroke_mut(&mut self, sample: SampleId, id: StrokeId) -> Option<&mut AnnotationStroke> {
        self.erasers
            .get_mut(&sample)
            .and_then(|strokes| strokes.get_mut(&id))
            .or_else(|| {
                self.classes
                    .0
                    .values_mut()
                    .find_map(|class| class.annotations.get_mut(&sample)?.get_mut(&id))
            })
    }

    /// Remove the stroke identifier from every annotation kind for a sample.
    pub fn remove_stroke(&mut self, sample: SampleId, id: StrokeId) {
        if let Some(strokes) = self.erasers.get_mut(&sample) {
            strokes.remove(&id);
        }
        for class in self.classes.0.values_mut() {
            if let Some(strokes) = class.annotations.get_mut(&sample) {
                strokes.remove(&id);
            }
        }
    }

    /// Remove all paint and eraser annotations for a sample.
    pub fn clear_sample_annotations(&mut self, sample: SampleId) {
        self.erasers.remove(&sample);
        for class in self.classes.0.values_mut() {
            class.annotations.remove(&sample);
        }
    }
}

impl Default for AutomaticSegmentationState {
    /// Create default segmentation settings with background and foreground classes.
    fn default() -> Self {
        Self {
            is_enabled: false,
            training: true,
            show_overlay: false,
            overlay_opacity: 30,
            use_threshold: false,
            threshold_value: 0.5,
            classes: SegmentationClasses(BTreeMap::from([
                (
                    ClassId::new(0),
                    SegmentationClass {
                        name: "Background".into(),
                        color: CLASS_PALETTE[0],
                        annotations: HashMap::new(),
                    },
                ),
                (
                    ClassId::new(1),
                    SegmentationClass {
                        name: "Foreground".into(),
                        color: CLASS_PALETTE[1],
                        annotations: HashMap::new(),
                    },
                ),
            ])),
            next_class_id: 2,
            hidden_classes: vec![],
            preview_selected: false,
            output_mode: SegmentationOutputMode::ClassMask,
            output_class: ClassId::new(1),
            active_class: ClassId::new(1),
            brush: AnnotationKind::default(),
            brush_radius: BrushRadius::default(),
            selected_channel: 0,
            channel_count: 0,
            selected_position: Position::default(),
            features: FeatureConfig::default(),
            forest: ForestConfig::default(),
            inference: InferenceConfig::default(),
            model_path: None,
            dataset_samples: 0,
            dataset_path: None,
            dataset_dirty: false,
            dataset_operation: None,
            erasers: HashMap::new(),
            current_sample: None,
            selected_stroke: None,
            busy: false,
            model_ready: false,
            oob_error: None,
            model_revision: 0,
        }
    }
}

/// Aggregated tool state enum
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolState {
    AutomaticSegmentationState(crate::AutomaticSegmentationState),
    CompositorState(CompositorState),
    ContrastState(ContrastState),
    DifferenceOfGaussiansState(DifferenceOfGaussiansState),
    GaussianBlurState(GaussianBlurState),
    GrayscaleState(GrayscaleState),
    LocalMaximaState(LocalMaximaState),
    MorphologyState(MorphologyState),
    ObjectSizeFilterState(ObjectSizeFilterState),
    RollingBallState(RollingBallState),
    SobelState(SobelState),
    ThresholdState(ThresholdState),
    CounterState(CounterState),
    DecomposerState(DecomposerState),
    WatershedState(WatershedState),
    NerveTracerState(NerveTracerState),
}

/// Object-safe loading into detached candidate instances.
pub trait PersistedState {
    /// Load serialized fields into this state.
    ///
    /// # Errors
    /// Returns an error if the fields cannot be deserialized into this state type.
    fn load_persisted_fields(&mut self, fields: serde_value::Value) -> anyhow::Result<()>;
}
impl<T: Clone + serde::de::DeserializeOwned> PersistedState for T {
    /// Deserialize fields into a clone and replace this state only on success.
    fn load_persisted_fields(&mut self, fields: serde_value::Value) -> anyhow::Result<()> {
        let mut merged = self.clone();
        T::deserialize_in_place(fields, &mut merged)?;
        *self = merged;
        Ok(())
    }
}

impl ToolState {
    /// Check that tool parameters are within their supported ranges.
    ///
    /// # Errors
    /// Returns an error if a checked parameter is non-finite or outside its valid range.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::ContrastState(s) => ensure!(
                s.min.is_finite() && s.max.is_finite() && s.min <= s.max,
                "Contrast minimum must not exceed maximum"
            ),
            Self::GaussianBlurState(s) => ensure!(
                s.sigma.is_finite() && s.sigma > 0.,
                "Sigma must be positive"
            ),
            Self::DifferenceOfGaussiansState(s) => ensure!(
                s.small_sigma.is_finite()
                    && s.small_sigma > 0.
                    && s.large_sigma.is_finite()
                    && s.large_sigma > s.small_sigma,
                "Invalid Gaussian sigmas"
            ),
            Self::LocalMaximaState(s) => {
                if let ThresholdType::Absolute(value) | ThresholdType::Relative(value) = s.threshold
                {
                    ensure!(
                        value.is_finite() && (0.0..=1.0).contains(&value),
                        "Invalid maxima threshold"
                    );
                }
            }
            Self::MorphologyState(s) => {
                ensure!(s.element_size > 0, "Element size must be positive");
            }
            Self::RollingBallState(s) => ensure!(s.ball_radius > 0, "Ball radius must be positive"),
            Self::SobelState(s) => ensure!(
                s.threshold_value.is_finite() && (0.0..=1.0).contains(&s.threshold_value),
                "Invalid Sobel threshold"
            ),
            Self::WatershedState(s) => ensure!(
                s.sigma.is_finite() && s.sigma > 0.,
                "Sigma must be positive"
            ),
            _ => {}
        }
        Ok(())
    }
}
