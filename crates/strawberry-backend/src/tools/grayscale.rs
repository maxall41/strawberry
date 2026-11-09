use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, grayscale::GpuGrayscale};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, GrayscaleState, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};

pub struct Grayscale {
    pub state: GrayscaleState,
    pub base: ToolBase,
    pub cache: ToolCache<GrayscaleState>,
    processor: GpuGrayscale,
}

#[async_trait]
impl ToolBackend for Grayscale {
    impl_tool_backend_basics!(Grayscale);

    fn get_state(&self) -> ToolState {
        ToolState::GrayscaleState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Grayscale
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self.processor.apply_grayscale(image).await {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Grayscale error: {e}"),
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
                PipelineCommand::PipelineObject(PipelineObjectMessage::ToggleActive(value)) => {
                    self.state.is_enabled = value;
                }
                _ => {}
            }
        }

        Ok(())
    }
}

impl Grayscale {
    #[must_use]
    pub fn new(
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = GrayscaleState { is_enabled: false };
        Self {
            state: state.clone(),
            base: ToolBase::new(global_sender),
            cache: ToolCache::new(&state),
            processor: GpuGrayscale::new(gpu_image_processor),
        }
    }
}
