use anyhow::Context as _;
use num_traits::ToPrimitive;

use std::collections::HashMap;
use std::sync::Arc;

use crate::counter::fitting::{find_counts_within_polygon, ramer_douglas_peucker_closed};

use crate::counter::io::export_counter_to_xml_file;

use async_trait::async_trait;
use lutz::{Image as LutzImage, Pixel, lutz};
use strawberry_types::consts::CIRCLE_COLORS;
use strawberry_types::overlay::{CircleStyle, OverlayElement, ToolOverlay};
use strawberry_types::{
    BackendToolType, CountData, CountMessage, CounterMode, CounterState, CounterTool,
    FilterMessage, FilterProxy, FrontendMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};

pub struct SizeFilter {
    pub is_enabled: bool,
    pub min_size: u16,
}

impl Filter for SizeFilter {
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    fn name(&self) -> &'static str {
        "Filter by Size"
    }
    fn process(&self, counts: &[CountData]) -> Vec<CountData> {
        let mut out = counts.to_vec();
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
        self.is_enabled = enabled;
    }
    fn get_values(&self) -> Vec<f64> {
        vec![f64::from(self.min_size)]
    }
}

impl SizeFilter {
    #[must_use]
    pub const fn new(min_size: u16) -> Self {
        Self {
            is_enabled: false,
            min_size,
        }
    }
}

pub trait Filter: Send + Sync {
    fn is_enabled(&self) -> bool;
    fn name(&self) -> &str;
    fn process(&self, counts: &[CountData]) -> Vec<CountData>;
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

// Image wrapper for lutz algorithm
struct BinaryImageWrapper<'a> {
    image: &'a image::RgbaImage,
    threshold: u8,
}

impl<'a> BinaryImageWrapper<'a> {
    const fn new(image: &'a image::RgbaImage, threshold: u8) -> Self {
        Self { image, threshold }
    }
}

impl LutzImage for BinaryImageWrapper<'_> {
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
            && u16::from(pixel[0])
                .saturating_add(u16::from(pixel[1]))
                .saturating_add(u16::from(pixel[2]))
                .checked_div(3)
                .unwrap_or_default()
                > u16::from(self.threshold)
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
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub cache: crate::tools::cache::ToolCache<CounterState>,
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
    async fn process(
        &mut self,
        output_images: &mut Vec<Arc<image::RgbaImage>>,
    ) -> anyhow::Result<()> {
        if self.state.mode == CounterMode::Auto {
            self.state.auto_counts.clear();

            for image in output_images.iter() {
                let binary_wrapper = BinaryImageWrapper::new(image, 128); // Use threshold of 128
                //TODO: move to gpu
                for blob_pixels in lutz::<BinaryImageWrapper, Vec<Pixel>>(binary_wrapper) {
                    if !blob_pixels.is_empty() {
                        // Calculate centroid of this blob
                        let sum_x: u64 = blob_pixels.iter().map(|p| u64::from(p.x)).sum();
                        let sum_y: u64 = blob_pixels.iter().map(|p| u64::from(p.y)).sum();
                        let count = u64::try_from(blob_pixels.len())?;

                        self.state.auto_counts.push(CountData {
                            position: (
                                sum_x.checked_div(count).unwrap_or_default(),
                                sum_y.checked_div(count).unwrap_or_default(),
                            ),
                            size: u16::try_from(blob_pixels.len()).unwrap_or(u16::MAX),
                        });
                    }
                }

                for filter in &self.filters {
                    if filter.is_enabled() {
                        self.state.auto_counts = filter.process(&self.state.auto_counts);
                    }
                }
            }
        }

        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Command dispatch as one function is cleaner"
    )]
    fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands: Vec<PipelineCommand> =
            std::iter::from_fn(|| self.receiver.try_recv().ok()).collect();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(&PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    ))?;
                }
                PipelineCommand::HandleClick(x, y) => {
                    // `continue`, not `return`: bailing out of the whole batch
                    // would silently drop every command drained after this one.
                    if !self.state.is_enabled {
                        continue;
                    }
                    if let Some(id) = self.state.active_count_type {
                        match self.state.active_tool {
                            Some(CounterTool::Place) => {
                                self.state
                                    .manual_counts
                                    .entry(id)
                                    .or_default()
                                    .push(CountData {
                                        position: (x, y),
                                        size: 0,
                                    });
                            }
                            Some(CounterTool::Delete) => {
                                if let Some(counts) = self.state.manual_counts.get_mut(&id) {
                                    counts.retain(|count| {
                                        let dx = count.position.0.abs_diff(x);
                                        let dy = count.position.1.abs_diff(y);
                                        // A six-pixel radius; avoid signed conversion and overflow.
                                        dx > 6
                                            || dy > 6
                                            || dx
                                                .saturating_mul(dx)
                                                .saturating_add(dy.saturating_mul(dy))
                                                > 36
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
                PipelineCommand::InteractionStart(x, y) => {
                    if !self.state.is_enabled {
                        continue;
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
                                let filtered = find_counts_within_polygon(&polygon, counts.clone());
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
                            self.state.count_type_names.insert(id, format!("Type {id}"));
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
                                            "Active count type {active_count_type} not found"
                                        )
                                    })?
                                    .clone();
                                self.state.manual_counts.insert(
                                    *active_count_type,
                                    find_counts_within_polygon(&polygon, counts),
                                );
                            }
                        }
                        CountMessage::SetActiveCountType(id) => {
                            self.state.active_count_type = Some(id);
                            // The review cursor is per-tool, not per-type; reset it so
                            // it can't dangle past the end of a shorter list.
                            self.state.review_current_index = 0;
                        }
                        CountMessage::SetActiveTool(tool) => {
                            self.state.active_tool = tool;
                        }
                        CountMessage::DeleteCountType(id) => {
                            self.state.manual_counts.remove(&id);
                            self.state.count_type_visibility.remove(&id);
                            self.state.count_type_names.remove(&id);
                            // Never let active_count_type dangle
                            if self.state.active_count_type == Some(id) {
                                self.state.active_count_type =
                                    self.state.manual_counts.keys().min().copied();
                                self.state.review_current_index = 0;
                            }
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
                            let index = usize::from(filter_index);
                            if index >= self.filters.len() {
                                return Err(anyhow::anyhow!(
                                    "Filter index {} out of bounds (max: {})",
                                    index,
                                    self.filters.len()
                                ));
                            }
                            if let Some(filter) = self.filters.get_mut(index) {
                                filter.set_enabled(enabled);
                            }
                        }
                        CountMessage::SaveCounts(file) => {
                            // Use the utils::io module to export to XML
                            if let Err(e) = export_counter_to_xml_file(&self.state, &file) {
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(
                                        "Error saving counts: XML export failed".to_string(),
                                    ),
                                ))?;
                                return Err(anyhow::anyhow!("Error saving counts to file: {e}"));
                            }
                            self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::success("Counts saved successfully!".to_string()),
                            ))?;
                        }
                        CountMessage::LoadCounts(file) => {
                            match crate::counter::io::import_counter_from_xml_file(&file) {
                                Ok(loaded_state) => {
                                    self.state.manual_counts = loaded_state.manual_counts;
                                    self.state.auto_counts = loaded_state.auto_counts;
                                    self.state.mode = loaded_state.mode;
                                    self.state.active_count_type = loaded_state.active_count_type;
                                    self.state.count_type_names = loaded_state.count_type_names;
                                    self.state.is_enabled = true;

                                    // Initialize visibility for all loaded count types
                                    self.state.count_type_visibility.clear();
                                    for &count_type_id in self.state.manual_counts.keys() {
                                        self.state
                                            .count_type_visibility
                                            .insert(count_type_id, true);
                                    }

                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::success(
                                            "Counts loaded successfully!".to_string(),
                                        ),
                                    ))?;
                                    println!(
                                        "Sent count loaded user feedback over command_sender broadcast channel"
                                    );
                                }
                                Err(e) => {
                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::error(
                                            "Error loading counts: XML import failed".to_string(),
                                        ),
                                    ))?;
                                    return Err(anyhow::anyhow!(
                                        "Error loading counts from file: {e}"
                                    ));
                                }
                            }
                        }
                        CountMessage::CopyToManual => {
                            // `len()` collides with existing ids once types have
                            // been deleted; pick the first genuinely free id.
                            let new_key = (0..=u16::MAX)
                                .find(|k| !self.state.manual_counts.contains_key(k))
                                .ok_or_else(|| anyhow::anyhow!("No count type IDs available"))?;
                            self.state
                                .manual_counts
                                .insert(new_key, self.state.auto_counts.clone());
                            self.state.count_type_visibility.insert(new_key, true);
                            self.state
                                .count_type_names
                                .insert(new_key, format!("Type {new_key}"));
                            self.state.mode = CounterMode::Manual;
                            self.state.active_count_type = Some(new_key);
                            self.state.review_current_index = 0;
                        }
                        CountMessage::ReviewNavigatePrevious => {
                            if let Some(active_count_type) = self.state.active_count_type
                                && let Some(counts) =
                                    self.state.manual_counts.get(&active_count_type)
                            {
                                let counts_len = counts.len();

                                if counts_len > 0 {
                                    // Clamp: the cursor can be stale when counts were
                                    // trimmed outside review (Delete tool, lasso).
                                    let current_index = self
                                        .state
                                        .review_current_index
                                        .min(counts_len.saturating_sub(1));
                                    let new_index = if current_index == 0 {
                                        counts_len.saturating_sub(1)
                                    } else {
                                        current_index.saturating_sub(1)
                                    };
                                    self.state.review_current_index = new_index;

                                    // Emit snap to position
                                    if let Some(count) = counts.get(new_index) {
                                        let _ = self.global_sender.send(
                                            PipelineCommand::FrontendMessage(
                                                FrontendMessage::SnapToPosition(
                                                    count.position.0.to_f32().context(
                                                        "Count coordinate cannot be displayed",
                                                    )?,
                                                    count.position.1.to_f32().context(
                                                        "Count coordinate cannot be displayed",
                                                    )?,
                                                ),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        CountMessage::ReviewNavigateNext => {
                            if let Some(active_count_type) = self.state.active_count_type
                                && let Some(counts) =
                                    self.state.manual_counts.get(&active_count_type)
                            {
                                let counts_len = counts.len();

                                if counts_len > 0 {
                                    // Clamp: the cursor can be stale when counts were
                                    // trimmed outside review (Delete tool, lasso).
                                    let current_index = self
                                        .state
                                        .review_current_index
                                        .min(counts_len.saturating_sub(1));
                                    let new_index = (current_index.saturating_add(1))
                                        .checked_rem(counts_len)
                                        .unwrap_or_default();
                                    self.state.review_current_index = new_index;

                                    // Emit snap to position
                                    if let Some(count) = counts.get(new_index) {
                                        let _ = self.global_sender.send(
                                            PipelineCommand::FrontendMessage(
                                                FrontendMessage::SnapToPosition(
                                                    count.position.0.to_f32().context(
                                                        "Count coordinate cannot be displayed",
                                                    )?,
                                                    count.position.1.to_f32().context(
                                                        "Count coordinate cannot be displayed",
                                                    )?,
                                                ),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        CountMessage::DeleteCurrentCount => {
                            if let Some(active_count_type) = self.state.active_count_type
                                && let Some(counts) =
                                    self.state.manual_counts.get_mut(&active_count_type)
                                && !counts.is_empty()
                            {
                                // Clamp instead of the old `index < len` guard: a
                                // stale cursor (counts trimmed outside review) must
                                // still delete something, not silently no-op.
                                let current_index = self
                                    .state
                                    .review_current_index
                                    .min(counts.len().saturating_sub(1));
                                counts.remove(current_index);

                                self.state.review_current_index = if counts.is_empty() {
                                    0
                                } else {
                                    current_index.min(counts.len().saturating_sub(1))
                                };
                                // Deleting the last count leaves nothing to snap to.
                                if let Some(count) = counts.get(self.state.review_current_index) {
                                    let _ =
                                        self.global_sender.send(PipelineCommand::FrontendMessage(
                                            FrontendMessage::SnapToPosition(
                                                count.position.0.to_f32().context(
                                                    "Count coordinate cannot be displayed",
                                                )?,
                                                count.position.1.to_f32().context(
                                                    "Count coordinate cannot be displayed",
                                                )?,
                                            ),
                                        ));
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

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Counter
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    crate::impl_tool_cache!();

    /// Push state to the frontend without discarding the cached image.
    fn sync_frontend(&mut self) -> anyhow::Result<()> {
        if !self.cache.sync.needs_frontend_sync(&self.state) {
            return Ok(());
        }
        let message =
            PipelineCommand::FrontendMessage(FrontendMessage::ReturnState(self.get_state()));
        self.send_to_frontend(&message)?;
        self.cache.sync.frontend_latest_state = Some(self.state.clone());
        self.command_sender
            .send(PipelineCommand::RequestOverlays)
            .map_err(|e| anyhow::anyhow!("Failed to request overlay refresh: {e}"))?;
        Ok(())
    }

    fn get_overlays(&self) -> Option<crate::overlay::ToolOverlay> {
        // Only the overlay is rounded to f32; stored count positions remain exact integers.
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

            let color_index = usize::from(*count_type_id)
                .checked_rem(CIRCLE_COLORS.len())
                .unwrap_or_default();
            let color = CIRCLE_COLORS
                .get(color_index)
                .copied()
                .unwrap_or(CIRCLE_COLORS[0])
                .0;

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
                    x: count.position.0.to_f32()?,
                    y: count.position.1.to_f32()?,
                    radius: 10.0 / 3.0,
                    color: circle_color,
                    style: CircleStyle::Border { width: 0.7 },
                });
            }
        }

        // Auto counts
        if self.state.mode == CounterMode::Auto {
            let color = CIRCLE_COLORS[0].0; // Use first color for auto counts
            for count in &self.state.auto_counts {
                elements.push(OverlayElement::Circle {
                    x: count.position.0.to_f32()?,
                    y: count.position.1.to_f32()?,
                    radius: 10.0 / 3.0,
                    color,
                    style: CircleStyle::Border { width: 0.7 },
                });
            }
        }

        // Lasso points (during drag)
        if self.is_lasso_dragging && self.lasso_points.len() > 1 {
            for point in &self.lasso_points {
                elements.push(OverlayElement::Point {
                    x: point.0.to_f32()?,
                    y: point.1.to_f32()?,
                    radius: 2.0,
                    color: [255, 0, 0, 255], // Red
                });
            }
        }

        Some(ToolOverlay {
            tool_id: strawberry_types::BackendToolType::Counter,
            z_order: 100, // High priority - on top
            elements,
        })
    }
}

impl Counter {
    /// # Errors
    /// Returns an error if a command fails or a communication channel is disconnected.
    pub fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> anyhow::Result<Self> {
        let (sender, receiver) = flume::unbounded();
        command_sender
            .send(PipelineCommand::RegisterClickReceiver(sender.clone()))
            .map_err(|e| anyhow::anyhow!("Failed to register click receiver: {e}"))?;

        let filters: Vec<Box<dyn Filter>> = vec![Box::new(SizeFilter::new(10))];
        let filter_proxys = update_filter_proxys(&filters);

        let state = CounterState {
            is_enabled: false,
            manual_counts: HashMap::new(),
            auto_counts: Vec::new(),
            mode: CounterMode::Manual,
            active_tool: None,
            active_count_type: None,
            review_current_index: 0,
            filter_proxys,
            count_type_visibility: HashMap::new(),
            count_type_names: HashMap::new(),
        };
        Ok(Self {
            state: state.clone(),
            frontend_senders: vec![],
            index: 0,
            filters,
            command_sender,
            sender,
            receiver,
            global_sender,
            cache: crate::tools::cache::ToolCache::new(&state),
            lasso_points: Vec::new(),
            is_lasso_dragging: false,
        })
    }

    fn send_to_frontend(&self, message: &PipelineCommand) -> anyhow::Result<()> {
        for (i, sender) in self.frontend_senders.iter().enumerate() {
            sender
                .send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send message to sender {i}: {e}"))?;
        }
        Ok(())
    }
}
