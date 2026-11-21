use crate::tools::base::ToolEvents;
use std::sync::Arc;

use async_trait::async_trait;
use image::Rgba;
use lutz::{Pixel, lutz};
use strawberry_types::{
    BackendToolType, ImageCollection, ObjectSizeFilterMessage, ObjectSizeFilterMode,
    ObjectSizeFilterState, PipelineObjectMessage, ToolBackend, ToolInput, ToolState,
};

use crate::{
    impl_tool_backend_basics,
    tools::{base::ToolBase, cache::ToolCache},
    utils::binary_image::BinaryImageWrapper,
};

pub struct ObjectSizeFilter {
    pub state: ObjectSizeFilterState,
    pub base: ToolBase,
    pub cache: ToolCache<ObjectSizeFilterState>,
}

#[async_trait]
impl ToolBackend for ObjectSizeFilter {
    impl_tool_backend_basics!(ObjectSizeFilter);

    /// Return a snapshot of the tool settings for subscribers.
    fn get_state(&self) -> ToolState {
        ToolState::ObjectSizeFilterState(self.state.clone())
    }

    /// Return the backend tool kind for this instance.
    fn iam(&self) -> BackendToolType {
        BackendToolType::ObjectSizeFilter
    }

    /// Filter connected image objects using the configured size range.
    async fn process(&mut self, images: &mut ImageCollection) -> anyhow::Result<()> {
        for image in images.planes_mut() {
            let mut output = None;
            for pixels in lutz::<_, Vec<Pixel>>(BinaryImageWrapper::new(image, 128)) {
                let area = u64::try_from(pixels.len())?;
                let size = u64::from(self.state.size);
                let remove = match self.state.mode {
                    ObjectSizeFilterMode::RemoveSmallerThan => area < size,
                    ObjectSizeFilterMode::RemoveLargerThan => area > size,
                };
                if remove {
                    let output = output.get_or_insert_with(|| image.as_ref().clone());
                    for pixel in pixels {
                        let target = output.get_pixel_mut(pixel.x, pixel.y);
                        *target = Rgba([0, 0, 0, target[3]]);
                    }
                }
            }
            if let Some(output) = output {
                *image = Arc::new(output);
            }
        }
        Ok(())
    }

    /// Apply tool commands and canvas input to this instance.
    fn handle_input(&mut self, message: ToolInput) -> anyhow::Result<()> {
        if let ToolInput::Message(PipelineObjectMessage::ObjectSizeFilterMessage(message)) = message
        {
            match message {
                ObjectSizeFilterMessage::SetMode(mode) => self.state.mode = mode,
                ObjectSizeFilterMessage::SetSize(size) => self.state.size = size,
            }
        }

        Ok(())
    }
}

impl ObjectSizeFilter {
    /// Construct the tool with its initial settings and required resources.
    #[must_use]
    pub fn new(events: ToolEvents) -> Self {
        let state = ObjectSizeFilterState::default();
        Self {
            base: ToolBase::new(events),
            cache: ToolCache::new(&state),
            state,
        }
    }
}
