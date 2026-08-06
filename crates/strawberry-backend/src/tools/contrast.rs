use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{contrast::GpuContrast, GpuImageProcessor};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, ContrastMessage, ContrastState, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};

pub struct Contrast {
    pub state: ContrastState,
    pub base: ToolBase,
    pub cache: ToolCache<ContrastState>,
    contrast: GpuContrast,
}

#[async_trait]
impl ToolBackend for Contrast {
    impl_tool_backend_basics!(Contrast);

    fn get_state(&self) -> ToolState {
        ToolState::ContrastState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::ContrastState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Contrast
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .contrast
                .apply_contrast(image, self.state.min, self.state.max)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Contrast error: {e}"),
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
                    PipelineObjectMessage::ContrastMessage(message) => match message {
                        ContrastMessage::SetMinMax(min, max) => {
                            self.state.min = min;
                            self.state.max = max;
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

impl Contrast {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: ContrastState {
                is_enabled: false,
                min: 0.0,
                max: 1.0,
            },
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            contrast: GpuContrast::new(gpu_image_processor).await,
        }
    }
}
