use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{
    local_maxima::GpuLocalMaxima,
    GpuImageProcessor,
};

use strawberry_types::{
    BackendToolType, FrontendMessage, LocalMaximaMessage, PipelineCommand, PipelineObjectMessage,
    ToolBackend, ToolState, LocalMaximaState,
};
use async_trait::async_trait;

pub struct LocalMaxima {
    pub state: LocalMaximaState,
    pub base: ToolBase,
    pub cache: ToolCache<LocalMaximaState>,
    local_maxima: GpuLocalMaxima,
}

#[async_trait]
impl ToolBackend for LocalMaxima {
    impl_tool_backend_basics!(LocalMaxima);

    fn get_state(&self) -> ToolState {
        ToolState::LocalMaximaState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::LocalMaximaState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::LocalMaxima
    }

    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .local_maxima
                .apply_local_maxima(
                    image,
                    self.state.min_distance,
                    self.state.threshold.clone(),
                    self.state.exclude_border,
                )
                .await
            {
                Ok(img) => img,
                Err(e) => {
                    self.base.global_sender.send(PipelineCommand::FrontendMessage(
                        FrontendMessage::error(format!("Local Maxima error: {e}")),
                    ))?;
                    return Err(e);
                }
            };
        }
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
                    PipelineObjectMessage::LocalMaximaMessage(message) => match message {
                        LocalMaximaMessage::SetParameters(
                            min_distance,
                            threshold,
                            exclude_border,
                        ) => {
                            self.state.min_distance = min_distance;
                            self.state.threshold = threshold;
                            self.state.exclude_border = exclude_border;
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

impl LocalMaxima {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: LocalMaximaState::default(),
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            local_maxima: GpuLocalMaxima::new(gpu_image_processor).await,
        }
    }
}
