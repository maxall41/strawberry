use crate::tools::base::ToolEvents;
use anyhow::{Result, anyhow, bail};
use image::RgbaImage;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    GpuImageProcessor, compositor::GpuCompositor, decomposer::GpuDecomposer,
};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, DecomposerMessage, DecomposerState, FrontendMessage, ImageCollection,
    PipelineObjectMessage, Plane, PlaneProgressExt, ReportPlaneProgress, ToolBackend, ToolInput,
    ToolState,
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

    /// Decompose each single-channel plane into its stain channels and composite the enabled
    /// ones back into one plane.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        let channel_count = images.get_shape().c;
        if channel_count != 1 {
            let error_msg = format!(
                "Decomposer can only process images with one channel, received {channel_count}"
            );
            self.base
                .events
                .emit(FrontendMessage::error(error_msg.clone()));
            bail!(error_msg);
        }
        for plane in images.planes_mut().report_progress(report_planes) {
            let input = plane.load()?;
            let enabled: Vec<RgbaImage> = self
                .processor
                .apply_decomposition(&input)
                .await?
                .into_iter()
                .zip(&self.state.enabled_channels)
                .filter(|&(_, &enabled)| enabled)
                .map(|(channel, _)| channel)
                .collect();
            *plane = Plane::new(self.compositer.composite_images(&enabled).await?);
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::DecomposerMessage(message)) = message {
            match message {
                DecomposerMessage::ToggleVisibility(value, channel_index) => {
                    let mut mutated = self.state.enabled_channels.clone();
                    let index = usize::from(channel_index);

                    if index >= mutated.len() {
                        let error_msg = format!(
                            "Decomposer: Channel index {} out of bounds (max: {})",
                            index,
                            mutated.len()
                        );
                        self.base
                            .events
                            .emit(FrontendMessage::error(error_msg.clone()));
                        return Err(anyhow!(error_msg));
                    }

                    if let Some(channel) = mutated.get_mut(index) {
                        *channel = value;
                    }
                    self.state.enabled_channels = mutated;
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
            enabled_channels: vec![true; GpuDecomposer::CHANNEL_COUNT],
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
