use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{watershed::GpuWatershed, GpuImageProcessor};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, ToolBackend,
    ToolState, WatershedMessage, WatershedState,
};

pub struct Watershed {
    pub state: WatershedState,
    pub base: ToolBase,
    pub cache: ToolCache<WatershedState>,
    watershed: GpuWatershed,
}

#[async_trait]
impl ToolBackend for Watershed {
    impl_tool_backend_basics!(Watershed);

    fn get_state(&self) -> ToolState {
        ToolState::WatershedState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::WatershedState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Watershed
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .watershed
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
                        .send_to_frontend(PipelineCommand::FrontendMessage(
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

        if self.cache.frontend_latest_state.as_ref() != Some(&self.state) {
            self.cache.cached_output = None;
            self.cache.cached_state = None;
            self.base
                .send_to_frontend(PipelineCommand::FrontendMessage(
                    FrontendMessage::ReturnState(self.get_state()),
                ))?;
            self.base.trigger_refresh()?;
            self.cache.frontend_latest_state = Some(self.state.clone());
        }

        Ok(())
    }
}

impl Watershed {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: WatershedState {
                is_enabled: false,
                sigma: 1.0,
                min_distance: 10,
            },
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            watershed: GpuWatershed::new(gpu_image_processor).await,
        }
    }
}
