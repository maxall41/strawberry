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
        let compositer = GpuCompositor::new().await;

        self.state.real_layers_len = input.len();

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

        let composited = compositer.composite_images(&images).await.unwrap();

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
                    self.state.enabled_layers[*layer_index as usize] = *value;
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
            command_sender,
        }
    }
}
