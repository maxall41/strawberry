use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, gaussian_blur::GpuGaussianBlur};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, GaussianBlurMessage, GaussianBlurState, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};

pub struct GaussianBlur {
    pub state: GaussianBlurState,
    pub base: ToolBase,
    pub cache: ToolCache<GaussianBlurState>,
    processor: GpuGaussianBlur,
}

#[async_trait]
impl ToolBackend for GaussianBlur {
    impl_tool_backend_basics!(GaussianBlur);

    fn get_state(&self) -> ToolState {
        ToolState::GaussianBlurState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::GaussianBlurState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::GaussianBlur
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self.processor.apply_blur(image, self.state.sigma).await {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Gaussian Blur error: {e}"),
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
                    PipelineObjectMessage::GaussianBlurMessage(message) => match message {
                        GaussianBlurMessage::SetSigma(sigma) => {
                            self.state.sigma = sigma;
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

impl GaussianBlur {
    #[must_use]
    pub fn new(
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = GaussianBlurState {
            is_enabled: false,
            sigma: 2.0,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(global_sender),
            cache: ToolCache::new(&state),
            processor: GpuGaussianBlur::new(gpu_image_processor),
        }
    }
}
