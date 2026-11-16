use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    GpuImageProcessor,
    morphology::{GpuMorphology, MorphologyOperation},
};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, MorphologyMessage, MorphologyState, PipelineObjectMessage,
    ToolBackend, ToolInput, ToolState,
};

pub struct Morphology {
    pub state: MorphologyState,
    pub base: ToolBase,
    pub cache: ToolCache<MorphologyState>,
    processor: GpuMorphology,
}

#[async_trait]
impl ToolBackend for Morphology {
    impl_tool_backend_basics!(Morphology);

    fn get_state(&self) -> ToolState {
        ToolState::MorphologyState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Morphology
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .processor
                .apply_morphology(image, self.state.operation, self.state.element_size)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .events
                        .emit(FrontendMessage::error(format!("Morphology error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::MorphologyMessage(message)) = message {
            match message {
                MorphologyMessage::SetOperation(operation) => {
                    self.state.operation = operation;
                }
                MorphologyMessage::SetElementSize(element_size) => {
                    anyhow::ensure!(element_size > 0, "Size must be positive");
                    self.state.element_size = element_size;
                }
            }
        }

        Ok(())
    }
}

impl Morphology {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = MorphologyState {
            is_enabled: false,
            operation: MorphologyOperation::Dilation,
            element_size: 3,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuMorphology::new(gpu_image_processor),
        }
    }
}
