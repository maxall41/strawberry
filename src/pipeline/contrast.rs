use std::sync::Arc;

use crate::gpu::{compositor::GpuCompositor, contrast::GpuContrast, GpuImageProcessor};

use super::{
    BackendToolType, CompositorMessage, ContrastMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState,
};
use async_trait::async_trait;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContrastState {
    pub min: f32,
    pub max: f32,
    pub is_enabled: bool,
}

pub struct Contrast {
    pub state: ContrastState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub contrast: GpuContrast,
}

#[async_trait]
impl ToolBackend for Contrast {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::ContrastState(self.state.clone())
    }
    async fn process(
        &mut self,
        input: Vec<image::RgbaImage>,
    ) -> anyhow::Result<Vec<image::RgbaImage>> {
        let mut output = Vec::new();
        for image in input {
            let output_image = self
                .contrast
                .apply_contrast(&image, self.state.min, self.state.max)
                .await
                .unwrap();
            output.push(output_image);
        }
        Ok(output)
    }

    async fn receive_message(&mut self, message: &PipelineObjectMessage) -> anyhow::Result<()> {
        let old_state = self.state.clone();
        match message {
            PipelineObjectMessage::ToggleActive(value) => {
                self.state.is_enabled = *value;
            }
            PipelineObjectMessage::ContrastMessage(message) => match message {
                ContrastMessage::SetMinMax(min, max) => {
                    self.state.min = *min;
                    self.state.max = *max;
                }
            },
            _ => {}
        }
        if old_state != self.state {
            self.command_sender
                .send(PipelineCommand::Refresh(0))
                .unwrap();
        }
        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Contrast(vec![])
    }
}

impl Contrast {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: ContrastState {
                is_enabled: false,
                min: 0.0,
                max: 1.0,
            },
            contrast: GpuContrast::new(gpu_image_processor).await,
            command_sender,
        }
    }
}
