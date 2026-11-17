use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;
use strawberry_processing::local_maxima::ThresholdType;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, local_maxima::GpuLocalMaxima};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, LocalMaximaMessage, LocalMaximaState, PipelineObjectMessage, ToolBackend,
    ToolInput, ToolState,
};

pub struct LocalMaxima {
    pub state: LocalMaximaState,
    pub base: ToolBase,
    pub cache: ToolCache<LocalMaximaState>,
    processor: GpuLocalMaxima,
}

#[async_trait]
impl ToolBackend for LocalMaxima {
    impl_tool_backend_basics!(LocalMaxima);

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::LocalMaximaState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::LocalMaxima
    }

    /// Detect local maxima in each input layer using the configured neighborhood.
    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = Arc::new(
                self.processor
                    .apply_local_maxima(
                        image,
                        self.state.min_distance,
                        self.state.threshold.clone(),
                        self.state.exclude_border,
                    )
                    .await?,
            );
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::LocalMaximaMessage(message)) = message {
            match message {
                LocalMaximaMessage::SetParameters(min_distance, threshold, exclude_border) => {
                    if let ThresholdType::Absolute(value) | ThresholdType::Relative(value) =
                        threshold
                    {
                        anyhow::ensure!(
                            value.is_finite() && (0.0..=1.0).contains(&value),
                            "Invalid maxima threshold"
                        );
                    }
                    self.state.min_distance = min_distance;
                    self.state.threshold = threshold;
                    self.state.exclude_border = exclude_border;
                }
            }
        }

        Ok(())
    }
}

impl LocalMaxima {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = LocalMaximaState::default();
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuLocalMaxima::new(gpu_image_processor).await?,
        })
    }
}
