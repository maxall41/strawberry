use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{threshold::GpuThreshold, GpuImageProcessor};

use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, ThresholdMessage,
    ToolBackend, ToolState, ThresholdState,
};
use async_trait::async_trait;

pub struct Threshold {
    pub state: ThresholdState,
    pub base: ToolBase,
    pub cache: ToolCache<ThresholdState>,
    threshold: GpuThreshold,
}

#[async_trait]
impl ToolBackend for Threshold {
    impl_tool_backend_basics!(Threshold);

    fn get_state(&self) -> ToolState {
        ToolState::ThresholdState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::ThresholdState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Threshold
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
                    self.base.global_sender.send(PipelineCommand::FrontendMessage(
                        FrontendMessage::error(format!("Threshold error: {e}")),
                    ))?;
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands = self.base.collect_commands();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.base.send_to_frontend(PipelineCommand::FrontendMessage(
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

        if self.cache.frontend_latest_state.as_ref() != Some(&self.state) {
            self.cache.cached_output = None;
            self.cache.cached_state = None;
            self.base.send_to_frontend(PipelineCommand::FrontendMessage(
                FrontendMessage::ReturnState(self.get_state()),
            ))?;
            self.base.trigger_refresh()?;
            self.cache.frontend_latest_state = Some(self.state.clone());
        }

        Ok(())
    }
}

impl Threshold {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: ThresholdState {
                is_enabled: false,
                threshold_value: 0.5,
            },
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            threshold: GpuThreshold::new(gpu_image_processor).await,
        }
    }
}
