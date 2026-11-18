use crate::tools::base::ToolEvents;
use num_traits::ToPrimitive;
use strawberry_types::RgbaColor;

use crate::nervetracer::io::{
    export_nervetracer_to_xml_file, import_nervetracer_from_xml_file_into,
};
use std::sync::Arc;
use strawberry_processing::nervetracer::create_path;

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, ImageCollection, NerveTracerMessage, NerveTracerState,
    PipelineObjectMessage, ToolBackend, ToolInput, ToolState,
    overlay::{OverlayElement, ToolOverlay},
};

/// Trace positions are pixel indices; overlays align with the centers of those
/// pixels. Keep this display offset out of stored and exported path coordinates.
fn pixel_center((x, y): (u64, u64)) -> Option<(f32, f32)> {
    Some((x.to_f32()? + 0.5, y.to_f32()? + 0.5))
}

pub struct NerveTracer {
    pub state: NerveTracerState,
    pub events: ToolEvents,
    pub cache: crate::tools::cache::ToolCache<NerveTracerState>,
    /// The selected layer of each stack, traced when the user clicks on that stack.
    stack_inputs: Vec<Option<Arc<image::RgbaImage>>>,
}

#[async_trait]
impl ToolBackend for NerveTracer {
    /// Return and clear queued frontend events.
    fn drain_events(&mut self) -> Vec<FrontendMessage> {
        self.events.drain()
    }
    /// Cancel the active canvas interaction.
    fn cancel_interaction(&mut self) {
        self.state.first_point = None;
    }
    /// Trace on, and show the paths of, the given stack.
    fn select_stack(&mut self, stack: usize) {
        self.state.traces.select(stack);
    }
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }

    /// Expose mutable settings for pipeline persistence.
    fn persisted_state_mut(&mut self) -> Option<&mut dyn strawberry_types::PersistedState> {
        Some(&mut self.state)
    }
    /// Update whether this tool participates in the pipeline.
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::NerveTracerState(self.state.clone())
    }

    /// Cache each stack's selected layer for tracing and refresh the available layer count.
    async fn process(&mut self, images: &mut ImageCollection) -> anyhow::Result<()> {
        self.state.total_layers = images.layer_count();
        if self.state.selected_layer >= self.state.total_layers && self.state.total_layers > 0 {
            self.state.selected_layer = self.state.total_layers.saturating_sub(1);
        }
        self.stack_inputs = images.collect_layer(self.state.selected_layer);
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        match message {
            ToolInput::HandleClick(x, y) => {
                if !self.state.is_enabled {
                    return Ok(());
                }
                let Some(image) = self
                    .stack_inputs
                    .get(self.state.traces.selected_index())
                    .and_then(Option::as_ref)
                else {
                    return Ok(());
                };

                match self.state.first_point {
                    None => {
                        // First click - store the point
                        self.state.first_point = Some((x, y));
                    }
                    Some(first) => {
                        // Second click - create path segment
                        let new_path = create_path(first, (x, y), image);
                        self.state.first_point = None;
                        if let Some(new_path) = new_path {
                            self.state
                                .traces
                                .selected_mut()
                                .current_path_segments
                                .push(new_path);
                        }
                    }
                }
            }

            ToolInput::Message(PipelineObjectMessage::NerveTracerMessage(message)) => match message
            {
                NerveTracerMessage::CommitCurrentPathSegments() => {
                    let traces = self.state.traces.selected_mut();
                    let segments = std::mem::take(&mut traces.current_path_segments);
                    traces.paths.push(segments);
                }
                NerveTracerMessage::DeletePath(id) => {
                    let paths = &mut self.state.traces.selected_mut().paths;
                    if id < paths.len() {
                        paths.remove(id);
                    }
                }
                NerveTracerMessage::DeletePathSegmentInCommitedPath(path_id, segment_id) => {
                    if let Some(path) = self.state.traces.selected_mut().paths.get_mut(path_id)
                        && segment_id < path.len()
                    {
                        path.remove(segment_id);
                    }
                }
                NerveTracerMessage::DeleteUncomittedPathSegment(id) => {
                    let segments = &mut self.state.traces.selected_mut().current_path_segments;
                    if id < segments.len() {
                        segments.remove(id);
                    }
                }
                NerveTracerMessage::LoadTraces(path, placement) => {
                    match import_nervetracer_from_xml_file_into(&path, &mut self.state, placement) {
                        Ok(()) => {
                            self.events.emit(FrontendMessage::success(
                                "Traces loaded successfully!".to_string(),
                            ));
                        }
                        Err(e) => {
                            eprintln!("Failed to load traces from {}: {e}", path.display());
                            self.events.emit(FrontendMessage::error(format!(
                                "Failed to load traces: {e}"
                            )));
                        }
                    }
                }
                NerveTracerMessage::SaveTraces(path) => {
                    match export_nervetracer_to_xml_file(&self.state, &path, None, None) {
                        Ok(()) => {
                            self.events.emit(FrontendMessage::success(format!(
                                "Traces saved to {}",
                                path.display()
                            )));
                        }
                        Err(e) => {
                            eprintln!("Failed to save traces to {}: {e}", path.display());
                            self.events.emit(FrontendMessage::error(format!(
                                "Failed to save traces: {e}"
                            )));
                        }
                    }
                }
                NerveTracerMessage::SelectLayer(index) => {
                    self.state.selected_layer = index;
                }
            },
            _ => {}
        }

        Ok(())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::NerveTracer
    }

    /// Reconcile settings and report whether processing is needed.
    fn reconcile_state(&mut self) -> bool {
        let (invalidate, process) = self.cache.sync.reconcile(&self.state);
        if invalidate {
            self.invalidate_cache();
        }
        process
    }

    /// Record the settings acknowledged by processing.
    fn acknowledge_processed_state(&mut self) {
        self.cache.sync.acknowledge(&self.state);
    }

    /// Return cached output when its processing settings still match.
    fn valid_cached_output(&self) -> Option<ImageCollection> {
        self.cache.valid_output(&self.state)
    }

    /// Cache processed images together with their settings.
    fn set_cached_output(&mut self, output: ImageCollection) {
        self.cache.cached_output = Some(output);
        self.cache.cached_state = Some(self.state.clone());
    }

    /// Discard cached output while preserving input needed for interaction.
    fn invalidate_cached_output(&mut self) {
        self.cache.invalidate();
    }

    /// Discard cached processing data.
    fn invalidate_cache(&mut self) {
        self.invalidate_cached_output();
        self.stack_inputs.clear();
    }

    /// Build the canvas overlays for this tool.
    fn get_overlays(&self, id: strawberry_types::ToolId) -> Option<crate::overlay::ToolOverlay> {
        // Only the overlay is rounded to f32; stored trace positions remain exact integers.
        if !self.state.is_enabled {
            return None;
        }

        let mut elements = Vec::new();

        // Color for uncommitted segments (orange/yellow)
        let uncommitted_color = RgbaColor::new(255, 200, 0, 255);
        // Color for committed paths (cyan/blue)
        let committed_color = RgbaColor::new(0, 200, 255, 255);
        // Color for current points (bright red/pink)
        let current_point_color = RgbaColor::new(255, 50, 100, 255);
        let stroke_width = 2.0;
        let point_radius = 2.0;

        // Draw first point if waiting for second click
        if let Some((x, y)) = self.state.first_point {
            let (x, y) = pixel_center((x, y))?;
            elements.push(OverlayElement::Point {
                x,
                y,
                radius: point_radius,
                color: current_point_color,
            });
        }

        // Draw uncommitted segments
        for segment in self.state.current_path_segments() {
            let points: Vec<(f32, f32)> = segment
                .iter()
                .copied()
                .map(pixel_center)
                .collect::<Option<Vec<_>>>()?;

            if points.len() >= 2 {
                elements.push(OverlayElement::Polyline {
                    points,
                    color: uncommitted_color,
                    stroke_width,
                });
            }
        }

        // Draw committed paths
        for path in self.state.paths() {
            for segment in path {
                let points: Vec<(f32, f32)> = segment
                    .iter()
                    .copied()
                    .map(pixel_center)
                    .collect::<Option<Vec<_>>>()?;

                if points.len() >= 2 {
                    elements.push(OverlayElement::Polyline {
                        points,
                        color: committed_color,
                        stroke_width,
                    });
                }
            }
        }

        if elements.is_empty() {
            None
        } else {
            Some(ToolOverlay {
                tool_id: id,
                z_order: 50,
                elements,
            })
        }
    }
}

impl NerveTracer {
    /// Construct the tool with its initial settings and required resources.
    #[must_use]
    pub fn new(events: ToolEvents) -> Self {
        let state = NerveTracerState::default();
        Self {
            cache: crate::tools::cache::ToolCache::new(&state),
            state,
            events,
            stack_inputs: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strawberry_types::StackTraces;

    /// Verify trace overlays use pixel centers without modifying stored paths.
    #[test]
    fn trace_markers_and_segments_use_pixel_centers_without_changing_stored_paths()
    -> anyhow::Result<()> {
        let mut tracer = NerveTracer::new(ToolEvents::default());
        tracer.state.is_enabled = true;
        tracer.state.first_point = Some((0, 0));
        *tracer.state.traces.selected_mut() = StackTraces {
            paths: vec![vec![vec![(5, 6), (7, 8)]]],
            current_path_segments: vec![vec![(10, 20), (30, 20)]],
        };
        let state = tracer.state.clone();
        let overlay = tracer
            .get_overlays(strawberry_types::ToolId(1))
            .ok_or_else(|| anyhow::anyhow!("Trace overlay missing"))?;
        let expected = vec![
            OverlayElement::Point {
                x: 0.5,
                y: 0.5,
                radius: 2.0,
                color: RgbaColor::new(255, 50, 100, 255),
            },
            OverlayElement::Polyline {
                points: vec![(10.5, 20.5), (30.5, 20.5)],
                color: RgbaColor::new(255, 200, 0, 255),
                stroke_width: 2.0,
            },
            OverlayElement::Polyline {
                points: vec![(5.5, 6.5), (7.5, 8.5)],
                color: RgbaColor::new(0, 200, 255, 255),
                stroke_width: 2.0,
            },
        ];
        anyhow::ensure!(
            overlay.elements == expected,
            "Trace overlays are not centered on their pixels"
        );
        anyhow::ensure!(
            tracer.state == state,
            "Rendering changed stored trace coordinates"
        );
        Ok(())
    }
}
