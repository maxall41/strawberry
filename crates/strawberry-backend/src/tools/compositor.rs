use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, compositor::GpuCompositor};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, CompositorMessage, CompositorState, FrontendMessage, ImageCollection,
    PipelineObjectMessage, ToolBackend, ToolInput, ToolState,
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

    /// Composite the enabled layers of each stack into a single BGRA image.
    async fn process(&mut self, images: &mut ImageCollection) -> anyhow::Result<()> {
        self.state.real_layers_len = images.layer_count();

        for stack in &mut images.stacks {
            if stack.layers.len() == 1 {
                continue;
            }
            let enabled: Vec<_> = stack
                .layers
                .iter()
                .enumerate()
                .filter(|(i, _)| self.state.enabled_layers.get(*i).copied().unwrap_or(false))
                .map(|(_, layer)| layer.clone())
                .collect();
            let composited = self.compositer.composite_images(&enabled).await?;
            stack.layers = vec![Arc::new(composited)];
        }

        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::CompositorMessage(message)) = message {
            match message {
                CompositorMessage::ToggleVisibility(value, layer_index) => {
                    let mut mutated = self.state.enabled_layers.clone();
                    let index = usize::from(layer_index);

                    if index >= mutated.len() {
                        let error_msg = format!(
                            "Compositor: Layer index {} out of bounds (max: {})",
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

                    let end_range = std::cmp::min(self.state.real_layers_len, mutated.len());
                    if mutated
                        .get(0..end_range)
                        .is_none_or(|slice| slice.iter().all(|&x| !x))
                    {
                        return Ok(());
                    }

                    self.state.enabled_layers = mutated;
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
            enabled_layers: vec![true; 8],
            real_layers_len: 0,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            compositer: GpuCompositor::new(gpu_image_processor).await?,
        })
    }
}
