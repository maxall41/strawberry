use num_traits::ToPrimitive;

use crate::nervetracer::io::{export_nervetracer_to_xml_file, import_nervetracer_from_xml_file};
use std::sync::Arc;
use strawberry_processing::nervetracer::create_path;

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, NerveTracerMessage, NerveTracerState, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
    overlay::{OverlayElement, ToolOverlay},
};

/// Check if statechange requires reprocess.
const fn nervetracer_output_differs(a: &NerveTracerState, b: &NerveTracerState) -> bool {
    a.is_enabled != b.is_enabled || a.selected_image_index != b.selected_image_index
}

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
    pub cached_output: Option<Vec<Arc<image::RgbaImage>>>,
    pub cached_state: Option<NerveTracerState>,
    pub frontend_latest_state: Option<NerveTracerState>,
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
    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::NerveTracerState(new_state) = state {
            self.state = new_state;
        }
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
                        FrontendMessage::ReturnState(self.get_state()),
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
                            match import_nervetracer_from_xml_file(&path) {
                                Ok(loaded_state) => {
                                    self.state = loaded_state;
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

        let (changed, affects_output) = match self.frontend_latest_state.as_ref() {
            Some(previous) => (
                previous != &self.state,
                nervetracer_output_differs(previous, &self.state),
            ),
            None => (true, true),
        };

        if changed {
            if affects_output {
                self.update_state()?;
            } else {
                self.sync_frontend()?;
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

    fn valid_cached_output(&self) -> Option<Vec<Arc<image::RgbaImage>>> {
        if self.cached_state.as_ref() == Some(&self.state) {
            self.cached_output.clone()
        } else {
            None
        }
    }

    fn get_overlays(&self) -> Option<crate::overlay::ToolOverlay> {
        // Only the overlay is rounded to f32; stored trace positions remain exact integers.
        if !self.state.is_enabled {
            return None;
        }

        let mut elements = Vec::new();

        // Color for uncommitted segments (orange/yellow)
        let uncommitted_color = [255, 200, 0, 255];
        // Color for committed paths (cyan/blue)
        let committed_color = [0, 200, 255, 255];
        // Color for current points (bright red/pink)
        let current_point_color = [255, 50, 100, 255];
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

    fn set_cached_output(&mut self, output: Vec<Arc<image::RgbaImage>>) {
        self.cached_output = Some(output);
        self.cached_state = Some(self.state.clone());
    }

    fn invalidate_cache(&mut self) {
        self.cached_output = None;
        self.cached_state = None;
        self.cached_input = None;
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
        Ok(Self {
            state: NerveTracerState {
                is_enabled: false,
                paths: vec![],
                current_path_segments: vec![],
                first_point: None,
                selected_image_index: 0,
                total_images: 0,
            },
            frontend_senders: Vec::new(),
            command_sender,
            global_sender,
            sender,
            receiver,
            index: 0,
            cached_output: None,
            cached_state: None,
            frontend_latest_state: None,
            cached_input: None,
        })
    }

    /// Push state to the frontend without discarding the cached image.
    fn sync_frontend(&mut self) -> anyhow::Result<()> {
        self.send_to_frontend(&PipelineCommand::FrontendMessage(
            FrontendMessage::ReturnState(self.get_state()),
        ))?;
        self.frontend_latest_state = Some(self.state.clone());
        self.command_sender
            .send(PipelineCommand::RequestOverlays)
            .map_err(|e| anyhow::anyhow!("Failed to request overlay refresh: {e}"))?;
        Ok(())
    }

    fn update_state(&mut self) -> anyhow::Result<()> {
        self.invalidate_cache();
        self.sync_frontend()?;
        self.command_sender
            .send(PipelineCommand::Refresh(self.index))
            .map_err(|e| anyhow::anyhow!("Failed to send refresh command: {e}"))?;
        Ok(())
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
                color: [255, 50, 100, 255],
            },
            OverlayElement::Polyline {
                points: vec![(10.5, 20.5), (30.5, 20.5)],
                color: [255, 200, 0, 255],
                stroke_width: 2.0,
            },
            OverlayElement::Polyline {
                points: vec![(5.5, 6.5), (7.5, 8.5)],
                color: [0, 200, 255, 255],
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
