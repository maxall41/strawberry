use async_trait::async_trait;
use image::RgbaImage;

use crate::{overlay::ToolOverlay, BackendToolType, PipelineCommand, ToolState};

/// Trait for image processing tools (backend)
#[async_trait]
pub trait ToolBackend: Send + Sync {
    /// Process input images and return modified images
    /// If the tool is disabled, it should return the input unchanged
    async fn process(&mut self, output_images: &mut Vec<RgbaImage>) -> anyhow::Result<()>;

    /// Check if this tool is currently enabled
    fn is_enabled(&self) -> bool;

    /// Receive messages from the pipeline
    async fn receive_messages(&mut self) -> anyhow::Result<()>;

    /// Enable or disable this tool
    fn set_enabled(&mut self, enabled: bool);

    /// Get the current state of the tool for synchronization
    fn get_state(&self) -> ToolState;

    /// Identify
    fn iam(&self) -> BackendToolType;

    /// Get the sender for frontend->backend communication
    fn get_sender(&self) -> flume::Sender<PipelineCommand>;

    /// Set the sender for backend->frontend communication
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>);

    /// Disconnect the frontend by clearing the frontend sender
    fn disconnect(&mut self);

    /// Supports processing
    fn supports_processing(&self) -> bool {
        true
    }

    /// Apply state to the tool. Used during pipeline load.
    fn apply_state(&mut self, state: ToolState);

    /// Get the tool's index in the pipeline
    fn get_index(&self) -> usize;

    /// Set the tool's index in the pipeline
    fn set_index(&mut self, index: usize);

    /// Check if cached output is valid (based on tool state)
    fn has_valid_cache(&self) -> bool {
        false
    }

    /// Get cached output if available
    fn get_cached_output(&self) -> Option<Vec<RgbaImage>> {
        None
    }

    /// Cache the output
    fn set_cached_output(&mut self, _output: Vec<RgbaImage>) {}

    /// Invalidate the cache
    fn invalidate_cache(&mut self) {}

    /// Get overlay elements to render on the primary viewer.
    /// Returns None if the tool doesn't produce overlays.
    fn get_overlays(&self) -> Option<ToolOverlay> {
        None
    }
}

/// Trait for frontend tool views (GPUI)
pub trait ToolFrontend {
    fn get_window_options(&self) -> gpui::WindowOptions;
    fn initialize(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>)
    where
        Self: Sized;
}
