use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, gaussian_blur::GpuGaussianBlur};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, GaussianBlurMessage, GaussianBlurState, ImageCollection,
    PipelineObjectMessage, Plane, PlaneProgressExt, ReportPlaneProgress, ToolBackend, ToolInput,
    ToolState,
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

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::GaussianBlurState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::GaussianBlur
    }

    /// Apply the configured Gaussian blur to every input plane.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        for plane in images.planes_mut().report_progress(report_planes) {
            let input = plane.load()?;
            *plane = Plane::new(self.processor.apply_blur(&input, self.state.sigma).await?);
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::GaussianBlurMessage(message)) = message {
            match message {
                GaussianBlurMessage::SetSigma(sigma) => {
                    anyhow::ensure!(sigma.is_finite() && sigma > 0., "Sigma must be positive");
                    self.state.sigma = sigma;
                }
            }
        }

        Ok(())
    }
}

impl GaussianBlur {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = GaussianBlurState {
            is_enabled: false,
            sigma: 2.0,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuGaussianBlur::new(gpu_image_processor).await?,
        })
    }
}
