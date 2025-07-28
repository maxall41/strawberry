use std::sync::Arc;

use crate::processing::{compositor::GpuCompositor, GpuImageProcessor};

use super::{
    BackendToolType, CompositorMessage, FrontendMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};
use async_trait::async_trait;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompositorState {
    pub is_enabled: bool,
    pub enabled_layers: Vec<bool>,
    pub real_layers_len: usize,
}

pub struct Compositor {
    pub state: CompositorState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
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
    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::CompositorState(new_state) = state {
            self.state = new_state;
        }
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

        if input.len() == 1 {
            self.global_sender.send(PipelineCommand::FrontendMessage(
                FrontendMessage::UserFeedback("Compositor needs >1 image to composite".to_string()),
            ))?;
            self.set_enabled(false);
            self.send_to_frontend(PipelineCommand::FrontendMessage(
                FrontendMessage::ReturnState(self.get_state()),
            ))?;
            return Ok(());
        }

        let images: Vec<image::RgbaImage> = input
            .clone()
            .into_iter()
            .enumerate()
            .filter_map(|(i, image)| {
                if self.state.enabled_layers.get(i).copied().unwrap_or(false) {
                    Some(image)
                } else {
                    None
                }
            })
            .collect();

        let composited = match self.compositer.composite_images(&images).await {
            Ok(image) => image,
            Err(e) => {
                self.global_sender.send(PipelineCommand::FrontendMessage(
                    FrontendMessage::UserFeedback(format!("Compositor error: {e}")),
                ))?;
                return Err(e);
            }
        };

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
                    ))?;
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::CompositorMessage(message) => match message {
                        CompositorMessage::ToggleVisibility(value, layer_index) => {
                            let mut mutated = self.state.enabled_layers.clone();
                            let index = layer_index as usize;

                            if index >= mutated.len() {
                                let error_msg = format!(
                                    "Compositor: Layer index {} out of bounds (max: {})",
                                    index,
                                    mutated.len()
                                );
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::UserFeedback(error_msg.clone()),
                                ))?;
                                return Err(anyhow::anyhow!(error_msg));
                            }

                            mutated[index] = value;

                            let end_range =
                                std::cmp::min(self.state.real_layers_len, mutated.len());
                            if mutated
                                .get(0..end_range)
                                .map(|slice| slice.iter().all(|&x| !x))
                                .unwrap_or(true)
                            {
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

                self.command_sender
                    .send(PipelineCommand::Refresh(self.index))
                    .map_err(|e| anyhow::anyhow!("Failed to send refresh command: {}", e))?;

                // Push state update to frontend
                self.send_to_frontend(PipelineCommand::FrontendMessage(
                    FrontendMessage::ReturnState(self.get_state()),
                ))?;
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
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
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
            global_sender,
            sender,
            receiver,
            index: 0,
            cached_output: None,
            cached_state: None,
        }
    }

    fn send_to_frontend(&self, message: PipelineCommand) -> anyhow::Result<()> {
        for sender in &self.frontend_senders {
            sender
                .try_send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send message to frontend: {}", e))?;
        }
        Ok(())
    }
}
