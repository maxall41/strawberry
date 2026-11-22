use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    GpuImageProcessor, compositor::GpuCompositor, decomposer::GpuDecomposer,
};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, DecomposerMessage, DecomposerState, Dimension, FrontendMessage,
    ImageCollection, PipelineObjectMessage, PlaneProgressExt, ReportPlaneProgress, ToolBackend,
    ToolInput, ToolState,
};

pub struct Decomposer {
    pub state: DecomposerState,
    pub base: ToolBase,
    pub cache: ToolCache<DecomposerState>,
    compositer: GpuCompositor,
    processor: GpuDecomposer,
}

#[async_trait]
impl ToolBackend for Decomposer {
    impl_tool_backend_basics!(Decomposer);

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::DecomposerState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Decomposer
    }

    /// Decompose the single layer at each position and composite its enabled channel layers.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        let mut lanes = images.lanes(Dimension::C);
        for lane in lanes
            .iter_mut()
            .report_progress_in_batches(GpuDecomposer::CHANNEL_COUNT, report_planes)
        {
            let [image] = lane.as_slice() else {
                let error_msg = format!(
                    "Decomposer can only process images with one layer, received {}",
                    lane.len()
                );
                self.base
                    .events
                    .emit(FrontendMessage::error(error_msg.clone()));
                return Err(anyhow::anyhow!(error_msg));
            };

            let decomposed = self.processor.apply_decomposition(image).await?;

            let enabled: Vec<image::RgbaImage> = decomposed
                .into_iter()
                .enumerate()
                .filter(|(i, _)| self.state.enabled_layers.get(*i).copied().unwrap_or(false))
                .map(|(_, channel)| channel)
                .collect();

            let composited = self.compositer.composite_images(&enabled).await?;
            *lane = vec![Arc::new(composited)];
        }
        images.replace_lanes(Dimension::C, lanes)
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::DecomposerMessage(message)) = message {
            match message {
                DecomposerMessage::ToggleVisibility(value, layer_index) => {
                    let mut mutated = self.state.enabled_layers.clone();
                    let index = usize::from(layer_index);

                    if index >= mutated.len() {
                        let error_msg = format!(
                            "Decomposer: Layer index {} out of bounds (max: {})",
                            index,
                            mutated.len()
                        );
                        self.base
                            .events
                            .emit(FrontendMessage::error(error_msg.clone()));
                        return Err(anyhow::anyhow!(error_msg));
                    }

                    if let Some(layer) = mutated.get_mut(index) {
                        *layer = value;
                    }
                    self.state.enabled_layers = mutated;
                }
            }
        }

        Ok(())
    }
}

impl Decomposer {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = DecomposerState {
            is_enabled: false,
            enabled_layers: vec![true; GpuDecomposer::CHANNEL_COUNT],
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            compositer: GpuCompositor::new(gpu_image_processor.clone()).await?,
            processor: GpuDecomposer::new(gpu_image_processor).await?,
        })
    }
}
