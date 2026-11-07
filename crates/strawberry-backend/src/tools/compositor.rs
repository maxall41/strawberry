use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, compositor::GpuCompositor};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, CompositorMessage, CompositorState, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
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

    fn get_state(&self) -> ToolState {
        ToolState::CompositorState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::CompositorState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Compositor
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        self.state.real_layers_len = input.len();

        if input.len() == 1 {
            self.base
                .send_to_frontend(PipelineCommand::FrontendMessage(
                    FrontendMessage::ReturnState(self.get_state()),
                ))?;
            return Ok(());
        }

        let images: Vec<Arc<image::RgbaImage>> = input
            .clone()
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
                self.base
                    .global_sender
                    .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                        format!("Compositor error: {e}"),
                    )))?;
                return Err(e);
            }
        };

        *input = vec![Arc::new(composited)];

        if let Some(frontend_state) = &self.cache.frontend_latest_state {
            println!("State Status {}", frontend_state == &self.state);
            println!(
                "Real Layers Length Comparison | Frontend: {} Backend: {}",
                frontend_state.real_layers_len, self.state.real_layers_len
            )
        } else {
            println!("Frontend state is empty")
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
                    PipelineObjectMessage::CompositorMessage(message) => match message {
                        CompositorMessage::ToggleVisibility(value, layer_index) => {
                            let mut mutated = self.state.enabled_layers.clone();
                            let index = layer_index as usize;

                            if index >= mutated.len() {
                                let error_msg = format!(
                                    "Compositor: Layer index {} out of bounds (max: {})",
                                    index,
                                    mutated.len()
                                );
                                self.base
                                    .global_sender
                                    .send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::error(error_msg.clone()),
                                    ))?;
                                return Err(anyhow::anyhow!(error_msg));
                            }

                            mutated[index] = value;

                            let end_range =
                                std::cmp::min(self.state.real_layers_len, mutated.len());
                            if mutated
                                .get(0..end_range)
                                .map(|slice| slice.iter().all(|&x| !x))
                                .unwrap_or(true)
                            {
                                return Ok(());
                            }

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

impl Compositor {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: CompositorState {
                is_enabled: false,
                enabled_layers: vec![true; 8],
                real_layers_len: 0,
            },
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            compositer: GpuCompositor::new(gpu_image_processor).await,
        }
    }
}
