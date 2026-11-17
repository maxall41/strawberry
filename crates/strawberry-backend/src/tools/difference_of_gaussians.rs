use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use async_trait::async_trait;
use strawberry_processing::{GpuImageProcessor, difference_of_gaussians::GpuDifferenceOfGaussians};
use strawberry_types::{
    BackendToolType, DifferenceOfGaussiansMessage, DifferenceOfGaussiansState,
    PipelineObjectMessage, ToolBackend, ToolInput, ToolState,
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

    fn get_state(&self) -> ToolState {
        ToolState::DifferenceOfGaussiansState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::DifferenceOfGaussians
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = Arc::new(
                self.processor
                    .apply(image, self.state.small_sigma, self.state.large_sigma)
                    .await?,
            );
        }
        Ok(())
    }

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
