use num_traits::ToPrimitive;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::Path;

use quick_xml::de::from_str;
use quick_xml::se::to_string;
use serde::{Deserialize, Serialize};
use std::fs;

use strawberry_types::{
    AnnotationPlacement, Dimension, NerveTracerState, Position, PositionTraces, Shape,
};

// XML serialization structures matching the nerve tracer format
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "tracings")]
pub struct Tracings {
    #[serde(rename = "samplespacing")]
    pub samplespacing: SampleSpacing,
    #[serde(rename = "imagesize")]
    pub imagesize: ImageSize,
    #[serde(rename = "path", default)]
    pub paths: Vec<PathElement>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SampleSpacing {
    #[serde(rename = "@x")]
    pub x: f64,
    #[serde(rename = "@y")]
    pub y: f64,
    #[serde(rename = "@z")]
    pub z: f64,
    #[serde(rename = "@units")]
    pub units: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImageSize {
    #[serde(rename = "@width")]
    pub width: u32,
    #[serde(rename = "@height")]
    pub height: u32,
    #[serde(rename = "@depth")]
    pub depth: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PathElement {
    #[serde(rename = "@id")]
    pub id: usize,
    #[serde(rename = "@swctype")]
    pub swctype: u32,
    #[serde(rename = "@color")]
    pub color: String,
    #[serde(rename = "@channel")]
    pub channel: u32,
    #[serde(rename = "@frame")]
    pub frame: u32,
    #[serde(rename = "@spines")]
    pub spines: u32,
    #[serde(rename = "@primary")]
    pub primary: bool,
    #[serde(rename = "@usefitted")]
    pub usefitted: bool,
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@reallength")]
    pub reallength: f64,
    #[serde(rename = "point", default)]
    pub points: Vec<PointElement>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PointElement {
    #[serde(rename = "@x")]
    pub x: u64,
    #[serde(rename = "@y")]
    pub y: u64,
    #[serde(rename = "@z")]
    pub z: u64,
    #[serde(rename = "@xd")]
    pub xd: f64,
    #[serde(rename = "@yd")]
    pub yd: f64,
    #[serde(rename = "@zd")]
    pub zd: f64,
}

impl Default for SampleSpacing {
    /// Create the default values for this type.
    fn default() -> Self {
        Self {
            x: 1.0,
            y: 1.0,
            z: 1.0,
            units: "pixel".to_string(),
        }
    }
}

impl Default for ImageSize {
    /// Create the default values for this type.
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            depth: 1,
        }
    }
}

impl Tracings {
    /// Create tracings from the committed paths of every position of `shape`, numbering the
    /// Z, Y, X position as the point Z and the time point as the path frame.
    ///
    /// # Errors
    /// Returns an error if a position or trace coordinate cannot be represented in the file.
    pub fn from_nerve_tracer_state(
        state: &NerveTracerState,
        shape: Shape,
        samplespacing: Option<SampleSpacing>,
        imagesize: Option<ImageSize>,
    ) -> Result<Self, Box<dyn Error>> {
        let samplespacing = samplespacing.unwrap_or_default();
        let mut imagesize = imagesize.unwrap_or_default();

        let mut paths = Vec::new();
        let mut path_id = 1usize;

        for (position, traces) in state.traces.iter() {
            let slice = shape
                .flatten(Position { t: 0, ..position })
                .ok_or("Trace position exceeds the slice range")?;
            let z = u64::try_from(slice)?;
            let zd = calibrate(z, samplespacing.z, "Z")?;
            imagesize.depth = imagesize.depth.max(u32::try_from(slice)?.saturating_add(1));
            let frame = u32::try_from(position.t)?.saturating_add(1);

            for path in &traces.paths {
                // Flatten all segments in this path into a single list of points
                let all_points = path
                    .iter()
                    .flatten()
                    .map(|&(x, y)| {
                        Ok(PointElement {
                            x,
                            y,
                            z,
                            xd: calibrate(x, samplespacing.x, "X")?,
                            yd: calibrate(y, samplespacing.y, "Y")?,
                            zd,
                        })
                    })
                    .collect::<Result<Vec<_>, Box<dyn Error>>>()?;

                if !all_points.is_empty() {
                    let reallength = calculate_path_length(&all_points);

                    paths.push(PathElement {
                        id: path_id,
                        swctype: 0,
                        color: "None".to_string(),
                        channel: 1,
                        frame,
                        spines: 0,
                        primary: true,
                        usefitted: false,
                        name: format!("Path ({path_id})"),
                        reallength,
                        points: all_points,
                    });
                    path_id = path_id.saturating_add(1);
                }
            }
        }

        Ok(Self {
            samplespacing,
            imagesize,
            paths,
        })
    }

    /// Convert to XML string with proper XML declaration
    /// # Errors
    /// Returns an error if XML conversion or file I/O fails.
    pub fn to_xml_string(&self) -> Result<String, Box<dyn Error>> {
        let xml_body = to_string(self)?;
        let xml = format!("<?xml version='1.0' encoding='utf-8'?>\n{xml_body}");
        Ok(xml)
    }

    /// Parse XML string and create Tracings
    /// # Errors
    /// Returns an error if XML conversion or file I/O fails.
    pub fn from_xml_string(xml: &str) -> Result<Self, Box<dyn Error>> {
        // Remove XML declaration if present
        let xml_content = if xml.starts_with("<?xml") {
            xml.split_once('>').map_or(xml, |(_, body)| body.trim())
        } else {
            xml
        };

        let tracings: Self = from_str(xml_content)?;
        Ok(tracings)
    }

    /// Count the distinct slice and frame pairs trace points lie on.
    #[must_use]
    pub fn count_unique_path_slices(&self) -> usize {
        self.paths
            .iter()
            .flat_map(|path| path.points.iter().map(|point| (point.z, path.frame)))
            .collect::<HashSet<_>>()
            .len()
    }

    /// Replace the traces of each position the imported paths are placed on, reading point Z
    /// and path frames against `shape`.
    ///
    /// # Errors
    /// Returns an error if a Z index cannot address a position.
    pub fn apply_to_nerve_tracer_state(
        &self,
        state: &mut NerveTracerState,
        placement: AnnotationPlacement,
        shape: Shape,
    ) -> Result<(), Box<dyn Error>> {
        let selected = state.traces.selected_position();
        let mut imported: HashMap<Position, PositionTraces> = HashMap::new();
        if placement == AnnotationPlacement::CurrentPosition {
            imported.insert(selected, PositionTraces::default());
        }
        // The frame carries time, so slice numbers only span Z, Y, and X.
        let frame_shape = shape.with_extent(Dimension::T, 1);

        for path_element in &self.paths {
            let Some(first) = path_element.points.first() else {
                continue;
            };
            // A path belongs to the position of its first point.
            let position = match placement {
                AnnotationPlacement::CurrentPosition => selected,
                AnnotationPlacement::SplitByPosition => Position {
                    t: usize::try_from(path_element.frame.saturating_sub(1))?,
                    ..frame_shape.unflatten(usize::try_from(first.z)?)
                },
            };
            // The XML format has no segment boundaries, so each path is one segment.
            let segment = path_element
                .points
                .iter()
                .map(|point| (point.x, point.y))
                .collect();
            imported
                .entry(position)
                .or_default()
                .paths
                .push(vec![segment]);
        }

        for (position, traces) in imported {
            *state.traces.position_mut(position) = traces;
        }
        state.is_enabled = true;
        state.first_point = None;
        Ok(())
    }
}

/// Convert an integer pixel coordinate to a calibrated distance without losing precision.
fn calibrate(value: u64, spacing: f64, axis: &str) -> Result<f64, Box<dyn Error>> {
    let exact = value
        .to_f64()
        .filter(|converted| converted.to_u64() == Some(value))
        .ok_or_else(|| {
            format!("Trace {axis} coordinate cannot be represented exactly for calibration")
        })?;
    Ok(exact * spacing)
}

/// Calculate the real length of a path by summing distances between consecutive points
fn calculate_path_length(points: &[PointElement]) -> f64 {
    points
        .windows(2)
        .filter_map(|pair| {
            let [a, b] = pair else {
                return None;
            };
            Some((b.xd - a.xd).hypot(b.yd - a.yd).hypot(b.zd - a.zd))
        })
        .sum()
}

/// Export the traces of every position of `shape` to an XML file.
/// # Errors
/// Returns an error if XML conversion or file I/O fails.
pub fn export_nervetracer_to_xml_file(
    state: &NerveTracerState,
    shape: Shape,
    file_path: &Path,
    samplespacing: Option<SampleSpacing>,
    imagesize: Option<ImageSize>,
) -> Result<(), Box<dyn Error>> {
    let xml = Tracings::from_nerve_tracer_state(state, shape, samplespacing, imagesize)?
        .to_xml_string()?;
    fs::write(file_path, xml)?;
    Ok(())
}

/// Read and parse a traces XML file.
/// # Errors
/// Returns an error if file reading or XML parsing fails.
pub fn read_tracings_file(file_path: &Path) -> Result<Tracings, Box<dyn Error>> {
    Tracings::from_xml_string(&fs::read_to_string(file_path)?)
}

/// Import XML trace data into existing state, retaining runtime image metadata.
/// # Errors
/// Returns an error if file reading, XML parsing, or position placement fails; leaves state
/// unchanged.
pub fn import_nervetracer_from_xml_file_into(
    file_path: &Path,
    state: &mut NerveTracerState,
    placement: AnnotationPlacement,
    shape: Shape,
) -> Result<(), Box<dyn Error>> {
    let tracings = read_tracings_file(file_path)?;
    let mut imported = state.clone();
    tracings.apply_to_nerve_tracer_state(&mut imported, placement, shape)?;
    *state = imported;
    Ok(())
}
