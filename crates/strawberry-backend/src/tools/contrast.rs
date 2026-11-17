use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, contrast::GpuContrast};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, ContrastMessage, ContrastState, PipelineObjectMessage, ToolBackend, ToolInput,
    ToolState,
};

pub struct Contrast {
    pub state: ContrastState,
    pub base: ToolBase,
    pub cache: ToolCache<ContrastState>,
    processor: GpuContrast,
}

#[async_trait]
impl ToolBackend for Contrast {
    impl_tool_backend_basics!(Contrast);

    fn get_state(&self) -> ToolState {
        ToolState::ContrastState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Contrast
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = Arc::new(
                self.processor
                    .apply_contrast(image, self.state.min, self.state.max)
                    .await?,
            );
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::ContrastMessage(message)) = message {
            match message {
                ContrastMessage::SetMinMax(min, max) => {
                    anyhow::ensure!(
                        min.is_finite() && max.is_finite() && min <= max,
                        "Invalid contrast range"
                    );
                    self.state.min = min;
                    self.state.max = max;
                }
            }
        }

        Ok(())
    }
}

impl Contrast {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = ContrastState {
            is_enabled: false,
            min: 0.0,
            max: 1.0,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuContrast::new(gpu_image_processor).await?,
        })
    }
}
