use std::sync::Arc;

use crate::processing::{
    local_maxima::{GpuLocalMaxima, ThresholdType},
    GpuImageProcessor,
};

use super::{
    BackendToolType, FrontendMessage, LocalMaximaMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq)]
pub struct LocalMaximaState {
    pub min_distance: u32,
    pub threshold: ThresholdType,
    pub exclude_border: bool,
    pub is_enabled: bool,
}

impl Default for LocalMaximaState {
    fn default() -> Self {
        Self {
            min_distance: 5,
            threshold: ThresholdType::Relative(0.01),
            exclude_border: true,
            is_enabled: false,
        }
    }
}

pub struct LocalMaxima {
    pub state: LocalMaximaState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub local_maxima: GpuLocalMaxima,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_sender: Option<flume::Sender<PipelineCommand>>,
    pub index: usize,
}

#[async_trait]
impl ToolBackend for LocalMaxima {
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
        ToolState::LocalMaximaState(self.state.clone())
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
        let mut output = Vec::new();
        for image in input {
            let output_image = self
                .local_maxima
                .apply_local_maxima(
                    &image,
                    self.state.min_distance,
                    self.state.threshold.clone(),
                    self.state.exclude_border,
                )
                .await?;
            output.push(output_image);
        }
        Ok(output)
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
                    PipelineObjectMessage::LocalMaximaMessage(message) => match message {
                        LocalMaximaMessage::SetParameters(
                            min_distance,
                            threshold,
                            exclude_border,
                        ) => {
                            self.state.min_distance = min_distance;
                            self.state.threshold = threshold;
                            self.state.exclude_border = exclude_border;
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
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::LocalMaxima
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
    }
}

impl LocalMaxima {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            state: LocalMaximaState::default(),
            local_maxima: GpuLocalMaxima::new(gpu_image_processor).await,
            frontend_sender: None,
            command_sender,
            sender,
            receiver,
            index: 0,
        }
    }
}
