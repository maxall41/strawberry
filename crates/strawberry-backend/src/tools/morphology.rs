use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    morphology::{GpuMorphology, MorphologyOperation},
    GpuImageProcessor,
};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, MorphologyMessage, MorphologyState, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};

pub struct Morphology {
    pub state: MorphologyState,
    pub base: ToolBase,
    pub cache: ToolCache<MorphologyState>,
    morphology: GpuMorphology,
}

#[async_trait]
impl ToolBackend for Morphology {
    impl_tool_backend_basics!(Morphology);

    fn get_state(&self) -> ToolState {
        ToolState::MorphologyState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::MorphologyState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Morphology
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .morphology
                .apply_morphology(image, self.state.operation, self.state.element_size)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Morphology error: {e}"),
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
                    PipelineObjectMessage::MorphologyMessage(message) => match message {
                        MorphologyMessage::SetOperation(operation) => {
                            self.state.operation = operation;
                        }
                        MorphologyMessage::SetElementSize(element_size) => {
                            self.state.element_size = element_size;
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

impl Morphology {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: MorphologyState {
                is_enabled: false,
                operation: MorphologyOperation::Dilation,
                element_size: 3,
            },
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            morphology: GpuMorphology::new(gpu_image_processor).await,
        }
    }
}
