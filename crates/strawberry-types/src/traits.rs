use async_trait::async_trait;
use image::RgbaImage;
use std::sync::Arc;

use crate::{BackendToolType, PipelineCommand, ToolState, overlay::ToolOverlay};

/// Trait for image processing tools (backend)
#[async_trait]
pub trait ToolBackend: Send + Sync {
    /// Process input images and return modified images
    async fn process(&mut self, output_images: &mut Vec<Arc<RgbaImage>>) -> anyhow::Result<()>;

    /// Check if this tool is currently enabled
    fn is_enabled(&self) -> bool;

    /// Receive messages from the pipeline
    /// # Errors
    /// Returns an error if a command cannot be handled or a channel is disconnected.
    fn receive_messages(&mut self) -> anyhow::Result<()>;

    /// Reconcile backend mutations; return whether the pipeline needs processing.
    fn reconcile_state(&mut self) -> bool;

    /// Acknowledge state produced by processing, without scheduling another pass.
    fn acknowledge_processed_state(&mut self);

    /// Publish frontend state independently of processing/cache validity.
    /// # Errors
    /// Returns an error if a communication channel is disconnected.
    fn sync_frontend(&mut self) -> anyhow::Result<()>;

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

    /// Set the tool's index in the pipeline
    fn set_index(&mut self, index: usize);

    /// Return the cached output only if it is still valid for the tool's current state.
    fn valid_cached_output(&self) -> Option<Vec<Arc<RgbaImage>>> {
        None
    }

    /// Cache the output
    fn set_cached_output(&mut self, _output: Vec<Arc<RgbaImage>>) {}

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
