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
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            depth: 1,
        }
    }
}

impl Tracings {
    /// Create a new Tracings structure from NerveTracerState
    pub fn from_nerve_tracer_state(
        state: &NerveTracerState,
        samplespacing: Option<SampleSpacing>,
        imagesize: Option<ImageSize>,
    ) -> Self {
        let samplespacing = samplespacing.unwrap_or_default();
        let imagesize = imagesize.unwrap_or_default();

        let mut paths = Vec::new();
        let mut path_id = 1;

        // Convert committed paths
        for path in &state.paths {
            let mut all_points = Vec::new();

            // Flatten all segments in this path into a single list of points
            for segment in path {
                for &(x, y) in segment {
                    let xd = x as f64 * samplespacing.x;
                    let yd = y as f64 * samplespacing.y;
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
                    name: format!("Path ({})", path_id),
                    reallength,
                    points: all_points,
                });
                path_id += 1;
            }
        }

        Self {
            samplespacing,
            imagesize,
            paths,
        }
    }

    /// Convert to XML string with proper XML declaration
    pub fn to_xml_string(&self) -> Result<String, Box<dyn std::error::Error>> {
        let xml_body = to_string(self)?;
        let xml = format!("<?xml version='1.0' encoding='utf-8'?>\n{}", xml_body);
        Ok(xml)
    }

    /// Parse XML string and create Tracings
    pub fn from_xml_string(xml: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Remove XML declaration if present
        let xml_content = if xml.starts_with("<?xml") {
            if let Some(start) = xml.find('>') {
                xml[start + 1..].trim()
            } else {
                xml
            }
        } else {
            xml
        };

        let tracings: Tracings = from_str(xml_content)?;
        Ok(tracings)
    }

    /// Convert to NerveTracerState
    pub fn to_nerve_tracer_state(&self) -> NerveTracerState {
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

        NerveTracerState {
            is_enabled: true,
            paths,
            current_path_segments: vec![],
            first_point: None,
            selected_image_index: 0,
            total_images: 0,
        }
    }
}

/// Calculate the real length of a path by summing distances between consecutive points
fn calculate_path_length(points: &[PointElement]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }

    let mut length = 0.0;
    for i in 0..points.len() - 1 {
        let dx = points[i + 1].xd - points[i].xd;
        let dy = points[i + 1].yd - points[i].yd;
        let dz = points[i + 1].zd - points[i].zd;
        length += (dx * dx + dy * dy + dz * dz).sqrt();
    }
    length
}

/// Export nerve tracer data to XML format
pub fn export_nervetracer_to_xml(
    state: &NerveTracerState,
    samplespacing: Option<SampleSpacing>,
    imagesize: Option<ImageSize>,
) -> Result<String, Box<dyn std::error::Error>> {
    let tracings = Tracings::from_nerve_tracer_state(state, samplespacing, imagesize);
    tracings.to_xml_string()
}

/// Export nerve tracer data to XML file
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
pub fn import_nervetracer_from_xml(
    xml: &str,
) -> Result<NerveTracerState, Box<dyn std::error::Error>> {
    let tracings = Tracings::from_xml_string(xml)?;
    Ok(tracings.to_nerve_tracer_state())
}

/// Import nerve tracer data from XML file
pub fn import_nervetracer_from_xml_file(
    file_path: &Path,
) -> Result<NerveTracerState, Box<dyn std::error::Error>> {
    let xml = fs::read_to_string(file_path)?;
    import_nervetracer_from_xml(&xml)
}
