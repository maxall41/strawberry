use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, rolling_ball::GpuRollingBall};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineObjectMessage, RollingBallMessage, RollingBallState,
    ToolBackend, ToolInput, ToolState,
};

pub struct RollingBall {
    pub state: RollingBallState,
    pub base: ToolBase,
    pub cache: ToolCache<RollingBallState>,
    processor: GpuRollingBall,
}

#[async_trait]
impl ToolBackend for RollingBall {
    impl_tool_backend_basics!(RollingBall);

    fn get_state(&self) -> ToolState {
        ToolState::RollingBallState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::RollingBall
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .processor
                .apply_rolling_ball(image, self.state.ball_radius)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .events
                        .emit(FrontendMessage::error(format!("Rolling Ball error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::RollingBallMessage(message)) = message {
            match message {
                RollingBallMessage::SetBallRadius(ball_radius) => {
                    anyhow::ensure!(ball_radius > 0, "Size must be positive");
                    self.state.ball_radius = ball_radius;
                }
            }
        }

        Ok(())
    }
}

impl RollingBall {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = RollingBallState::default();
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuRollingBall::new(gpu_image_processor),
        }
    }
}
