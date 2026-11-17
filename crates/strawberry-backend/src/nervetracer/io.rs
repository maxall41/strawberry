use num_traits::ToPrimitive;

use std::path::Path;

use quick_xml::de::from_str;
use quick_xml::se::to_string;
use serde::{Deserialize, Serialize};
use std::fs;

use strawberry_types::NerveTracerState;

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
    /// Create a new Tracings structure from `NerveTracerState`.
    ///
    /// # Errors
    /// Returns an error if a trace coordinate cannot be represented exactly for XML calibration.
    pub fn from_nerve_tracer_state(
        state: &NerveTracerState,
        samplespacing: Option<SampleSpacing>,
        imagesize: Option<ImageSize>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let samplespacing = samplespacing.unwrap_or_default();
        let imagesize = imagesize.unwrap_or_default();

        let mut paths = Vec::new();
        let mut path_id = 1usize;

        // Convert committed paths
        for path in &state.paths {
            let mut all_points = Vec::new();

            // Flatten all segments in this path into a single list of points
            for segment in path {
                for &(x, y) in segment {
                    // Keep integer coordinates exact before applying floating-point calibration.
                    let xd = x.to_f64().filter(|value| value.to_u64() == Some(x)).ok_or(
                        "Trace X coordinate cannot be represented exactly for calibration",
                    )? * samplespacing.x;
                    let yd = y.to_f64().filter(|value| value.to_u64() == Some(y)).ok_or(
                        "Trace Y coordinate cannot be represented exactly for calibration",
                    )? * samplespacing.y;
                    all_points.push(PointElement {
                        x,
                        y,
                        z: 0,
                        xd,
                        yd,
                        zd: 0.0,
                    });
                }
            }

            if !all_points.is_empty() {
                // Calculate real length
                let reallength = calculate_path_length(&all_points);

                paths.push(PathElement {
                    id: path_id,
                    swctype: 0,
                    color: "None".to_string(),
                    channel: 1,
                    frame: 1,
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

        Ok(Self {
            samplespacing,
            imagesize,
            paths,
        })
    }

    /// Convert to XML string with proper XML declaration
    /// # Errors
    /// Returns an error if XML conversion or file I/O fails.
    pub fn to_xml_string(&self) -> Result<String, Box<dyn std::error::Error>> {
        let xml_body = to_string(self)?;
        let xml = format!("<?xml version='1.0' encoding='utf-8'?>\n{xml_body}");
        Ok(xml)
    }

    /// Parse XML string and create Tracings
    /// # Errors
    /// Returns an error if XML conversion or file I/O fails.
    pub fn from_xml_string(xml: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Remove XML declaration if present
        let xml_content = if xml.starts_with("<?xml") {
            xml.split_once('>').map_or(xml, |(_, body)| body.trim())
        } else {
            xml
        };

        let tracings: Self = from_str(xml_content)?;
        Ok(tracings)
    }

    /// Convert to `NerveTracerState`
    #[must_use]
    pub fn to_nerve_tracer_state(&self) -> NerveTracerState {
        let mut state = NerveTracerState::default();
        self.apply_to_nerve_tracer_state(&mut state);
        state
    }

    /// Apply imported trace data without replacing runtime image metadata.
    pub fn apply_to_nerve_tracer_state(&self, state: &mut NerveTracerState) {
        let mut paths = Vec::new();

        for path_element in &self.paths {
            // For now, treat each path as a single segment
            // (We don't have segment boundaries in the XML format)
            if !path_element.points.is_empty() {
                let segment: Vec<(u64, u64)> = path_element
                    .points
                    .iter()
                    .map(|point| (point.x, point.y))
                    .collect();
                paths.push(vec![segment]);
            }
        }

        state.is_enabled = true;
        state.paths = paths;
        state.current_path_segments.clear();
        state.first_point = None;
        state.selected_image_index = 0;
    }
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

/// Export nerve tracer data to XML format
/// # Errors
/// Returns an error if XML conversion or file I/O fails.
pub fn export_nervetracer_to_xml(
    state: &NerveTracerState,
    samplespacing: Option<SampleSpacing>,
    imagesize: Option<ImageSize>,
) -> Result<String, Box<dyn std::error::Error>> {
    let tracings = Tracings::from_nerve_tracer_state(state, samplespacing, imagesize)?;
    tracings.to_xml_string()
}

/// Export nerve tracer data to XML file
/// # Errors
/// Returns an error if XML conversion or file I/O fails.
pub fn export_nervetracer_to_xml_file(
    state: &NerveTracerState,
    file_path: &Path,
    samplespacing: Option<SampleSpacing>,
    imagesize: Option<ImageSize>,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = export_nervetracer_to_xml(state, samplespacing, imagesize)?;
    fs::write(file_path, xml)?;
    Ok(())
}

/// Import nerve tracer data from XML format
/// # Errors
/// Returns an error if XML conversion or file I/O fails.
pub fn import_nervetracer_from_xml(
    xml: &str,
) -> Result<NerveTracerState, Box<dyn std::error::Error>> {
    let tracings = Tracings::from_xml_string(xml)?;
    Ok(tracings.to_nerve_tracer_state())
}

/// Import nerve tracer data from XML file
/// # Errors
/// Returns an error if XML conversion or file I/O fails.
pub fn import_nervetracer_from_xml_file(
    file_path: &Path,
) -> Result<NerveTracerState, Box<dyn std::error::Error>> {
    let xml = fs::read_to_string(file_path)?;
    import_nervetracer_from_xml(&xml)
}

/// Import XML trace data into existing state, retaining runtime image metadata.
/// # Errors
/// Returns an error if file reading or XML parsing fails; leaves state unchanged.
pub fn import_nervetracer_from_xml_file_into(
    file_path: &Path,
    state: &mut NerveTracerState,
) -> Result<(), Box<dyn std::error::Error>> {
    let xml = fs::read_to_string(file_path)?;
    let tracings = Tracings::from_xml_string(&xml)?;
    tracings.apply_to_nerve_tracer_state(state);
    Ok(())
}
