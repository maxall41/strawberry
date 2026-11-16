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
            let result = if self.state.auto_threshold {
                self.processor
                    .apply_auto_threshold(image, self.state.auto_method)
                    .await
            } else {
                self.processor
                    .apply_threshold(image, self.state.threshold_value)
                    .await
            };
            *image = match result {
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
                    self.state.threshold_value = value;
                }
                ThresholdMessage::SetAutoThreshold(enabled) => {
                    self.state.auto_threshold = enabled;
                }
                ThresholdMessage::SetAutoMethod(method) => {
                    self.state.auto_method = method;
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
            threshold_value: 128,
            ..ThresholdState::default()
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuThreshold::new(gpu_image_processor),
        }
    }
}
