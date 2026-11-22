use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, sobel::GpuSobel};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, ImageCollection, PipelineObjectMessage, PlaneProgressExt, ReportPlaneProgress,
    SobelMessage, SobelState, ToolBackend, ToolInput, ToolState,
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

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::SobelState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Sobel
    }

    /// Apply Sobel edge detection to every input plane.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        let threshold = if self.state.use_threshold {
            Some(self.state.threshold_value)
        } else {
            None
        };

        for image in images.planes_mut().report_progress(report_planes) {
            *image = Arc::new(self.processor.apply_sobel(image, threshold).await?);
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
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
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = SobelState {
            is_enabled: false,
            threshold_value: 0.1,
            use_threshold: false,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuSobel::new(gpu_image_processor).await?,
        })
    }
}
