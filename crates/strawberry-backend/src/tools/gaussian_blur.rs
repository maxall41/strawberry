use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, gaussian_blur::GpuGaussianBlur};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, GaussianBlurMessage, GaussianBlurState,
    PipelineObjectMessage, ToolBackend, ToolInput, ToolState,
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

    fn iam(&self) -> BackendToolType {
        BackendToolType::GaussianBlur
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self.processor.apply_blur(image, self.state.sigma).await {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .events
                        .emit(FrontendMessage::error(format!("Gaussian Blur error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        match message {
            ToolInput::Message(message) => match message {
                PipelineObjectMessage::GaussianBlurMessage(message) => match message {
                    GaussianBlurMessage::SetSigma(sigma) => {
                        anyhow::ensure!(sigma.is_finite() && sigma > 0., "Sigma must be positive");
                        self.state.sigma = sigma;
                    }
                },
                _ => {}
            },
            _ => {}
        }

        Ok(())
    }
}

impl GaussianBlur {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = GaussianBlurState {
            is_enabled: false,
            sigma: 2.0,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuGaussianBlur::new(gpu_image_processor),
        }
    }
}
