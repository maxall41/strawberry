use async_trait::async_trait;
use image::RgbaImage;
use std::sync::Arc;

use crate::{
    BackendToolType, PersistedState, PipelineCommand, PreparedStateUpdate, ToolState,
    overlay::ToolOverlay,
};

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

    /// Reconcile backend mutations against last known pipeline run; returns whether the pipeline needs processing.
    fn reconcile_state(&mut self) -> bool;

    /// Acknowledge state produced by latest processing run, without scheduling another pass.
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

    /// Prepare parsed settings without changing live state.
    /// # Errors
    /// Returns an error if the wrapper is malformed or fields cannot be deserialized.
    fn prepare_loaded_persisted_state<'a>(
        &'a mut self,
        loaded_state: serde_value::Value,
    ) -> anyhow::Result<Option<PreparedStateUpdate<'a>>> {
        let serde_value::Value::Map(mut variants) = loaded_state else {
            anyhow::bail!("Expected an externally tagged tool state");
        };
        anyhow::ensure!(variants.len() == 1, "Expected one tool state variant");
        let expected = serde_value::Value::String(format!("{:?}State", self.iam()));
        let Some(fields) = variants.remove(&expected) else {
            anyhow::bail!("Expected {:?}State variant for {:?}", self.iam(), self.iam());
        };
        let Some(current) = self.persisted_state_mut() else {
            return Ok(None);
        };
        current.prepare_persisted_fields(fields).map(Some)
    }

    /// Expose the concrete state for loading, or `None` if loading is unsupported.
    fn persisted_state_mut(&mut self) -> Option<&mut dyn PersistedState> {
        None
    }

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
