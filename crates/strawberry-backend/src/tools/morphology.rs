use crate::tools::base::ToolEvents;
use anyhow::Result;
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
    BackendToolType, ImageCollection, MorphologyMessage, MorphologyState, PipelineObjectMessage,
    Plane, PlaneProgressExt, ReportPlaneProgress, ToolBackend, ToolInput, ToolState,
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

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::MorphologyState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Morphology
    }

    /// Apply the selected morphological operation to every input plane.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        for plane in images.planes_mut().report_progress(report_planes) {
            let input = plane.load()?;
            *plane = Plane::new(
                self.processor
                    .apply_morphology(&input, self.state.operation, self.state.element_size)
                    .await?,
            );
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
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
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = MorphologyState {
            is_enabled: false,
            operation: MorphologyOperation::Dilation,
            element_size: 3,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuMorphology::new(gpu_image_processor).await?,
        })
    }
}
