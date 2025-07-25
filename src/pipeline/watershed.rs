use std::sync::Arc;

use crate::processing::{
    watershed::{GpuWatershed, WatershedConfig},
    GpuImageProcessor,
};

use super::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, ToolBackend,
    ToolState, WatershedMessage,
};
use async_trait::async_trait;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatershedState {
    pub sigma: f32,
    pub min_distance: u32,
    pub is_enabled: bool,
}

pub struct Watershed {
    pub state: WatershedState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub watershed: GpuWatershed,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<WatershedState>,
}

#[async_trait]
impl ToolBackend for Watershed {
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
        ToolState::WatershedState(self.state.clone())
    }
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        for image in input {
            *image = self.watershed.apply_watershed(image).await?;
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
                    ));
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::WatershedMessage(message) => match message {
                        WatershedMessage::SetParameters(sigma, min_distance) => {
                            self.state.sigma = sigma;
                            self.state.min_distance = min_distance;
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
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Watershed
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

impl Watershed {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            state: WatershedState {
                is_enabled: false,
                sigma: 1.0,
                min_distance: 10,
            },
            watershed: GpuWatershed::new(
                WatershedConfig {
                    sigma: 1.0,
                    min_distance: 10,
                },
                gpu_image_processor,
            )
            .await,
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
