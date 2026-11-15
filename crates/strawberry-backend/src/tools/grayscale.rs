use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, grayscale::GpuGrayscale};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, GrayscaleState, ToolBackend, ToolInput, ToolState,
};

pub struct Grayscale {
    pub state: GrayscaleState,
    pub base: ToolBase,
    pub cache: ToolCache<GrayscaleState>,
    processor: GpuGrayscale,
}

#[async_trait]
impl ToolBackend for Grayscale {
    impl_tool_backend_basics!(Grayscale);

    fn get_state(&self) -> ToolState {
        ToolState::GrayscaleState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Grayscale
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self.processor.apply_grayscale(image).await {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .events
                        .emit(FrontendMessage::error(format!("Grayscale error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, _message: ToolInput) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Grayscale {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = GrayscaleState { is_enabled: false };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuGrayscale::new(gpu_image_processor),
        }
    }
}
