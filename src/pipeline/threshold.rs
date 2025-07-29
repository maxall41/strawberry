use std::sync::Arc;

use crate::processing::{threshold::GpuThreshold, GpuImageProcessor};

use super::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, ThresholdMessage,
    ToolBackend, ToolState,
};
use async_trait::async_trait;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThresholdState {
    pub threshold_value: f32,
    pub is_enabled: bool,
}

pub struct Threshold {
    pub state: ThresholdState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub threshold: GpuThreshold,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<ThresholdState>,
    pub frontend_latest_state: Option<ThresholdState>,
}

#[async_trait]
impl ToolBackend for Threshold {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }
    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::ThresholdState(new_state) = state {
            self.state = new_state;
        }
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::ThresholdState(self.state.clone())
    }
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .threshold
                .apply_threshold(image, self.state.threshold_value)
                .await
            {
                Ok(img) => img,
                Err(e) => {
                    self.global_sender.send(PipelineCommand::FrontendMessage(
                        FrontendMessage::UserFeedback(format!("Threshold error: {e}")),
                    ))?;
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
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
                    PipelineObjectMessage::ThresholdMessage(threshold) => match threshold {
                        ThresholdMessage::SetThresholdValue(value) => {
                            self.state.threshold_value = value;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        if self.frontend_latest_state.is_none() {
            self.update_state()?;
        } else if self
            .frontend_latest_state
            .as_ref()
            .expect("Just checked that frontend_latest_state is some")
            != &self.state
        {
            self.update_state()?;
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Threshold
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

impl Threshold {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            state: ThresholdState {
                is_enabled: false,
                threshold_value: 0.5,
            },
            threshold: GpuThreshold::new(gpu_image_processor).await,
            frontend_senders: Vec::new(),
            command_sender,
            global_sender,
            sender,
            receiver,
            index: 0,
            cached_output: None,
            cached_state: None,
            frontend_latest_state: None,
        }
    }

    fn update_state(&mut self) -> anyhow::Result<()> {
        self.invalidate_cache();
        self.send_to_frontend(PipelineCommand::FrontendMessage(
            FrontendMessage::ReturnState(self.get_state()),
        ))?;
        self.command_sender
            .send(PipelineCommand::Refresh(self.index))
            .map_err(|e| anyhow::anyhow!("Failed to send refresh command: {}", e))?;
        self.frontend_latest_state = Some(self.state.clone());
        Ok(())
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
