use std::sync::Arc;

use async_trait::async_trait;
use strawberry_processing::{GpuImageProcessor, difference_of_gaussians::GpuDifferenceOfGaussians};
use strawberry_types::{
    BackendToolType, DifferenceOfGaussiansMessage, DifferenceOfGaussiansState, FrontendMessage,
    PipelineCommand, PipelineObjectMessage, ToolBackend, ToolState,
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
            *image = match self
                .processor
                .apply(image, self.state.small_sigma, self.state.large_sigma)
                .await
            {
                Ok(output) => Arc::new(output),
                Err(error) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Difference of Gaussians error: {error}"),
                        )))?;
                    return Err(error);
                }
            };
        }
        Ok(())
    }

    fn receive_messages(&mut self) -> anyhow::Result<()> {
        for message in self.base.collect_commands() {
            match message {
                PipelineCommand::RequestState() => {
                    self.base
                        .send_to_frontend(&PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnState(self.get_state()),
                        ))?;
                }
                PipelineCommand::PipelineObject(PipelineObjectMessage::ToggleActive(enabled)) => {
                    self.state.is_enabled = enabled;
                }
                PipelineCommand::PipelineObject(
                    PipelineObjectMessage::DifferenceOfGaussiansMessage(message),
                ) => match message {
                    DifferenceOfGaussiansMessage::SetSmallSigma(sigma)
                        if sigma.is_finite() && sigma > 0.0 && sigma < self.state.large_sigma =>
                    {
                        self.state.small_sigma = sigma;
                    }
                    DifferenceOfGaussiansMessage::SetLargeSigma(sigma)
                        if sigma.is_finite() && sigma > self.state.small_sigma && sigma <= 64.0 =>
                    {
                        self.state.large_sigma = sigma;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }
}

impl DifferenceOfGaussians {
    #[must_use]
    pub fn new(
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = DifferenceOfGaussiansState::default();
        Self {
            base: ToolBase::new(global_sender),
            cache: ToolCache::new(&state),
            state,
            processor: GpuDifferenceOfGaussians::new(gpu),
        }
    }
}
