use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, grayscale::GpuGrayscale};

use async_trait::async_trait;
use strawberry_types::{BackendToolType, GrayscaleState, ToolBackend, ToolInput, ToolState};

pub struct Grayscale {
    pub state: GrayscaleState,
    pub base: ToolBase,
    pub cache: ToolCache<GrayscaleState>,
    processor: GpuGrayscale,
}

#[async_trait]
impl ToolBackend for Grayscale {
    impl_tool_backend_basics!(Grayscale);

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::GrayscaleState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Grayscale
    }

    /// Convert each input layer to grayscale.
    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = Arc::new(self.processor.apply_grayscale(image).await?);
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, _message: ToolInput) -> anyhow::Result<()> {
        Ok(())
    }
}

impl Grayscale {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = GrayscaleState { is_enabled: false };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuGrayscale::new(gpu_image_processor).await?,
        })
    }
}
