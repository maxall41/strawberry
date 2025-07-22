use std::sync::Arc;

use crate::processing::{compositor::GpuCompositor, GpuImageProcessor};

use super::{
    BackendToolType, CompositorMessage, FrontendMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};
use async_trait::async_trait;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompositorState {
    pub is_enabled: bool,
    pub enabled_layers: Vec<bool>,
    pub real_layers_len: usize,
}

pub struct Compositor {
    pub state: CompositorState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub compositer: GpuCompositor,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_sender: Option<flume::Sender<PipelineCommand>>,
}

#[async_trait]
impl ToolBackend for Compositor {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::CompositorState(self.state.clone())
    }
    fn set_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_sender = Some(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_sender = None;
    }
    async fn process(
        &mut self,
        input: Vec<image::RgbaImage>,
    ) -> anyhow::Result<Vec<image::RgbaImage>> {
        self.state.real_layers_len = input.len();

        let images: Vec<image::RgbaImage> = input
            .into_iter()
            .enumerate()
            .filter_map(|(i, image)| {
                if self.state.enabled_layers[i] {
                    Some(image)
                } else {
                    None
                }
            })
            .collect();

        let composited = self.compositer.composite_images(&images).await?;

        Ok(vec![composited])
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let old_state = self.state.clone();

        if let Ok(message) = self.receiver.try_recv() {
            match message {
                PipelineCommand::RequestState() => {
                    if let Some(sender) = &self.frontend_sender {
                        if let Err(e) = sender.try_send(PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnState(self.get_state()),
                        )) {
                            eprintln!("Failed to send state to frontend: {}", e);
                        }
                    } else {
                        eprintln!("No frontend sender available for state request");
                    }
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::CompositorMessage(message) => match message {
                        CompositorMessage::ToggleVisibility(value, layer_index) => {
                            let mut mutated = self.state.enabled_layers.clone();
                            mutated[layer_index as usize] = value;

                            if mutated[0..self.state.real_layers_len].iter().all(|&x| !x) {
                                return Ok(());
                            }

                            self.state.enabled_layers = mutated;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
            if old_state != self.state {
                if let Err(e) = self.command_sender.send(PipelineCommand::Refresh(0)) {
                    eprintln!("Failed to send refresh command: {}", e);
                }

                // Push state update to frontend
                if let Some(frontend_sender) = &self.frontend_sender {
                    if let Err(e) = frontend_sender.try_send(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    )) {
                        eprintln!("Failed to send state update to frontend: {}", e);
                    }
                }
            }
        };

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Compositor
    }
}

impl Compositor {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Compositor {
            state: CompositorState {
                is_enabled: false,
                enabled_layers: vec![true; 8], //TODO: Mmmm danger. Should be fine since buffer only supports 7 input images anyways...
                real_layers_len: 0,
            },
            compositer: GpuCompositor::new(gpu_image_processor).await,
            frontend_sender: None,
            command_sender,
            sender,
            receiver,
        }
    }
}
