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
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<CompositorState>,
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
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        self.state.real_layers_len = input.len();

        let images: Vec<image::RgbaImage> = input
            .clone()
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

        *input = vec![composited];

        Ok(())
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let old_state = self.state.clone();

        if let Ok(message) = self.receiver.try_recv() {
            match message {
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    ));
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
                // Invalidate cache when state changes
                self.invalidate_cache();

                if let Err(e) = self
                    .command_sender
                    .send(PipelineCommand::Refresh(self.index))
                {
                    eprintln!("Failed to send refresh command: {}", e);
                }

                // Push state update to frontend
                self.send_to_frontend(PipelineCommand::FrontendMessage(
                    FrontendMessage::ReturnState(self.get_state()),
                ));
            }
        };

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Compositor
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
            frontend_senders: Vec::new(),
            command_sender,
            sender,
            receiver,
            index: 0,
            cached_output: None,
            cached_state: None,
        }
    }

    fn send_to_frontend(&self, message: PipelineCommand) {
        for sender in &self.frontend_senders {
            if let Err(e) = sender.try_send(message.clone()) {
                eprintln!("Failed to send message to frontend: {}", e);
            }
        }
    }
}
