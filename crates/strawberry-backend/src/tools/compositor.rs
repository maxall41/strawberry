use crate::tools::base::ToolEvents;
use anyhow::{Result, anyhow, ensure};
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, compositor::GpuCompositor};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, CompositorMessage, CompositorState, Dimension, FrontendMessage,
    ImageCollection, PipelineObjectMessage, PlaneProgressExt, ReportPlaneProgress, ToolBackend,
    ToolInput, ToolState,
};

pub struct Compositor {
    pub state: CompositorState,
    pub base: ToolBase,
    pub cache: ToolCache<CompositorState>,
    compositer: GpuCompositor,
}

#[async_trait]
impl ToolBackend for Compositor {
    impl_tool_backend_basics!(Compositor);

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::CompositorState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Compositor
    }

    /// Composite the enabled channels at each position into a single BGRA image; positions
    /// holding a single channel are left untouched.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        let channel_count = images.get_shape().c;
        self.state.channel_count = channel_count;
        if self.state.enabled_channels.len() < channel_count {
            self.state.enabled_channels.resize(channel_count, true);
        }
        if channel_count <= 1 {
            return Ok(());
        }
        ensure!(
            self.state
                .enabled_channels
                .iter()
                .take(channel_count)
                .any(|&enabled| enabled),
            "Compositor has no enabled channels"
        );
        let mut lanes = images.get_lanes(Dimension::C);
        for lane in lanes.iter_mut().report_progress(report_planes) {
            let enabled: Vec<_> = lane
                .iter()
                .zip(&self.state.enabled_channels)
                .filter(|&(_, &enabled)| enabled)
                .map(|(image, _)| image.clone())
                .collect();
            *lane = vec![Arc::new(self.compositer.composite_images(&enabled).await?)];
        }
        images.replace_lanes(Dimension::C, lanes)
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::CompositorMessage(message)) = message {
            match message {
                CompositorMessage::ToggleVisibility(value, channel_index) => {
                    let mut mutated = self.state.enabled_channels.clone();
                    let index = usize::from(channel_index);

                    if index >= mutated.len() {
                        let error_msg = format!(
                            "Compositor: Channel index {} out of bounds (max: {})",
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

                    let end_range = self.state.channel_count.min(mutated.len());
                    if mutated
                        .get(0..end_range)
                        .is_none_or(|slice| slice.iter().all(|&x| !x))
                    {
                        return Ok(());
                    }

                    self.state.enabled_channels = mutated;
                }
            }
        }

        Ok(())
    }
}

impl Compositor {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = CompositorState {
            is_enabled: false,
            enabled_channels: Vec::new(),
            channel_count: 0,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            compositer: GpuCompositor::new(gpu_image_processor).await?,
        })
    }
}
