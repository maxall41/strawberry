use crate::tools::base::ToolEvents;
use anyhow::Result;
use std::sync::Arc;

use crate::impl_tool_backend_basics;
use crate::tools::base::ToolBase;
use crate::tools::cache::ToolCache;
use strawberry_processing::{GpuImageProcessor, watershed::GpuWatershed};

use async_trait::async_trait;
use strawberry_types::{
    BackendToolType, ImageCollection, PipelineObjectMessage, Plane, PlaneProgressExt,
    ReportPlaneProgress, ToolBackend, ToolInput, ToolState, WatershedMessage, WatershedState,
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

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::WatershedState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::Watershed
    }

    /// Apply the configured watershed segmentation to every input plane.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()> {
        for plane in images.planes_mut().report_progress(report_planes) {
            let input = plane.load()?;
            *plane = Plane::new(
                self.processor
                    .apply_watershed(&input, self.state.sigma, self.state.min_distance)
                    .await?,
            );
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::WatershedMessage(message)) = message {
            match message {
                WatershedMessage::SetParameters(sigma, min_distance) => {
                    anyhow::ensure!(
                        sigma.is_finite() && sigma > 0. && min_distance > 0,
                        "Invalid watershed parameters"
                    );
                    self.state.sigma = sigma;
                    self.state.min_distance = min_distance;
                }
            }
        }

        Ok(())
    }
}

impl Watershed {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(
        events: ToolEvents,
        gpu_image_processor: Arc<GpuImageProcessor>,
    ) -> Result<Self> {
        let state = WatershedState {
            is_enabled: false,
            sigma: 1.0,
            min_distance: 10,
        };
        Ok(Self {
            state: state.clone(),
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            processor: GpuWatershed::new(gpu_image_processor).await?,
        })
    }
}
