use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    compositor::GpuCompositor, decomposer::GpuDecomposer, GpuImageProcessor,
};

use strawberry_types::{
    BackendToolType, DecomposerMessage, DecomposerState, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};
use async_trait::async_trait;

pub struct Decomposer {
    pub state: DecomposerState,
    pub base: ToolBase,
    pub cache: ToolCache<DecomposerState>,
    compositer: GpuCompositor,
    gpu_decomposer: GpuDecomposer,
}

#[async_trait]
impl ToolBackend for Decomposer {
    impl_tool_backend_basics!(Decomposer);

    fn get_state(&self) -> ToolState {
        ToolState::DecomposerState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::DecomposerState(new_state) = state {
            self.state = new_state;
        }
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
            self.base.global_sender.send(PipelineCommand::FrontendMessage(
                FrontendMessage::error(error_msg.clone()),
            ))?;
            return Err(anyhow::anyhow!(error_msg));
        }

        let first_image = input.first().ok_or_else(|| {
            let error_msg = "No input image provided to decomposer".to_string();
            let _ = self.base.global_sender.send(PipelineCommand::FrontendMessage(
                FrontendMessage::error(error_msg.clone()),
            ));
            anyhow::anyhow!(error_msg)
        })?;

        let decomposed = match self.gpu_decomposer.apply_decomposition(first_image).await {
            Ok(result) => result,
            Err(e) => {
                self.base.global_sender.send(PipelineCommand::FrontendMessage(
                    FrontendMessage::error(format!("Decomposer error: {e}")),
                ))?;
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
                self.base.global_sender.send(PipelineCommand::FrontendMessage(
                    FrontendMessage::error(format!("Decomposer compositor error: {e}")),
                ))?;
                return Err(e);
            }
        };

        *input = vec![Arc::new(composited)];

        Ok(())
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands = self.base.collect_commands();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.base.send_to_frontend(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    ))?;
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::DecomposerMessage(message) => match message {
                        DecomposerMessage::ToggleVisibility(value, layer_index) => {
                            let mut mutated = self.state.enabled_layers.clone();
                            let index = layer_index as usize;

                            if index >= mutated.len() {
                                let error_msg = format!(
                                    "Decomposer: Layer index {} out of bounds (max: {})",
                                    index,
                                    mutated.len()
                                );
                                self.base.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(error_msg.clone()),
                                ))?;
                                return Err(anyhow::anyhow!(error_msg));
                            }

                            mutated[index] = value;
                            self.state.enabled_layers = mutated;
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
            self.base.send_to_frontend(PipelineCommand::FrontendMessage(
                FrontendMessage::ReturnState(self.get_state()),
            ))?;
            self.base.trigger_refresh()?;
            self.cache.frontend_latest_state = Some(self.state.clone());
        }

        Ok(())
    }
}

impl Decomposer {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: DecomposerState {
                is_enabled: false,
                enabled_layers: vec![true; 3],
            },
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            compositer: GpuCompositor::new(gpu_image_processor.clone()).await,
            gpu_decomposer: GpuDecomposer::new(gpu_image_processor).await,
        }
    }
}
