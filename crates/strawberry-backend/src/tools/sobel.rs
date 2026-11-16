use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, sobel::GpuSobel};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineObjectMessage, SobelMessage, SobelState, ToolBackend,
    ToolInput, ToolState,
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
                        .events
                        .emit(FrontendMessage::error(format!("Sobel error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::SobelMessage(sobel)) = message {
            match sobel {
                SobelMessage::SetThreshold(value) => {
                    anyhow::ensure!(
                        value.is_finite() && (0.0..=1.0).contains(&value),
                        "Invalid threshold"
                    );
                    self.state.threshold_value = value;
                }
                SobelMessage::ToggleThreshold(enabled) => {
                    self.state.use_threshold = enabled;
                }
            }
        }

        Ok(())
    }
}

impl Sobel {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = SobelState {
            is_enabled: false,
            threshold_value: 0.1,
            use_threshold: false,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuSobel::new(gpu_image_processor),
        }
    }
}
