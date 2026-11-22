use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::path::Path;

use quick_xml::de::from_str;
use quick_xml::se::to_string;
use serde::{Deserialize, Serialize};
use std::fs;

use strawberry_types::{
    AnnotationPlacement, CountData, CounterMode, CounterState, Position, PositionCounts, Shape,
};

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

impl Marker {
    /// Convert a count data object to a marker
    const fn from_count(count_data: &CountData, marker_z: u16) -> Self {
        Self {
            marker_x: count_data.position.0,
            marker_y: count_data.position.1,
            marker_z,
        }
    }

    /// Return the position this marker was saved on, reading its one-based slice number
    /// against `shape` on the tile of `selected`.
    fn get_saved_position(&self, shape: Shape, selected: Position) -> Position {
        shape.get_slice_position(usize::from(self.marker_z.saturating_sub(1)), selected)
    }
}

/// Return a position's counts that a marker file of `mode` records, by count type.
fn get_exported_counts(counts: &PositionCounts, mode: CounterMode) -> Vec<(u16, &Vec<CountData>)> {
    match mode {
        CounterMode::Manual => counts.manual.iter().map(|(&id, c)| (id, c)).collect(),
        CounterMode::Auto => vec![(0, &counts.auto)],
    }
}

impl From<&Marker> for CountData {
    /// Convert an XML marker to a count with a default marker size.
    fn from(marker: &Marker) -> Self {
        Self {
            position: (marker.marker_x, marker.marker_y),
            size: 1000u16, // Dummy value sinceXML doesn't store this
        }
    }
}

impl Default for ImageProperties {
    /// Create the default values for this type.
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
    /// Create a marker file from the counts of every position on the selected tile, numbering
    /// its depth slices through time points as one-based slices in Z.
    ///
    /// # Errors
    /// Returns an error if counts lie outside the shape or a slice number exceeds the marker Z
    /// range.
    pub fn from_counter_state(
        state: &CounterState,
        image_filename: Option<String>,
    ) -> Result<Self, Box<dyn Error>> {
        let shape = state.shape;
        let selected = state.counts.selected_position();
        // Marker files cannot number tiles, so other tiles' counts would land on this tile.
        let tile_counts = || {
            state
                .counts
                .iter()
                .filter(move |(position, _)| position.shares_tile_with(selected))
        };
        shape.ensure_contains_positions(
            tile_counts()
                .filter(|(_, counts)| {
                    get_exported_counts(counts, state.mode)
                        .iter()
                        .any(|(_, counts)| !counts.is_empty())
                })
                .map(|(position, _)| position),
            "Counts",
        )?;
        let mut image_properties = ImageProperties::default();
        if let Some(filename) = image_filename {
            image_properties.image_filename = filename;
        }

        // Ordered so marker types are written by ID.
        let mut markers_by_type: BTreeMap<u16, Vec<Marker>> = BTreeMap::new();
        if state.mode == CounterMode::Auto {
            markers_by_type.insert(0, Vec::new());
        }
        for (position, counts) in tile_counts() {
            let typed_counts = get_exported_counts(counts, state.mode);
            if typed_counts.iter().all(|(_, counts)| counts.is_empty()) {
                continue;
            }
            let marker_z = shape
                .get_slice_index(position)
                .and_then(|slice| slice.checked_add(1))
                .ok_or("Count position exceeds the slice range")?;
            let marker_z = u16::try_from(marker_z)?;
            for (type_id, counts) in typed_counts {
                markers_by_type.entry(type_id).or_default().extend(
                    counts
                        .iter()
                        .map(|count| Marker::from_count(count, marker_z)),
                );
            }
        }

        let marker_types = markers_by_type
            .into_iter()
            .filter(|(_, markers)| state.mode == CounterMode::Auto || !markers.is_empty())
            .map(|(type_id, markers)| MarkerType {
                type_id,
                name: state
                    .count_type_names
                    .get(&type_id)
                    .filter(|_| state.mode == CounterMode::Manual)
                    .cloned()
                    .unwrap_or_else(|| format!("Type {type_id}")),
                markers,
            })
            .collect();

        Ok(Self {
            image_properties,
            marker_data: MarkerData {
                current_type: state.active_count_type.unwrap_or(0),
                marker_types,
            },
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

    /// Parse XML string and create `CellCounterMarkerFile`
    /// # Errors
    /// Returns an error if XML conversion or file I/O fails.
    pub fn from_xml_string(xml: &str) -> Result<Self, Box<dyn Error>> {
        // Remove XML declaration if present
        let xml_content = if xml.starts_with("<?xml") {
            xml.split_once('>').map_or(xml, |(_, body)| body.trim())
        } else {
            xml
        };

        let cell_counter_file: Self = from_str(xml_content)?;
        Ok(cell_counter_file)
    }

    /// Return the positions markers were saved on, reading their slice numbers against `shape`
    /// on the tile of `selected`.
    #[must_use]
    pub fn get_saved_positions(&self, shape: Shape, selected: Position) -> HashSet<Position> {
        self.marker_data
            .marker_types
            .iter()
            .flat_map(|marker_type| &marker_type.markers)
            .map(|marker| marker.get_saved_position(shape, selected))
            .collect()
    }

    /// Replace the manual counts of each position the markers are placed on, reading slice
    /// numbers against the counter's shape on the selected tile, and register their types.
    ///
    /// # Errors
    /// Returns an error, leaving the state unchanged, if markers split across positions would
    /// land outside the counter's shape.
    pub fn apply_to_counter_state(
        &self,
        state: &mut CounterState,
        placement: AnnotationPlacement,
    ) -> Result<(), Box<dyn Error>> {
        let selected = state.counts.selected_position();
        if placement == AnnotationPlacement::SplitByPosition {
            state.shape.ensure_contains_positions(
                self.get_saved_positions(state.shape, selected),
                "Counts",
            )?;
        }
        let mut imported: HashMap<Position, HashMap<u16, Vec<CountData>>> = HashMap::new();
        if placement == AnnotationPlacement::CurrentPosition {
            imported.insert(selected, HashMap::new());
        }

        for marker_type in &self.marker_data.marker_types {
            if marker_type.markers.is_empty() {
                continue;
            }
            for marker in &marker_type.markers {
                let position = match placement {
                    AnnotationPlacement::CurrentPosition => selected,
                    AnnotationPlacement::SplitByPosition => {
                        marker.get_saved_position(state.shape, selected)
                    }
                };
                imported
                    .entry(position)
                    .or_default()
                    .entry(marker_type.type_id)
                    .or_default()
                    .push(marker.into());
            }
            state
                .count_type_names
                .insert(marker_type.type_id, marker_type.name.clone());
            state
                .count_type_visibility
                .insert(marker_type.type_id, true);
        }

        for (position, manual) in imported {
            state.counts.position_mut(position).manual = manual;
        }
        let current_type = self.marker_data.current_type;
        if state.count_type_names.contains_key(&current_type) {
            state.active_count_type = Some(current_type);
        }
        state.is_enabled = true;
        state.mode = CounterMode::Manual;
        state.review_current_index = 0;
        Ok(())
    }
}

/// Export the counts of every position on the selected tile to a `CellCounter` XML file.
/// # Errors
/// Returns an error if counts lie outside the counter's shape, or XML conversion or file I/O
/// fails.
pub fn export_counter_to_xml_file(
    state: &CounterState,
    file_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let filename = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(std::string::ToString::to_string);

    let xml = CellCounterMarkerFile::from_counter_state(state, filename)?.to_xml_string()?;
    fs::write(file_path, xml)?;
    Ok(())
}

/// Read and parse a `CellCounter` XML file.
/// # Errors
/// Returns an error if file reading or XML parsing fails.
pub fn read_marker_file(file_path: &Path) -> Result<CellCounterMarkerFile, Box<dyn Error>> {
    CellCounterMarkerFile::from_xml_string(&fs::read_to_string(file_path)?)
}
