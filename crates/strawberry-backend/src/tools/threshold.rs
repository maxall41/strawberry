use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, threshold::GpuThreshold};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineObjectMessage, ThresholdMessage, ThresholdState,
    ToolBackend, ToolInput, ToolState,
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
                        .events
                        .emit(FrontendMessage::error(format!("Threshold error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::ThresholdMessage(threshold)) = message {
            match threshold {
                ThresholdMessage::SetThresholdValue(value) => {
                    anyhow::ensure!(
                        value.is_finite() && (0.0..=1.0).contains(&value),
                        "Invalid threshold"
                    );
                    self.state.threshold_value = value;
                }
            }
        }

        Ok(())
    }
}

impl Threshold {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = ThresholdState {
            is_enabled: false,
            threshold_value: 0.5,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuThreshold::new(gpu_image_processor),
        }
    }
}
