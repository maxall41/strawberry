use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, rolling_ball::GpuRollingBall};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, RollingBallMessage,
    RollingBallState, ToolBackend, ToolState,
};

pub struct RollingBall {
    pub state: RollingBallState,
    pub base: ToolBase,
    pub cache: ToolCache<RollingBallState>,
    rolling_ball: GpuRollingBall,
}

#[async_trait]
impl ToolBackend for RollingBall {
    impl_tool_backend_basics!(RollingBall);

    fn get_state(&self) -> ToolState {
        ToolState::RollingBallState(self.state.clone())
    }

    fn apply_state(&mut self, state: ToolState) {
        if let ToolState::RollingBallState(new_state) = state {
            self.state = new_state;
        }
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::RollingBall
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .rolling_ball
                .apply_rolling_ball(image, self.state.ball_radius)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .global_sender
                        .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                            format!("Rolling Ball error: {e}"),
                        )))?;
                    return Err(e);
                }
            };
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
                    PipelineObjectMessage::RollingBallMessage(message) => match message {
                        RollingBallMessage::SetBallRadius(ball_radius) => {
                            self.state.ball_radius = ball_radius;
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

impl RollingBall {
    pub async fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Self {
        Self {
            state: RollingBallState::default(),
            base: ToolBase::new(command_sender, global_sender),
            cache: ToolCache::new(),
            rolling_ball: GpuRollingBall::new(gpu_image_processor).await,
        }
    }
}
