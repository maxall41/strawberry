use async_trait::async_trait;
use std::collections::HashMap;

use crate::{
    BackendToolType, ImageCollection, PersistedState, PipelineObjectMessage, PlaneProgress,
    Position, ToolInput, ToolState, overlay::ToolOverlay,
};

/// Named files stored alongside a tool's configuration in a pipeline archive.
pub type ToolAssets = HashMap<String, Vec<u8>>;

/// Publish an active stage's plane counts from the thread processing them.
pub type ReportPlaneProgress = Box<dyn FnMut(PlaneProgress) + Send>;

/// Adapts iterators so a processing loop reports its progress in planes.
pub trait PlaneProgressExt: ExactSizeIterator + Sized {
    /// Report one completed plane per iteration.
    fn report_progress(self, report_planes: ReportPlaneProgress) -> PlaneProgressIter<Self> {
        self.report_progress_in_batches(1, report_planes)
    }

    /// Report progress for items that each process the same number of planes.
    fn report_progress_in_batches(
        self,
        planes_per_item: usize,
        report_planes: ReportPlaneProgress,
    ) -> PlaneProgressIter<Self> {
        PlaneProgressIter {
            total: self.len().saturating_mul(planes_per_item),
            completed: 0,
            items: self,
            planes_per_item,
            report_planes,
        }
    }
}

impl<I: ExactSizeIterator> PlaneProgressExt for I {}

/// Iterator that counts an item's planes as finished when the loop asks for the next item.
pub struct PlaneProgressIter<I> {
    items: I,
    planes_per_item: usize,
    completed: usize,
    total: usize,
    report_planes: ReportPlaneProgress,
}

impl<I: Iterator> Iterator for PlaneProgressIter<I> {
    type Item = I::Item;

    /// Report the planes finished so far, then yield the next item.
    fn next(&mut self) -> Option<I::Item> {
        (self.report_planes)(PlaneProgress {
            completed: self.completed,
            total: self.total,
        });
        let item = self.items.next()?;
        self.completed = self.completed.saturating_add(self.planes_per_item);
        Some(item)
    }
}

/// Trait for image processing tools (backend)
#[async_trait]
pub trait ToolBackend: Send + Sync {
    /// Process the images of the collection in place, reporting plane counts when the stage
    /// tracks them.
    /// # Errors
    /// Returns an error if processing fails.
    async fn process(
        &mut self,
        images: &mut ImageCollection,
        report_planes: ReportPlaneProgress,
    ) -> anyhow::Result<()>;

    /// Check if this tool is currently enabled
    fn is_enabled(&self) -> bool;

    /// Handle a tool message or canvas interaction.
    ///
    /// # Errors
    /// Returns an error if the input cannot be applied.
    fn handle_input(&mut self, input: ToolInput) -> anyhow::Result<()>;

    /// Handle a tool message through the tool's input handler.
    ///
    /// # Errors
    /// Returns an error if the input handler cannot apply the message.
    fn handle_message(&mut self, message: PipelineObjectMessage) -> anyhow::Result<()> {
        self.handle_input(ToolInput::Message(message))
    }

    /// Collect results from background work.
    ///
    /// # Errors
    /// Returns an error if background work fails or its results cannot be applied.
    fn poll_background(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    /// Cancel any active canvas interaction owned by this tool.
    fn cancel_interaction(&mut self) {}
    /// Direct canvas edits and per-position annotations to the given position.
    fn select_position(&mut self, _position: Position) {}
    /// Return whether the tool is computing an overlay in the background, which the viewer shows
    /// as pipeline activity.
    fn has_pending_overlay(&self) -> bool {
        false
    }
    /// Report and consume pending overlay changes; return false by default.
    fn take_overlay_changes(&mut self) -> bool {
        false
    }
    /// Drain transient events queued by this tool.
    fn drain_events(&mut self) -> Vec<crate::FrontendMessage>;

    /// Reconcile backend mutations against last known pipeline run; returns whether the pipeline needs processing.
    fn reconcile_state(&mut self) -> bool;

    /// Acknowledge state produced by latest processing run, without scheduling another pass.
    fn acknowledge_processed_state(&mut self);

    /// Enable or disable this tool
    fn set_enabled(&mut self, enabled: bool);

    /// Get the current state of the tool for synchronization
    fn get_state(&self) -> ToolState;

    /// Identify
    fn iam(&self) -> BackendToolType;

    /// Supports processing
    fn supports_processing(&self) -> bool {
        true
    }

    /// Load parsed settings into a detached candidate.
    /// # Errors
    /// Returns an error if the wrapper is malformed or fields cannot be deserialized.
    fn load_persisted_state(
        &mut self,
        loaded_state: serde_value::Value,
        assets: &ToolAssets,
        cli: bool,
    ) -> anyhow::Result<()> {
        let serde_value::Value::Map(mut variants) = loaded_state else {
            anyhow::bail!("Expected an externally tagged tool state");
        };
        anyhow::ensure!(variants.len() == 1, "Expected one tool state variant");
        let expected = serde_value::Value::String(format!("{:?}State", self.iam()));
        let Some(fields) = variants.remove(&expected) else {
            anyhow::bail!(
                "Expected {:?}State variant for {:?}",
                self.iam(),
                self.iam()
            );
        };
        self.load_fields(fields, assets, cli)
    }

    /// Load concrete settings and embedded files into a detached candidate.
    /// # Errors
    /// Returns an error if settings or assets cannot be loaded.
    fn load_fields(
        &mut self,
        fields: serde_value::Value,
        _assets: &ToolAssets,
        _cli: bool,
    ) -> anyhow::Result<()> {
        let Some(current) = self.persisted_state_mut() else {
            return Ok(());
        };
        current.load_persisted_fields(fields)
    }

    /// Serialize files that must travel with this tool's saved pipeline settings.
    /// # Errors
    /// Returns an error if an asset cannot be serialized.
    fn pipeline_assets(&self) -> anyhow::Result<ToolAssets> {
        Ok(ToolAssets::new())
    }

    /// Expose the concrete state for loading, or `None` if loading is unsupported.
    fn persisted_state_mut(&mut self) -> Option<&mut dyn PersistedState> {
        None
    }

    /// Return cached output for the supplied input and current processing settings.
    fn valid_cached_output(&self, _input: &ImageCollection) -> Option<ImageCollection> {
        None
    }

    /// Refresh transient state when a processing pass restores this tool's output.
    fn refresh_after_cache_hit(&mut self) {}

    /// Cache the output of a successful run for the supplied input.
    fn set_cached_output(&mut self, _input: &ImageCollection, _output: ImageCollection) {}

    /// Clear a bypassed tool's retained input and output if its upstream images have changed.
    fn reconcile_bypassed_input(&mut self, _input: &ImageCollection) {}

    /// Invalidate reusable output while retaining input needed for canvas interactions.
    fn invalidate_cached_output(&mut self) {
        self.invalidate_cache();
    }

    /// Invalidate all processing caches, including cached input.
    fn invalidate_cache(&mut self) {}

    /// Release state tied to cleared input images, including when a new collection is loaded.
    fn release_previous_input(&mut self) {}

    /// Get overlay elements to render on the primary viewer.
    /// Returns None if the tool doesn't produce overlays.
    fn get_overlays(&self, _id: crate::ToolId) -> Option<ToolOverlay> {
        None
    }
}

/// Trait for frontend tool views (GPUI)
pub trait ToolFrontend {
    /// Initialize the tool view and connect its controls to the window.
    fn initialize(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>)
    where
        Self: Sized;
}
