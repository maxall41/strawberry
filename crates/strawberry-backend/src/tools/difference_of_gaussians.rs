use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use async_trait::async_trait;
use strawberry_processing::{GpuImageProcessor, difference_of_gaussians::GpuDifferenceOfGaussians};
use strawberry_types::{
    BackendToolType, DifferenceOfGaussiansMessage, DifferenceOfGaussiansState, ImageCollection,
    PipelineObjectMessage, PlaneProgressExt, ReportPlaneProgress, ToolBackend, ToolInput,
    ToolState,
};

use crate::{
    impl_tool_backend_basics,
    tools::{base::ToolBase, cache::ToolCache},
};

pub struct DifferenceOfGaussians {
    pub state: DifferenceOfGaussiansState,
    pub base: ToolBase,
    pub cache: ToolCache<DifferenceOfGaussiansState>,
    processor: GpuDifferenceOfGaussians,
}

#[async_trait]
impl ToolBackend for DifferenceOfGaussians {
    impl_tool_backend_basics!(DifferenceOfGaussians);

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::DifferenceOfGaussiansState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::DifferenceOfGaussians
    }

    /// Apply the configured difference of Gaussian blurs to every input plane.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        for image in images.planes_mut().report_progress(report_planes) {
            *image = Arc::new(
                self.processor
                    .apply(image, self.state.small_sigma, self.state.large_sigma)
                    .await?,
            );
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::DifferenceOfGaussiansMessage(message)) =
            message
        {
            match message {
                DifferenceOfGaussiansMessage::SetSmallSigma(sigma)
                    if sigma.is_finite() && sigma >= 0.1 && sigma < self.state.large_sigma =>
                {
                    self.state.small_sigma = sigma;
                }
                DifferenceOfGaussiansMessage::SetLargeSigma(sigma)
                    if sigma.is_finite() && sigma > self.state.small_sigma && sigma <= 64.0 =>
                {
                    self.state.large_sigma = sigma;
                }
                _ => {}
            }
        }

        Ok(())
    }
}

impl DifferenceOfGaussians {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(events: ToolEvents, gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        let state = DifferenceOfGaussiansState::default();
        Ok(Self {
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            state,
            processor: GpuDifferenceOfGaussians::new(gpu).await?,
        })
    }
}
