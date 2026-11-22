use crate::tools::{base::ToolEvents, cache::ToolCache};
use num_traits::ToPrimitive;
use strawberry_types::RgbaColor;

use crate::nervetracer::io::{
    export_nervetracer_to_xml_file, import_nervetracer_from_xml_file_into,
};
use strawberry_processing::nervetracer::create_path;

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, ImageCollection, NerveTracerMessage, NerveTracerState,
    PipelineObjectMessage, Position, ReportPlaneProgress, ToolBackend, ToolInput, ToolState,
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
    pub cache: ToolCache<NerveTracerState>,
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
    /// Drop the previous images' traces, which may lie on slices the new images lack.
    fn release_previous_input(&mut self) {
        self.state.traces.clear();
    }
    /// Trace on, and show the paths of, the given position.
    fn select_position(&mut self, position: Position) {
        self.state.traces.select(position);
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

    /// Remember the images for tracing and refresh the available layer count.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        _report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        self.state.shape = images.shape();
        let layers = self.state.shape.c;
        if self.state.selected_layer >= layers && layers > 0 {
            self.state.selected_layer = layers.saturating_sub(1);
        }
        self.cache.remember_input(images);
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        match message {
            ToolInput::HandleClick(x, y) => self.trace_to_point((x, y)),

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

    crate::impl_tool_cache!();

    /// Discard cached output while keeping the remembered input, so clicks still trace.
    fn invalidate_cached_output(&mut self) {
        self.cache.invalidate();
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
        }
    }

    /// Start a path segment at `point`, or finish the pending one there by tracing the selected
    /// layer of the viewed position.
    fn trace_to_point(&mut self, point: (u64, u64)) {
        if !self.state.is_enabled {
            return;
        }
        let coordinates = self
            .state
            .traces
            .selected_position()
            .with_channel(self.state.selected_layer);
        let Some(image) = self
            .cache
            .get_input()
            .and_then(|input| input.plane(coordinates))
        else {
            return;
        };
        let Some(first) = self.state.first_point.take() else {
            self.state.first_point = Some(point);
            return;
        };
        if let Some(segment) = create_path(first, point, image) {
            self.state
                .traces
                .selected_mut()
                .current_path_segments
                .push(segment);
        }
    }
}
