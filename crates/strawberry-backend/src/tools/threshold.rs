use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, threshold::GpuThreshold};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, ThresholdMessage,
    ThresholdState, ToolBackend, ToolState,
};

pub struct Threshold {
    pub state: ThresholdState,
    pub base: ToolBase,
    pub cache: ToolCache<ThresholdState>,
    processor: GpuThreshold,
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

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .processor
                .apply_threshold(image, self.state.threshold_value)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Threshold error: {e}"),
                        )))?;
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands = self.base.collect_commands();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.base
                        .send_to_frontend(&PipelineCommand::FrontendMessage(
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

        Ok(())
    }
}

impl Threshold {
    #[must_use]
    pub fn new(
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = ThresholdState {
            is_enabled: false,
            threshold_value: 0.5,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(global_sender),
            cache: ToolCache::new(&state),
            processor: GpuThreshold::new(gpu_image_processor),
        }
    }
}
