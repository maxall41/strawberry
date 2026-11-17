use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, rolling_ball::GpuRollingBall};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, PipelineObjectMessage, RollingBallMessage, RollingBallState, ToolBackend,
    ToolInput, ToolState,
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

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::RollingBallState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::RollingBall
    }

    /// Subtract the configured rolling-ball background from each input layer.
    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = Arc::new(
                self.processor
                    .apply_rolling_ball(image, self.state.ball_radius)
                    .await?,
            );
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
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
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = RollingBallState::default();
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuRollingBall::new(gpu_image_processor).await?,
        })
    }
}
