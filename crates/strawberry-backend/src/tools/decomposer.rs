use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    GpuImageProcessor, compositor::GpuCompositor, decomposer::GpuDecomposer,
};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, DecomposerMessage, DecomposerState, FrontendMessage, PipelineObjectMessage,
    ToolBackend, ToolInput, ToolState,
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

    fn get_state(&self) -> ToolState {
        ToolState::DecomposerState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Decomposer
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        if input.len() > 1 {
            let error_msg = format!(
                "Decomposer can only process one image, received {}",
                input.len()
            );
            self.base
                .events
                .emit(FrontendMessage::error(error_msg.clone()));
            return Err(anyhow::anyhow!(error_msg));
        }

        let first_image = input.first().ok_or_else(|| {
            let error_msg = "No input image provided to decomposer".to_string();
            self.base
                .events
                .emit(FrontendMessage::error(error_msg.clone()));
            anyhow::anyhow!(error_msg)
        })?;

        let decomposed = match self.processor.apply_decomposition(first_image).await {
            Ok(result) => result,
            Err(e) => {
                self.base
                    .events
                    .emit(FrontendMessage::error(format!("Decomposer error: {e}")));
                return Err(e);
            }
        };

        let images: Vec<image::RgbaImage> = decomposed
            .into_iter()
            .enumerate()
            .filter_map(|(i, image)| {
                if self.state.enabled_layers.get(i).copied().unwrap_or(false) {
                    Some(image)
                } else {
                    None
                }
            })
            .collect();

        let composited = match self.compositer.composite_images(&images).await {
            Ok(image) => image,
            Err(e) => {
                self.base.events.emit(FrontendMessage::error(format!(
                    "Decomposer compositor error: {e}"
                )));
                return Err(e);
            }
        };

        *input = vec![Arc::new(composited)];

        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        match message {
            ToolInput::Message(message) => match message {
                PipelineObjectMessage::DecomposerMessage(message) => match message {
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
                },
                _ => {}
            },
            _ => {}
        }

        Ok(())
    }
}

impl Decomposer {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = DecomposerState {
            is_enabled: false,
            enabled_layers: vec![true; 3],
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            compositer: GpuCompositor::new(gpu_image_processor.clone()),
            processor: GpuDecomposer::new(gpu_image_processor),
        }
    }
}
