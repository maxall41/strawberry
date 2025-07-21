use crate::composite::GpuCompositor;

use super::{CompositorMessage, PipelineCommand, PipelineObjectMessage, ToolBackend, ToolState};
use async_trait::async_trait;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompositorState {
    pub is_enabled: bool,
    pub enabled_layers: Vec<bool>,
    pub real_layers_len: usize,
}

pub struct Compositor {
    pub state: CompositorState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub compositer: Option<GpuCompositor>,
}

#[async_trait]
impl ToolBackend for Compositor {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::CompositorState(self.state.clone())
    }
    async fn process(
        &mut self,
        input: Vec<image::RgbaImage>,
    ) -> anyhow::Result<Vec<image::RgbaImage>> {
        self.state.real_layers_len = input.len();

        if self.compositer.is_none() {
            self.compositer = Some(GpuCompositor::new().await);
        }

        let images: Vec<image::RgbaImage> = input
            .into_iter()
            .enumerate()
            .filter_map(|(i, image)| {
                if self.state.enabled_layers[i] {
                    Some(image)
                } else {
                    None
                }
            })
            .collect();

        let composited = self
            .compositer
            .as_ref()
            .expect("We just checked that it is not None/Ensured it is not None")
            .composite_images(&images)
            .await
            .unwrap();

        Ok(vec![composited])
    }

    async fn receive_message(&mut self, message: &PipelineObjectMessage) -> anyhow::Result<()> {
        let old_state = self.state.clone();
        match message {
            PipelineObjectMessage::ToggleActive(value) => {
                self.state.is_enabled = *value;
            }
            PipelineObjectMessage::CompositorMessage(message) => match message {
                CompositorMessage::ToggleVisibility(value, layer_index) => {
                    let mut mutated = self.state.enabled_layers.clone();
                    mutated[*layer_index as usize] = *value;

                    if mutated[0..self.state.real_layers_len].iter().all(|&x| !x) {
                        return Ok(());
                    }

                    self.state.enabled_layers = mutated;
                }
            },
        }
        if old_state != self.state {
            self.command_sender
                .send(PipelineCommand::Refresh(0))
                .unwrap();
        }
        Ok(())
    }
}

impl Compositor {
    pub fn new(command_sender: tokio::sync::broadcast::Sender<PipelineCommand>) -> Self {
        Compositor {
            state: CompositorState {
                is_enabled: false,
                enabled_layers: vec![true; 8], //TODO: Mmmm danger. Should be fine since buffer only supports 7 input images anyways...
                real_layers_len: 0,
            },
            compositer: None,
            command_sender,
        }
    }
}
