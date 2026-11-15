use num_traits::ToPrimitive;
use strawberry_types::RgbaColor;

use crate::nervetracer::io::{
    export_nervetracer_to_xml_file, import_nervetracer_from_xml_file_into,
};
use std::sync::Arc;
use strawberry_processing::nervetracer::create_path;

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, NerveTracerMessage, NerveTracerState, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
    overlay::{OverlayElement, ToolOverlay},
};

/// Trace positions are pixel indices; overlays align with the centers of those
/// pixels. Keep this display offset out of stored and exported path coordinates.
fn pixel_center((x, y): (u64, u64)) -> Option<(f32, f32)> {
    Some((x.to_f32()? + 0.5, y.to_f32()? + 0.5))
}

pub struct NerveTracer {
    pub state: NerveTracerState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cache: crate::tools::cache::ToolCache<NerveTracerState>,
    pub cached_input: Option<Arc<image::RgbaImage>>,
}

#[async_trait]
impl ToolBackend for NerveTracer {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }
    fn persisted_state_mut(&mut self) -> Option<&mut dyn strawberry_types::PersistedState> {
        Some(&mut self.state)
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::NerveTracerState(self.state.clone())
    }
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        self.state.total_images = input.len();

        // Clamp selected_image_index if it's out of bounds
        if self.state.selected_image_index >= input.len() && !input.is_empty() {
            self.state.selected_image_index = input.len().saturating_sub(1);
        }

        // Cache the selected image
        if input.is_empty() {
            self.cached_input = None;
        } else {
            self.cached_input = input.get(self.state.selected_image_index).cloned();
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
                PipelineCommand::HandleClick(x, y) => {
                    if !self.state.is_enabled {
                        return Ok(());
                    }
                    let Some(image) = &self.cached_input else {
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
                                self.state.current_path_segments.push(new_path);
                            }
                        }
                    }
                }
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(&PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(Box::new(self.get_state())),
                    ))?;
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::NerveTracerMessage(message) => match message {
                        NerveTracerMessage::CommitCurrentPathSegments() => {
                            self.state
                                .paths
                                .push(self.state.current_path_segments.clone());
                            self.state.current_path_segments.clear();
                        }
                        NerveTracerMessage::DeletePath(id) => {
                            if id < self.state.paths.len() {
                                self.state.paths.remove(id);
                            }
                        }
                        NerveTracerMessage::DeletePathSegmentInCommitedPath(
                            path_id,
                            segment_id,
                        ) => {
                            if let Some(path) = self.state.paths.get_mut(path_id)
                                && segment_id < path.len()
                            {
                                path.remove(segment_id);
                            }
                        }
                        NerveTracerMessage::DeleteUncomittedPathSegment(id) => {
                            if id < self.state.current_path_segments.len() {
                                self.state.current_path_segments.remove(id);
                            }
                        }
                        NerveTracerMessage::LoadTraces(path) => {
                            match import_nervetracer_from_xml_file_into(&path, &mut self.state) {
                                Ok(()) => {
                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::success(
                                            "Traces loaded successfully!".to_string(),
                                        ),
                                    ))?;
                                }
                                Err(e) => {
                                    eprintln!("Failed to load traces from {}: {e}", path.display());
                                    if let Err(send_err) = self.global_sender.send(
                                        PipelineCommand::FrontendMessage(FrontendMessage::error(
                                            format!("Failed to load traces: {e}"),
                                        )),
                                    ) {
                                        eprintln!("Failed to send error feedback: {send_err}");
                                    }
                                }
                            }
                        }
                        NerveTracerMessage::SaveTraces(path) => {
                            match export_nervetracer_to_xml_file(&self.state, &path, None, None) {
                                Ok(()) => {
                                    if let Err(e) = self.global_sender.send(
                                        PipelineCommand::FrontendMessage(FrontendMessage::success(
                                            format!("Traces saved to {}", path.display()),
                                        )),
                                    ) {
                                        eprintln!("Failed to send save feedback: {e}");
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Failed to save traces to {}: {e}", path.display());
                                    if let Err(send_err) = self.global_sender.send(
                                        PipelineCommand::FrontendMessage(FrontendMessage::error(
                                            format!("Failed to save traces: {e}"),
                                        )),
                                    ) {
                                        eprintln!("Failed to send error feedback: {send_err}");
                                    }
                                }
                            }
                        }
                        NerveTracerMessage::SetSelectedImageIndex(index) => {
                            self.state.selected_image_index = index;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::NerveTracer
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    fn reconcile_state(&mut self) -> bool {
        let (invalidate, process) = self.cache.sync.reconcile(&self.state);
        if invalidate {
            self.invalidate_cache();
        }
        process
    }

    fn acknowledge_processed_state(&mut self) {
        self.cache.sync.acknowledge(&self.state);
    }

    fn valid_cached_output(&self) -> Option<Vec<Arc<image::RgbaImage>>> {
        self.cache.valid_output(&self.state)
    }

    fn set_cached_output(&mut self, output: Vec<Arc<image::RgbaImage>>) {
        self.cache.cached_output = Some(output);
        self.cache.cached_state = Some(self.state.clone());
    }

    fn invalidate_cache(&mut self) {
        self.cache.invalidate();
        self.cached_input = None;
    }

    /// Push state to the frontend without discarding the cached image.
    fn sync_frontend(&mut self) -> anyhow::Result<()> {
        if !self.cache.sync.needs_frontend_sync(&self.state) {
            return Ok(());
        }
        self.send_to_frontend(&PipelineCommand::FrontendMessage(
            FrontendMessage::ReturnState(Box::new(self.get_state())),
        ))?;
        self.cache.sync.frontend_latest_state = Some(self.state.clone());
        self.command_sender
            .send(PipelineCommand::RequestOverlays)
            .map_err(|e| anyhow::anyhow!("Failed to request overlay refresh: {e}"))?;
        Ok(())
    }

    fn get_overlays(&self) -> Option<crate::overlay::ToolOverlay> {
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
        for segment in &self.state.current_path_segments {
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
        for path in &self.state.paths {
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
                tool_id: BackendToolType::NerveTracer,
                z_order: 50,
                elements,
            })
        }
    }
}

impl NerveTracer {
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
        let state = NerveTracerState {
            is_enabled: false,
            paths: vec![],
            current_path_segments: vec![],
            first_point: None,
            selected_image_index: 0,
            total_images: 0,
        };
        Ok(Self {
            state: state.clone(),
            frontend_senders: Vec::new(),
            command_sender,
            global_sender,
            sender,
            receiver,
            index: 0,
            cache: crate::tools::cache::ToolCache::new(&state),
            cached_input: None,
        })
    }

    fn send_to_frontend(&self, message: &PipelineCommand) -> anyhow::Result<()> {
        for sender in &self.frontend_senders {
            sender
                .try_send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send message to frontend: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_markers_and_segments_use_pixel_centers_without_changing_stored_paths()
    -> anyhow::Result<()> {
        let (commands, _commands_rx) = tokio::sync::broadcast::channel(16);
        let (output, _output_rx) = tokio::sync::broadcast::channel(16);
        let mut tracer = NerveTracer::new(commands, output)?;
        tracer.state.is_enabled = true;
        tracer.state.first_point = Some((0, 0));
        tracer.state.current_path_segments = vec![vec![(10, 20), (30, 20)]];
        tracer.state.paths = vec![vec![vec![(5, 6), (7, 8)]]];
        let state = tracer.state.clone();
        let overlay = tracer
            .get_overlays()
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
