use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, sobel::GpuSobel};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, SobelMessage,
    SobelState, ToolBackend, ToolState,
};

pub struct Sobel {
    pub state: SobelState,
    pub base: ToolBase,
    pub cache: ToolCache<SobelState>,
    processor: GpuSobel,
}

#[async_trait]
impl ToolBackend for Sobel {
    impl_tool_backend_basics!(Sobel);

    fn get_state(&self) -> ToolState {
        ToolState::SobelState(self.state.clone())
    }

    fn replace_state(&mut self, state: ToolState) {
        if let ToolState::SobelState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Sobel
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        let threshold = if self.state.use_threshold {
            Some(self.state.threshold_value)
        } else {
            None
        };

        for image in input {
            *image = match self.processor.apply_sobel(image, threshold).await {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Sobel error: {e}"),
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
        }

        Ok(())
    }
}

impl Sobel {
    #[must_use]
    pub fn new(
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = SobelState {
            is_enabled: false,
            threshold_value: 0.1,
            use_threshold: false,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(global_sender),
            cache: ToolCache::new(&state),
            processor: GpuSobel::new(gpu_image_processor),
        }
    }
}
