use std::sync::Arc;

use crate::processing::{sobel::GpuSobel, GpuImageProcessor};

use super::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, SobelMessage,
    ToolBackend, ToolState,
};
use async_trait::async_trait;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SobelState {
    pub threshold_value: f32,
    pub use_threshold: bool,
    pub is_enabled: bool,
}

pub struct Sobel {
    pub state: SobelState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sobel: GpuSobel,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<SobelState>,
}

#[async_trait]
impl ToolBackend for Sobel {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }
    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::SobelState(new_state) = state {
            self.state = new_state;
        }
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::SobelState(self.state.clone())
    }
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        let threshold = if self.state.use_threshold {
            Some(self.state.threshold_value)
        } else {
            None
        };

        for image in input {
            *image = self.sobel.apply_sobel(image, threshold).await?;
        }
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
                    PipelineObjectMessage::SobelMessage(sobel) => match sobel {
                        SobelMessage::SetThreshold(value) => {
                            self.state.threshold_value = value;
                        }
                        SobelMessage::ToggleThreshold(enabled) => {
                            self.state.use_threshold = enabled;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
            if old_state != self.state {
                self.invalidate_cache();

                self.command_sender
                    .send(PipelineCommand::Refresh(self.index))
                    .map_err(|e| anyhow::anyhow!("Failed to send refresh command: {}", e))?;

                self.send_to_frontend(PipelineCommand::FrontendMessage(
                    FrontendMessage::ReturnState(self.get_state()),
                ))?;
            }
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Sobel
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

impl Sobel {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            state: SobelState {
                is_enabled: false,
                threshold_value: 0.1,
                use_threshold: false,
            },
            sobel: GpuSobel::new(gpu_image_processor).await,
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
