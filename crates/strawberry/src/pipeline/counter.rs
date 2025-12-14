use std::collections::HashMap;

use crate::utils::fitting::{find_counts_within_polygon, ramer_douglas_peucker_closed};

use serde::{Deserialize, Serialize};

use super::{
    BackendToolType, CountMessage, FilterMessage, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};
use crate::overlay::{CircleStyle, OverlayElement, ToolOverlay};
use crate::utils::commands::collect_and_deduplicate_commands;
use crate::utils::consts::CIRCLE_COLORS;
use crate::utils::io::export_counter_to_xml_file;
use async_trait::async_trait;
use lutz::{lutz, Image as LutzImage, Pixel};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CountData {
    pub position: (u64, u64), // X, Y
    pub size: u16,
}

pub struct SizeFilter {
    pub is_enabled: bool,
    pub min_size: u16,
}

impl Filter for SizeFilter {
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    fn name(&self) -> &str {
        "Filter by Size"
    }
    fn process(&self, counts: &Vec<CountData>) -> Vec<CountData> {
        let mut out = counts.clone();
        out.retain(|count| count.size > self.min_size);
        out
    }
    fn receive_message(&mut self, message: &FilterMessage) {
        match message {
            FilterMessage::ConfigureSizeFilter(min_size) => {
                self.min_size = *min_size;
            }
        }
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.is_enabled = enabled
    }
    fn get_values(&self) -> Vec<f64> {
        vec![self.min_size as f64]
    }
}

impl SizeFilter {
    pub fn new(min_size: u16) -> Self {
        Self {
            is_enabled: false,
            min_size,
        }
    }
}

pub trait Filter: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn name(&self) -> &str;
    fn process(&self, counts: &Vec<CountData>) -> Vec<CountData>;
    fn receive_message(&mut self, message: &FilterMessage);
    fn set_enabled(&mut self, enabled: bool);
    fn get_values(&self) -> Vec<f64>;
}

fn update_filter_proxys(input: &Vec<Box<dyn Filter>>) -> Vec<FilterProxy> {
    input
        .iter()
        .map(|filter| FilterProxy {
            name: filter.name().to_string(),
            is_enabled: filter.is_enabled(),
            values: filter.get_values(),
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum CounterMode {
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FilterProxy {
    pub name: String,
    pub is_enabled: bool,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum CounterTool {
    #[default]
    Place,
    Delete,
    LassoDelete,
    Review,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CounterState {
    pub is_enabled: bool,
    pub manual_counts: HashMap<u16, Vec<CountData>>,
    pub auto_counts: Vec<CountData>,
    pub mode: CounterMode,
    pub active_tool: Option<CounterTool>,
    pub active_count_type: Option<u16>,
    pub filter_proxys: Vec<FilterProxy>,
    pub count_type_visibility: HashMap<u16, bool>,
    // Review state fields
    pub review_current_index: usize,
}

// Image wrapper for lutz algorithm
struct BinaryImageWrapper<'a> {
    image: &'a image::RgbaImage,
    threshold: u8,
}

impl<'a> BinaryImageWrapper<'a> {
    fn new(image: &'a image::RgbaImage, threshold: u8) -> Self {
        Self { image, threshold }
    }
}

impl<'a> LutzImage for BinaryImageWrapper<'a> {
    fn width(&self) -> u32 {
        self.image.width()
    }

    fn height(&self) -> u32 {
        self.image.height()
    }

    fn has_pixel(&self, x: u32, y: u32) -> bool {
        if x >= self.width() || y >= self.height() {
            return false;
        }
        let pixel = self.image.get_pixel(x, y);
        // Consider pixel as foreground if it's not transparent and has sufficient brightness
        pixel[3] > 0
            && (pixel[0] as u16 + pixel[1] as u16 + pixel[2] as u16) / 3 > self.threshold as u16
    }
}

pub struct Counter {
    pub state: CounterState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub filters: Vec<Box<dyn Filter>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<CounterState>,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub frontend_latest_state: Option<CounterState>,
    pub lasso_points: Vec<(u64, u64)>,
    pub is_lasso_dragging: bool,
}

#[async_trait]
impl ToolBackend for Counter {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn apply_state(&mut self, _state: ToolState) {}
    fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::CounterState(self.state.clone())
    }
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, output_images: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        if self.state.mode == CounterMode::Auto {
            self.state.auto_counts.clear();

            for image in output_images.iter() {
                let binary_wrapper = BinaryImageWrapper::new(image, 128); // Use threshold of 128
                                                                          //TODO: move to gpu
                for blob_pixels in lutz::<BinaryImageWrapper, Vec<Pixel>>(binary_wrapper) {
                    if !blob_pixels.is_empty() {
                        // Calculate centroid of this blob
                        let sum_x: u64 = blob_pixels.iter().map(|p| p.x as u64).sum();
                        let sum_y: u64 = blob_pixels.iter().map(|p| p.y as u64).sum();
                        let count = blob_pixels.len() as u64;

                        self.state.auto_counts.push(CountData {
                            position: (sum_x / count, sum_y / count),
                            size: blob_pixels.len() as u16,
                        });
                    }
                }

                for filter in self.filters.iter() {
                    if filter.is_enabled() {
                        self.state.auto_counts = filter.process(&self.state.auto_counts);
                    }
                }
            }
        }

        // Push state update to frontend
        self.send_to_frontend(PipelineCommand::FrontendMessage(
            FrontendMessage::ReturnState(self.get_state()),
        ))?;

        Ok(())
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands = collect_and_deduplicate_commands(&self.receiver);

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    ))?;
                }
                PipelineCommand::HandleClick(x, y) => {
                    if !self.state.is_enabled {
                        return Ok(());
                    }
                    if let Some(id) = self.state.active_count_type {
                        match self.state.active_tool {
                            Some(CounterTool::Place) => {
                                if self.state.manual_counts.contains_key(&id) {
                                    let mut count_type = self
                                        .state
                                        .manual_counts
                                        .get(&id)
                                        .expect("Should never fire: Confirmed that key exists")
                                        .clone();
                                    count_type.push(CountData {
                                        position: (x, y),
                                        size: 0,
                                    });
                                    self.state.manual_counts.insert(id, count_type);
                                } else {
                                    self.state.manual_counts.insert(
                                        id,
                                        vec![CountData {
                                            position: (x, y),
                                            size: 0,
                                        }],
                                    );
                                }
                            }
                            Some(CounterTool::Delete) => {
                                if self.state.manual_counts.contains_key(&id) {
                                    let counts = self
                                        .state
                                        .manual_counts
                                        .get(&id)
                                        .expect("Should never fire: Confirmed that key exists")
                                        .clone();
                                    let new_counts: Vec<CountData> = counts
                                        .iter()
                                        .filter(|count| {
                                            let dx = (count.position.0 as i64 - x as i64).abs();
                                            let dy = (count.position.1 as i64 - y as i64).abs();
                                            let distance = ((dx * dx + dy * dy) as f64).sqrt();
                                            distance > 6.0
                                        })
                                        .cloned()
                                        .collect();
                                    self.state.manual_counts.insert(id, new_counts);
                                }
                            }
                            _ => {}
                        }
                    };
                }
                PipelineCommand::InteractionStart(x, y) => {
                    if !self.state.is_enabled {
                        return Ok(());
                    }
                    if self.state.active_tool == Some(CounterTool::LassoDelete) {
                        self.is_lasso_dragging = true;
                        self.lasso_points.clear();
                        self.lasso_points.push((x, y));
                        // Trigger overlay update
                        let overlays = self.get_overlays();
                        if let Some(overlays) = overlays {
                            let _ = self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::ReturnOverlays({
                                    let mut collection = crate::overlay::OverlayCollection::new();
                                    collection.merge(overlays);
                                    collection
                                }),
                            ));
                        }
                    }
                }
                PipelineCommand::InteractionMove(x, y) => {
                    if self.is_lasso_dragging {
                        self.lasso_points.push((x, y));
                        // Trigger overlay update for visual feedback
                        let overlays = self.get_overlays();
                        if let Some(overlays) = overlays {
                            let _ = self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::ReturnOverlays({
                                    let mut collection = crate::overlay::OverlayCollection::new();
                                    collection.merge(overlays);
                                    collection
                                }),
                            ));
                        }
                    }
                }
                PipelineCommand::InteractionEnd => {
                    if self.is_lasso_dragging && !self.lasso_points.is_empty() {
                        // Process the lasso deletion
                        if let Some(active_count_type) = &self.state.active_count_type {
                            let polygon = ramer_douglas_peucker_closed(self.lasso_points.clone());
                            if let Some(counts) = self.state.manual_counts.get(active_count_type) {
                                let filtered = find_counts_within_polygon(polygon, counts.clone());
                                self.state
                                    .manual_counts
                                    .insert(*active_count_type, filtered);
                            }
                        }
                        self.is_lasso_dragging = false;
                        self.lasso_points.clear();
                    }
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::CountMessage(count_message) => match count_message {
                        CountMessage::NewCountType(id) => {
                            self.state.manual_counts.insert(id, vec![]);
                            self.state.count_type_visibility.insert(id, true);
                            self.state.active_count_type = Some(id);
                        }
                        CountMessage::DeletionLasso(points) => {
                            if let Some(active_count_type) = &self.state.active_count_type {
                                let polygon = ramer_douglas_peucker_closed(points);
                                let counts = self
                                    .state
                                    .manual_counts
                                    .get(active_count_type)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "Active count type {} not found",
                                            active_count_type
                                        )
                                    })?
                                    .clone();
                                self.state.manual_counts.insert(
                                    *active_count_type,
                                    find_counts_within_polygon(polygon, counts),
                                );
                            }
                        }
                        CountMessage::SetActiveCountType(id) => {
                            self.state.active_count_type = Some(id);
                        }
                        CountMessage::SetActiveTool(tool) => {
                            self.state.active_tool = tool;
                        }
                        CountMessage::DeleteCountType(id) => {
                            self.state.manual_counts.remove(&id);
                            self.state.count_type_visibility.remove(&id);
                        }
                        CountMessage::ToggleCountTypeVisibility(id) => {
                            let current_visibility =
                                self.state.count_type_visibility.get(&id).unwrap_or(&true);
                            self.state
                                .count_type_visibility
                                .insert(id, !current_visibility);
                        }
                        CountMessage::SetMode(new_mode) => {
                            self.state.mode = new_mode;
                        }
                        CountMessage::FilterMessage(message) => {
                            for filter in &mut self.filters {
                                filter.receive_message(&message);
                            }
                        }
                        CountMessage::ToggleFilter(filter_index, enabled) => {
                            let index = filter_index as usize;
                            if index >= self.filters.len() {
                                return Err(anyhow::anyhow!(
                                    "Filter index {} out of bounds (max: {})",
                                    index,
                                    self.filters.len()
                                ));
                            }
                            self.filters[index].set_enabled(enabled);
                        }
                        CountMessage::SaveCounts(file) => {
                            // Use the utils::io module to export to XML
                            if let Err(e) = export_counter_to_xml_file(&self.state, &file).await {
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::UserFeedback(
                                        "Error saving counts: XML export failed".to_string(),
                                    ),
                                ))?;
                                return Err(anyhow::anyhow!("Error saving counts to file: {}", e));
                            }
                            self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::UserFeedback(
                                    "Counts saved successfully!".to_string(),
                                ),
                            ))?;
                        }
                        CountMessage::LoadCounts(file) => {
                            match crate::utils::io::import_counter_from_xml_file(&file).await {
                                Ok(loaded_state) => {
                                    self.state.manual_counts = loaded_state.manual_counts;
                                    self.state.auto_counts = loaded_state.auto_counts;
                                    self.state.mode = loaded_state.mode;
                                    self.state.active_count_type = loaded_state.active_count_type;

                                    // Initialize visibility for all loaded count types
                                    self.state.count_type_visibility.clear();
                                    for &count_type_id in self.state.manual_counts.keys() {
                                        self.state
                                            .count_type_visibility
                                            .insert(count_type_id, true);
                                    }

                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::UserFeedback(
                                            "Counts loaded successfully!".to_string(),
                                        ),
                                    ))?;
                                    println!("Sent count loaded user feedback over command_sender broadcast channel");
                                }
                                Err(e) => {
                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::UserFeedback(
                                            "Error loading counts: XML import failed".to_string(),
                                        ),
                                    ))?;
                                    return Err(anyhow::anyhow!(
                                        "Error loading counts from file: {}",
                                        e
                                    ));
                                }
                            }
                        }
                        CountMessage::CopyToManual => {
                            let new_key = self.state.manual_counts.len() as u16;
                            self.state
                                .manual_counts
                                .insert(new_key, self.state.auto_counts.clone());
                            self.state.count_type_visibility.insert(new_key, true);
                            self.state.mode = CounterMode::Manual;
                            self.state.active_count_type = Some(0)
                        }
                        CountMessage::ReviewNavigatePrevious => {
                            if let Some(active_count_type) = self.state.active_count_type {
                                if let Some(counts) =
                                    self.state.manual_counts.get(&active_count_type)
                                {
                                    let current_index = self.state.review_current_index;
                                    let counts_len = counts.len();

                                    if counts_len > 0 {
                                        let new_index = if current_index == 0 {
                                            counts_len - 1
                                        } else {
                                            current_index - 1
                                        };
                                        self.state.review_current_index = new_index;

                                        // Emit snap to position
                                        if let Some(count) = counts.get(new_index) {
                                            let _ = self.global_sender.send(
                                                PipelineCommand::FrontendMessage(
                                                    FrontendMessage::SnapToPosition(
                                                        count.position.0 as f32,
                                                        count.position.1 as f32,
                                                    ),
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        CountMessage::ReviewNavigateNext => {
                            if let Some(active_count_type) = self.state.active_count_type {
                                if let Some(counts) =
                                    self.state.manual_counts.get(&active_count_type)
                                {
                                    let current_index = self.state.review_current_index;
                                    let counts_len = counts.len();

                                    if counts_len > 0 {
                                        let new_index = (current_index + 1) % counts_len;
                                        self.state.review_current_index = new_index;

                                        // Emit snap to position
                                        if let Some(count) = counts.get(new_index) {
                                            let _ = self.global_sender.send(
                                                PipelineCommand::FrontendMessage(
                                                    FrontendMessage::SnapToPosition(
                                                        count.position.0 as f32,
                                                        count.position.1 as f32,
                                                    ),
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        CountMessage::DeleteCurrentCount => {
                            if let Some(active_count_type) = self.state.active_count_type {
                                if let Some(counts) =
                                    self.state.manual_counts.get_mut(&active_count_type)
                                {
                                    let current_index = self.state.review_current_index;

                                    if current_index < counts.len() {
                                        counts.remove(current_index);

                                        // Adjust review index if necessary
                                        if counts.is_empty() {
                                            self.state.review_current_index = 0;
                                        } else if current_index >= counts.len() {
                                            self.state.review_current_index = counts.len() - 1;
                                        }
                                    }
                                }
                            }
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        let new_filter_proxys = update_filter_proxys(&self.filters);
        if new_filter_proxys != self.state.filter_proxys {
            self.state.filter_proxys = new_filter_proxys;
        }

        if self.frontend_latest_state.is_none()
            || self
                .frontend_latest_state
                .as_ref()
                .expect("Just checked that frontend_latest_state is some")
                != &self.state
        {
            self.update_state()?;
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Counter
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    fn has_valid_cache(&self) -> bool {
        if let Some(cached_state) = &self.cached_state {
            cached_state == &self.state
        } else {
            false
        }
    }

    fn get_cached_output(&self) -> Option<Vec<image::RgbaImage>> {
        self.cached_output.clone()
    }

    fn set_cached_output(&mut self, output: Vec<image::RgbaImage>) {
        self.cached_output = Some(output);
        self.cached_state = Some(self.state.clone());
    }

    fn invalidate_cache(&mut self) {
        self.cached_output = None;
        self.cached_state = None;
    }

    fn get_overlays(&self) -> Option<crate::overlay::ToolOverlay> {
        if !self.state.is_enabled {
            return None;
        }

        let mut elements = Vec::new();

        // Manual counts
        for (count_type_id, counts) in &self.state.manual_counts {
            let is_visible = self
                .state
                .count_type_visibility
                .get(count_type_id)
                .unwrap_or(&true);
            if !*is_visible {
                continue;
            }

            let color_index = (*count_type_id as usize) % CIRCLE_COLORS.len();
            let color = CIRCLE_COLORS[color_index].0;

            for (index, count) in counts.iter().enumerate() {
                let is_selected = self.state.active_tool == Some(CounterTool::Review)
                    && self.state.active_count_type == Some(*count_type_id)
                    && index == self.state.review_current_index;

                let circle_color = if is_selected {
                    [255, 213, 0, 255] // Hot yellow for selected
                } else {
                    color
                };

                elements.push(OverlayElement::Circle {
                    x: count.position.0 as f32,
                    y: count.position.1 as f32,
                    radius: 10.0,
                    color: circle_color,
                    style: CircleStyle::Border { width: 2.0 },
                });
            }
        }

        // Auto counts
        if self.state.mode == CounterMode::Auto {
            let color = CIRCLE_COLORS[0].0; // Use first color for auto counts
            for count in &self.state.auto_counts {
                elements.push(OverlayElement::Circle {
                    x: count.position.0 as f32,
                    y: count.position.1 as f32,
                    radius: 10.0,
                    color,
                    style: CircleStyle::Border { width: 2.0 },
                });
            }
        }

        // Lasso points (during drag)
        if self.is_lasso_dragging && self.lasso_points.len() > 1 {
            for point in &self.lasso_points {
                elements.push(OverlayElement::Point {
                    x: point.0 as f32,
                    y: point.1 as f32,
                    radius: 2.0,
                    color: [255, 0, 0, 255], // Red
                });
            }
        }

        Some(ToolOverlay {
            tool_id: crate::pipeline::BackendToolType::Counter,
            z_order: 100, // High priority - on top
            elements,
        })
    }
}

impl Counter {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> anyhow::Result<Self> {
        let (sender, receiver) = flume::unbounded();
        command_sender
            .send(PipelineCommand::RegisterClickReceiver(sender.clone()))
            .map_err(|e| anyhow::anyhow!("Failed to register click receiver: {}", e))?;

        let filters: Vec<Box<dyn Filter>> = vec![Box::new(SizeFilter::new(10))];
        let filter_proxys = update_filter_proxys(&filters);

        Ok(Self {
            state: CounterState {
                is_enabled: false,
                manual_counts: HashMap::new(),
                auto_counts: Vec::new(),
                mode: CounterMode::Manual,
                active_tool: None,
                active_count_type: None,
                review_current_index: 0,
                filter_proxys,
                count_type_visibility: HashMap::new(),
            },
            frontend_senders: vec![],
            index: 0,
            filters,
            command_sender,
            sender,
            receiver,
            global_sender,
            cached_output: None,
            cached_state: None,
            frontend_latest_state: None,
            lasso_points: Vec::new(),
            is_lasso_dragging: false,
        })
    }
    fn update_state(&mut self) -> anyhow::Result<()> {
        self.invalidate_cache();
        let message =
            PipelineCommand::FrontendMessage(FrontendMessage::ReturnState(self.get_state()));
        self.send_to_frontend(message)?;
        self.frontend_latest_state = Some(self.state.clone());
        self.command_sender
            .send(PipelineCommand::Refresh(self.index))
            .map_err(|e| anyhow::anyhow!("Failed to send refresh command: {}", e))?;
        Ok(())
    }

    fn send_to_frontend(&self, message: PipelineCommand) -> anyhow::Result<()> {
        for (i, sender) in self.frontend_senders.iter().enumerate() {
            sender
                .send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send message to sender {}: {}", i, e))?;
        }
        Ok(())
    }
}
