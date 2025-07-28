use std::collections::HashMap;
use std::path::Path;

use quick_xml::de::from_str;
use quick_xml::se::to_string;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::pipeline::counter::{CountData, CounterMode, CounterState, CounterTool};

// XML serialization structures
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "CellCounter_Marker_File")]
pub struct CellCounterMarkerFile {
    #[serde(rename = "Image_Properties")]
    pub image_properties: ImageProperties,
    #[serde(rename = "Marker_Data")]
    pub marker_data: MarkerData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImageProperties {
    #[serde(rename = "Image_Filename")]
    pub image_filename: String,
    #[serde(rename = "X_Calibration")]
    pub x_calibration: f64,
    #[serde(rename = "Y_Calibration")]
    pub y_calibration: f64,
    #[serde(rename = "Z_Calibration")]
    pub z_calibration: f64,
    #[serde(rename = "Calibration_Unit")]
    pub calibration_unit: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MarkerData {
    #[serde(rename = "Current_Type")]
    pub current_type: u16,
    #[serde(rename = "Marker_Type")]
    pub marker_types: Vec<MarkerType>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MarkerType {
    #[serde(rename = "Type")]
    pub type_id: u16,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Marker")]
    pub markers: Vec<Marker>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Marker {
    #[serde(rename = "MarkerX")]
    pub marker_x: u64,
    #[serde(rename = "MarkerY")]
    pub marker_y: u64,
    #[serde(rename = "MarkerZ")]
    pub marker_z: u16,
}

impl From<&CountData> for Marker {
    fn from(count_data: &CountData) -> Self {
        Self {
            marker_x: count_data.position.0,
            marker_y: count_data.position.1,
            marker_z: 1, // Default Z value
        }
    }
}

impl From<&Marker> for CountData {
    fn from(marker: &Marker) -> Self {
        Self {
            position: (marker.marker_x, marker.marker_y),
            size: 1000u16, // Dummy value sinceXML doesn't store this
        }
    }
}

impl Default for ImageProperties {
    fn default() -> Self {
        Self {
            image_filename: "composite.tif".to_string(),
            x_calibration: 1.0,
            y_calibration: 1.0,
            z_calibration: 1.0,
            calibration_unit: "pixel".to_string(),
        }
    }
}

impl CellCounterMarkerFile {
    /// Create a new CellCounterMarkerFile from counter state
    pub fn from_counter_state(state: &CounterState, image_filename: Option<String>) -> Self {
        let mut image_properties = ImageProperties::default();
        if let Some(filename) = image_filename {
            image_properties.image_filename = filename;
        }

        let mut marker_types = Vec::new();

        if state.mode == CounterMode::Manual {
            // Convert manual counts to marker types
            for (&type_id, counts) in &state.manual_counts {
                if !counts.is_empty() {
                    let markers: Vec<Marker> = counts.iter().map(|count| count.into()).collect();
                    marker_types.push(MarkerType {
                        type_id,
                        name: format!("Type {type_id}"),
                        markers,
                    });
                }
            }
        } else if state.mode == CounterMode::Auto {
            let markers: Vec<Marker> = state.auto_counts.iter().map(|count| count.into()).collect();
            marker_types.push(MarkerType {
                type_id: 0u16,
                name: format!("Type {}", 0),
                markers,
            });
        }

        // Sort marker types by type_id for consistent output
        marker_types.sort_by_key(|mt| mt.type_id);

        let current_type = state.active_count_type.unwrap_or(0);

        let marker_data = MarkerData {
            current_type,
            marker_types,
        };

        Self {
            image_properties,
            marker_data,
        }
    }

    /// Convert to XML string with proper XML declaration
    pub fn to_xml_string(&self) -> Result<String, Box<dyn std::error::Error>> {
        let xml_body = to_string(self)?;
        let xml = format!("<?xml version='1.0' encoding='utf-8'?>\n{xml_body}");
        Ok(xml)
    }

    /// Parse XML string and create CellCounterMarkerFile
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

        let cell_counter_file: CellCounterMarkerFile = from_str(xml_content)?;
        Ok(cell_counter_file)
    }

    /// Convert to CounterState
    pub fn to_counter_state(&self) -> CounterState {
        let mut manual_counts = HashMap::new();

        for marker_type in &self.marker_data.marker_types {
            let counts: Vec<CountData> = marker_type
                .markers
                .iter()
                .map(|marker| marker.into())
                .collect();
            if !counts.is_empty() {
                manual_counts.insert(marker_type.type_id, counts);
            }
        }

        CounterState {
            is_enabled: true,
            manual_counts,
            auto_counts: vec![],
            mode: CounterMode::Manual,
            active_tool: CounterTool::Place,
            active_count_type: Some(self.marker_data.current_type),
            filter_proxys: Vec::new(),
            review_current_index: 0,
        }
    }
}

/// Export counter data to CellCounter XML format
pub fn export_counter_to_xml(
    state: &CounterState,
    filename: Option<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cell_counter_file = CellCounterMarkerFile::from_counter_state(state, filename);
    cell_counter_file.to_xml_string()
}

/// Export counter data to CellCounter XML format and save to file
pub async fn export_counter_to_xml_file(
    state: &CounterState,
    file_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let filename = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|s| s.to_string());

    let xml = export_counter_to_xml(state, filename)?;
    fs::write(file_path, xml).await?;
    Ok(())
}

/// Import counter data from CellCounter XML format
pub fn import_counter_from_xml(xml: &str) -> Result<CounterState, Box<dyn std::error::Error>> {
    let cell_counter_file = CellCounterMarkerFile::from_xml_string(xml)?;
    Ok(cell_counter_file.to_counter_state())
}

/// Import counter data from CellCounter XML file
pub async fn import_counter_from_xml_file(
    file_path: &Path,
) -> Result<CounterState, Box<dyn std::error::Error>> {
    let xml = fs::read_to_string(file_path).await?;
    import_counter_from_xml(&xml)
}
