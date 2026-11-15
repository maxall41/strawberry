use crate::tools::base::ToolEvents;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, watershed::GpuWatershed};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineObjectMessage, ToolBackend, ToolInput, ToolState,
    WatershedMessage, WatershedState,
};

pub struct Watershed {
    pub state: WatershedState,
    pub base: ToolBase,
    pub cache: ToolCache<WatershedState>,
    processor: GpuWatershed,
}

#[async_trait]
impl ToolBackend for Watershed {
    impl_tool_backend_basics!(Watershed);

    fn get_state(&self) -> ToolState {
        ToolState::WatershedState(self.state.clone())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Watershed
    }

    async fn process(&mut self, input: &mut Vec<Arc<image::RgbaImage>>) -> anyhow::Result<()> {
        for image in input {
            *image = match self
                .processor
                .apply_watershed(image, self.state.sigma, self.state.min_distance)
                .await
            {
                Ok(img) => Arc::new(img),
                Err(e) => {
                    self.base
                        .events
                        .emit(FrontendMessage::error(format!("Watershed error: {e}")));
                    return Err(e);
                }
            };
        }
        Ok(())
    }

    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        match message {
            ToolInput::Message(message) => match message {
                PipelineObjectMessage::WatershedMessage(message) => match message {
                    WatershedMessage::SetParameters(sigma, min_distance) => {
                        anyhow::ensure!(
                            sigma.is_finite() && sigma > 0. && min_distance > 0,
                            "Invalid watershed parameters"
                        );
                        self.state.sigma = sigma;
                        self.state.min_distance = min_distance;
                    }
                },
                _ => {}
            },
            _ => {}
        }

        Ok(())
    }
}

impl Watershed {
    #[must_use]
    pub fn new(events: ToolEvents, gpu_image_processor: Arc<GpuImageProcessor>) -> Self {
        let state = WatershedState {
            is_enabled: false,
            sigma: 1.0,
            min_distance: 10,
        };
        Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuWatershed::new(gpu_image_processor),
        }
    }
}
