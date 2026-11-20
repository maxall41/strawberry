use crate::tools::base::ToolEvents;
use anyhow::Context as _;
use num_traits::ToPrimitive;
use strawberry_types::RgbaColor;

use std::collections::HashMap;

use crate::counter::fitting::{find_counts_within_polygon, ramer_douglas_peucker_closed};

use crate::counter::io::{export_counter_to_xml_file, read_marker_file};

use crate::utils::binary_image::BinaryImageWrapper;
use async_trait::async_trait;
use image::RgbaImage;
use lutz::{Pixel, lutz};
use strawberry_types::consts::CIRCLE_COLORS;
use strawberry_types::overlay::{CircleStyle, OverlayElement, ToolOverlay};
use strawberry_types::{
    BackendToolType, CountData, CountMessage, CounterMode, CounterState, CounterTool,
    FrontendMessage, ImageCollection, PerPosition, PipelineObjectMessage, Position, Shape,
    ToolBackend, ToolInput, ToolState,
};

pub struct Counter {
    pub state: CounterState,
    pub events: ToolEvents,
    pub cache: crate::tools::cache::ToolCache<CounterState>,
    pub lasso_points: Vec<(u64, u64)>,
    pub is_lasso_dragging: bool,
    overlays_dirty: bool,
    shape: Shape,
}

/// Find the centroid of every connected foreground blob in a binary image.
fn detect_blob_centroids(image: &RgbaImage) -> anyhow::Result<Vec<CountData>> {
    let binary_wrapper = BinaryImageWrapper::new(image, 128); // Use threshold of 128
    //TODO: move to gpu
    lutz::<BinaryImageWrapper, Vec<Pixel>>(binary_wrapper)
        .filter(|blob_pixels| !blob_pixels.is_empty())
        .map(|blob_pixels| {
            let sum_x: u64 = blob_pixels.iter().map(|p| u64::from(p.x)).sum();
            let sum_y: u64 = blob_pixels.iter().map(|p| u64::from(p.y)).sum();
            let count = u64::try_from(blob_pixels.len())?;
            Ok(CountData {
                position: (
                    sum_x.checked_div(count).unwrap_or_default(),
                    sum_y.checked_div(count).unwrap_or_default(),
                ),
                size: u16::try_from(blob_pixels.len()).unwrap_or(u16::MAX),
            })
        })
        .collect()
}

#[async_trait]
impl ToolBackend for Counter {
    /// Return and clear queued frontend events.
    fn drain_events(&mut self) -> Vec<FrontendMessage> {
        self.events.drain()
    }
    /// Expose mutable settings for pipeline persistence.
    fn persisted_state_mut(&mut self) -> Option<&mut dyn strawberry_types::PersistedState> {
        Some(&mut self.state)
    }
    /// Cancel the active canvas interaction.
    fn cancel_interaction(&mut self) {
        self.overlays_dirty |= self.is_lasso_dragging && self.lasso_points.len() > 1;
        self.lasso_points.clear();
        self.is_lasso_dragging = false;
    }
    /// Place, review, and show counts on the given position.
    fn select_position(&mut self, position: Position) {
        self.state.counts.select(position);
        self.state.review_current_index = 0;
    }
    /// Return and clear the pending overlay change flag.
    fn take_overlay_changes(&mut self) -> bool {
        std::mem::take(&mut self.overlays_dirty)
    }
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }

    /// Update whether this tool participates in the pipeline.
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::CounterState(self.state.clone())
    }

    /// Detect connected components at every position when automatic mode is active.
    async fn process(&mut self, images: &mut ImageCollection) -> anyhow::Result<()> {
        self.shape = images.shape();
        if self.state.mode == CounterMode::Auto {
            for position in images.positions() {
                let mut detected = Vec::new();
                for channel in images.channels_at(position) {
                    detected.extend(detect_blob_centroids(channel)?);
                }
                self.state.counts.position_mut(position).auto = detected;
            }
        }

        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    #[expect(
        clippy::too_many_lines,
        reason = "Command dispatch as one function is cleaner"
    )]
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        match message {
            ToolInput::HandleClick(x, y) => {
                if !self.state.is_enabled {
                    return Ok(());
                }
                if let Some(id) = self.state.active_count_type {
                    let counts = self
                        .state
                        .counts
                        .selected_mut()
                        .manual
                        .entry(id)
                        .or_default();
                    match self.state.active_tool {
                        Some(CounterTool::Place) => {
                            counts.push(CountData {
                                position: (x, y),
                                size: 0,
                            });
                        }
                        Some(CounterTool::Delete) => {
                            counts.retain(|count| {
                                let dx = count.position.0.abs_diff(x);
                                let dy = count.position.1.abs_diff(y);
                                // A six-pixel radius; avoid signed conversion and overflow.
                                dx > 6
                                    || dy > 6
                                    || dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
                                        > 36
                            });
                        }
                        _ => {}
                    }
                }
            }
            ToolInput::InteractionStart(x, y) => {
                if !self.state.is_enabled {
                    return Ok(());
                }
                if self.state.active_tool == Some(CounterTool::LassoDelete) {
                    self.cancel_interaction();
                    self.is_lasso_dragging = true;
                    self.lasso_points.push((x, y));
                }
            }
            ToolInput::InteractionMove(x, y) => {
                if self.is_lasso_dragging && self.lasso_points.last() != Some(&(x, y)) {
                    self.lasso_points.push((x, y));
                    self.overlays_dirty = true;
                }
            }
            ToolInput::InteractionEnd => {
                if self.is_lasso_dragging && !self.lasso_points.is_empty() {
                    let polygon = ramer_douglas_peucker_closed(self.lasso_points.clone());
                    self.delete_counts_inside(&polygon);
                    self.cancel_interaction();
                }
            }
            ToolInput::Message(message) => {
                let PipelineObjectMessage::CountMessage(count_message) = message else {
                    return Ok(());
                };
                match count_message {
                    CountMessage::NewCountType(id) => {
                        self.state.count_type_visibility.insert(id, true);
                        self.state.count_type_names.insert(id, format!("Type {id}"));
                        self.state.active_count_type = Some(id);
                    }
                    CountMessage::DeletionLasso(points) => {
                        let polygon = ramer_douglas_peucker_closed(points);
                        self.delete_counts_inside(&polygon);
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
                        for counts in self.state.counts.values_mut() {
                            counts.manual.remove(&id);
                        }
                        self.state.count_type_visibility.remove(&id);
                        self.state.count_type_names.remove(&id);
                        // Never let active_count_type dangle
                        if self.state.active_count_type == Some(id) {
                            self.state.active_count_type =
                                self.state.count_type_names.keys().min().copied();
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
                    CountMessage::SaveCounts(file) => {
                        if let Err(e) = export_counter_to_xml_file(&self.state, self.shape, &file) {
                            self.events.emit(FrontendMessage::error(
                                "Error saving counts: XML export failed".to_string(),
                            ));
                            return Err(anyhow::anyhow!("Error saving counts to file: {e}"));
                        }
                        self.events.emit(FrontendMessage::success(
                            "Counts saved successfully!".to_string(),
                        ));
                    }
                    CountMessage::LoadCounts(file, placement) => match read_marker_file(&file) {
                        Ok(marker_file) => {
                            marker_file.apply_to_counter_state(
                                &mut self.state,
                                placement,
                                self.shape,
                            );
                            self.events.emit(FrontendMessage::success(
                                "Counts loaded successfully!".to_string(),
                            ));
                        }
                        Err(e) => {
                            self.events.emit(FrontendMessage::error(
                                "Error loading counts: XML import failed".to_string(),
                            ));
                            return Err(anyhow::anyhow!("Error loading counts from file: {e}"));
                        }
                    },
                    CountMessage::CopyToManual => {
                        // `len()` collides with existing ids once types have
                        // been deleted; pick the first genuinely free id.
                        let new_key = (0..=u16::MAX)
                            .find(|k| !self.state.count_type_names.contains_key(k))
                            .ok_or_else(|| anyhow::anyhow!("No count type IDs available"))?;
                        // Automatic counting covers every position, so every position's counts are copied.
                        for counts in self.state.counts.values_mut() {
                            counts.manual.insert(new_key, counts.auto.clone());
                        }
                        self.state.count_type_visibility.insert(new_key, true);
                        self.state
                            .count_type_names
                            .insert(new_key, format!("Type {new_key}"));
                        self.state.mode = CounterMode::Manual;
                        self.state.active_count_type = Some(new_key);
                        self.state.review_current_index = 0;
                    }
                    CountMessage::ReviewNavigatePrevious => {
                        let counts_len = self.active_counts().len();
                        if counts_len > 0 {
                            // Clamp: the cursor can be stale when counts were
                            // trimmed outside review (Delete tool, lasso).
                            let current_index = self
                                .state
                                .review_current_index
                                .min(counts_len.saturating_sub(1));
                            self.state.review_current_index = if current_index == 0 {
                                counts_len.saturating_sub(1)
                            } else {
                                current_index.saturating_sub(1)
                            };
                            self.snap_to_reviewed_count()?;
                        }
                    }
                    CountMessage::ReviewNavigateNext => {
                        let counts_len = self.active_counts().len();
                        if counts_len > 0 {
                            // Clamp: the cursor can be stale when counts were
                            // trimmed outside review (Delete tool, lasso).
                            let current_index = self
                                .state
                                .review_current_index
                                .min(counts_len.saturating_sub(1));
                            self.state.review_current_index = (current_index.saturating_add(1))
                                .checked_rem(counts_len)
                                .unwrap_or_default();
                            self.snap_to_reviewed_count()?;
                        }
                    }
                    CountMessage::DeleteCurrentCount => {
                        if let Some(active_count_type) = self.state.active_count_type
                            && let Some(counts) = self
                                .state
                                .counts
                                .selected_mut()
                                .manual
                                .get_mut(&active_count_type)
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

                            self.state.review_current_index =
                                current_index.min(counts.len().saturating_sub(1));
                            // Deleting the last count leaves nothing to snap to.
                            self.snap_to_reviewed_count()?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Counter
    }

    crate::impl_tool_cache!();

    /// Build the canvas overlays for the selected position's counts.
    fn get_overlays(&self, id: strawberry_types::ToolId) -> Option<crate::overlay::ToolOverlay> {
        // Only the overlay is rounded to f32; stored count positions remain exact integers.
        if !self.state.is_enabled {
            return None;
        }

        let mut elements = Vec::new();

        // Manual counts
        let manual_counts = self.state.counts.selected().map(|counts| &counts.manual);
        for (count_type_id, counts) in manual_counts.into_iter().flatten() {
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
                .unwrap_or(CIRCLE_COLORS[0]);

            for (index, count) in counts.iter().enumerate() {
                let is_selected = self.state.active_tool == Some(CounterTool::Review)
                    && self.state.active_count_type == Some(*count_type_id)
                    && index == self.state.review_current_index;

                let circle_color = if is_selected {
                    RgbaColor::new(255, 213, 0, 255) // Hot yellow for selected
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
            let color = CIRCLE_COLORS[0]; // Use first color for auto counts
            for count in self.state.auto_counts() {
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
                    color: RgbaColor::new(255, 0, 0, 255), // Red
                });
            }
        }

        Some(ToolOverlay {
            tool_id: id,
            z_order: 100, // High priority - on top
            elements,
        })
    }
}

impl Counter {
    /// Construct the tool with its initial settings and required resources.
    #[must_use]
    pub fn new(events: ToolEvents) -> Self {
        let state = CounterState {
            is_enabled: false,
            counts: PerPosition::default(),
            mode: CounterMode::Manual,
            active_tool: None,
            active_count_type: None,
            review_current_index: 0,
            count_type_visibility: HashMap::new(),
            count_type_names: HashMap::new(),
        };
        Self {
            state: state.clone(),
            events,
            cache: crate::tools::cache::ToolCache::new(&state),
            lasso_points: Vec::new(),
            is_lasso_dragging: false,
            overlays_dirty: false,
            shape: Shape::default(),
        }
    }

    /// Return the selected position's counts of the active type.
    fn active_counts(&self) -> &[CountData] {
        self.state
            .active_count_type
            .map_or(&[], |id| self.state.manual_counts(id))
    }

    /// Delete the active type's counts on the selected position that lie inside a lasso polygon.
    fn delete_counts_inside(&mut self, polygon: &[(u64, u64)]) {
        if let Some(id) = self.state.active_count_type
            && let Some(counts) = self.state.counts.selected_mut().manual.get_mut(&id)
        {
            *counts = find_counts_within_polygon(polygon, std::mem::take(counts));
        }
    }

    /// Center the viewer on the count under the review cursor, if there is one.
    fn snap_to_reviewed_count(&self) -> anyhow::Result<()> {
        if let Some(count) = self.active_counts().get(self.state.review_current_index) {
            self.events.emit(FrontendMessage::SnapToPosition(
                count
                    .position
                    .0
                    .to_f32()
                    .context("Count coordinate cannot be displayed")?,
                count
                    .position
                    .1
                    .to_f32()
                    .context("Count coordinate cannot be displayed")?,
            ));
        }
        Ok(())
    }
}
