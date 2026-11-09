use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, watershed::GpuWatershed};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, ToolBackend,
    ToolState, WatershedMessage, WatershedState,
};

pub struct Watershed {
    pub state: WatershedState,
    pub base: ToolBase,
    pub cache: ToolCache<WatershedState>,
    processor: GpuWatershed,
}

#[async_trait]
impl ToolBackend for Watershed {
    impl_tool_backend_basics!(Watershed);

    fn get_state(&self) -> ToolState {
        ToolState::WatershedState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Watershed
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .processor
                .apply_watershed(image, self.state.sigma, self.state.min_distance)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Watershed error: {e}"),
                        )))?;
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands = self.base.collect_commands();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.base
                        .send_to_frontend(&PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnState(self.get_state()),
                        ))?;
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::WatershedMessage(message) => match message {
                        WatershedMessage::SetParameters(sigma, min_distance) => {
                            self.state.sigma = sigma;
                            self.state.min_distance = min_distance;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        Ok(())
    }
}

impl Watershed {
    #[must_use]
    pub fn new(
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = WatershedState {
            is_enabled: false,
            sigma: 1.0,
            min_distance: 10,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(global_sender),
            cache: ToolCache::new(&state),
            processor: GpuWatershed::new(gpu_image_processor),
        }
    }
}
