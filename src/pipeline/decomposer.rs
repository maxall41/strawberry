use std::sync::Arc;

use crate::processing::{compositor::GpuCompositor, GpuImageProcessor};

use super::{
    BackendToolType, DecomposerMessage, FrontendMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};
use crate::processing::decomposer::GpuDecomposer;
use async_trait::async_trait;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DecomposerState {
    pub is_enabled: bool,
    pub enabled_layers: Vec<bool>,
}

pub struct Decomposer {
    pub state: DecomposerState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub compositer: GpuCompositor,
    pub gpu_decomposer: GpuDecomposer,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<DecomposerState>,
}

#[async_trait]
impl ToolBackend for Decomposer {
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
        if let ToolState::DecomposerState(new_state) = state {
            self.state = new_state;
        }
    }
    fn get_state(&self) -> ToolState {
        ToolState::DecomposerState(self.state.clone())
    }
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        if input.len() > 1 {
            return Err(anyhow::anyhow!(
                "Decomposer can only process one image, received {}",
                input.len()
            ));
        }

        let first_image = input
            .get(0)
            .ok_or_else(|| anyhow::anyhow!("No input image provided to decomposer"))?;
        let decomposed = self.gpu_decomposer.apply_decomposition(first_image).await?;
        let images: Vec<image::RgbaImage> = decomposed
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
                    ))?;
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::DecomposerMessage(message) => match message {
                        DecomposerMessage::ToggleVisibility(value, layer_index) => {
                            let mut mutated = self.state.enabled_layers.clone();
                            let index = layer_index as usize;

                            if index >= mutated.len() {
                                return Err(anyhow::anyhow!(
                                    "Layer index {} out of bounds (max: {})",
                                    index,
                                    mutated.len()
                                ));
                            }

                            mutated[index] = value;
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
        BackendToolType::Decomposer
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

impl Decomposer {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Decomposer {
            state: DecomposerState {
                is_enabled: false,
                enabled_layers: vec![true; 3],
            },
            compositer: GpuCompositor::new(gpu_image_processor.clone()).await,
            gpu_decomposer: GpuDecomposer::new(gpu_image_processor).await,
            frontend_senders: Vec::new(),
            command_sender,
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
