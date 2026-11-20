// wgpu auto-trait checks exceed the default compiler recursion depth.
#![recursion_limit = "256"]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use image::{ImageReader, RgbaImage};
use tokio::sync::{broadcast, mpsc, watch};

use anyhow::{Context as _, anyhow, ensure};
use strawberry_processing::{
    GpuImageProcessor, colorspace::GpuColorspaceConverter, compositor::GpuCompositor,
};

// Import all types from strawberry-types
use strawberry_types::{
    CommandSender, Coordinates, Dimension, ExportScope, FrontendMessage, ImageArrangement,
    ImageCollection, OutputFrame, PendingProcessing, PipelineCommand, PipelineProgress, ToolId,
    ToolInput, overlay,
};

use tools::{ToolManager, manager::ToolInstance};
use utils::file_io::ImageArchiveWriter;

pub mod counter;
pub mod hyperstack;
pub mod nervetracer;
mod segmentation;
pub mod tools;
mod utils;

/// Pipeline that processes images through a series of tools
pub struct Pipeline {
    /// Tools in the pipeline, processed in order
    tool_manager: ToolManager,
    /// Channel for receiving commands
    command_receiver: mpsc::UnboundedReceiver<PipelineCommand>,
    /// Channel for sending output results
    output_sender: broadcast::Sender<FrontendMessage>,
    /// Latest-only channel for output frames, so the broadcast ring never pins full-resolution images
    output_frame_sender: watch::Sender<Option<OutputFrame>>,
    /// Bumped per Load command; published with each frame so consumers can match results to the load that produced them
    load_generation: u64,
    /// Input images to process
    input_images: ImageCollection,
    /// Captured owner of the current canvas gesture
    gesture_owner: Option<ToolId>,
    overlays_dirty: bool,
    /// Whether the frontend was told a tool is computing an overlay in the background.
    overlay_activity_shown: bool,
    /// Output of the last successful run
    last_output: ImageCollection,
    /// Plane shown in the viewer; interactive tools edit its position
    view: Coordinates,
    /// The canvas may retain an older successful frame after a failed run.
    output_is_current: bool,
    /// Reference to GPU image processor
    gpu: Arc<GpuImageProcessor>,
    /// Colorspace conversions
    colorspace_converter: GpuColorspaceConverter,
    /// Composites the channels at the displayed position for `ExportScope::CompositeView`.
    fallback_compositor: GpuCompositor,
}

impl Pipeline {
    /// Create a new pipeline with the given tools
    /// # Errors
    /// Returns an error if configuration, image processing, file I/O, or communication fails.
    pub async fn new() -> anyhow::Result<(
        Self,
        CommandSender,
        broadcast::Receiver<FrontendMessage>,
        watch::Receiver<Option<OutputFrame>>,
    )> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (output_tx, output_rx) = broadcast::channel(1000);
        let (frame_tx, frame_rx) = watch::channel(None);

        let gpu_image_processor = Arc::new(GpuImageProcessor::new().await?);

        let tool_manager = ToolManager::new(gpu_image_processor.clone());
        let pipeline = Self {
            tool_manager,
            command_receiver: command_rx,
            output_sender: output_tx,
            output_frame_sender: frame_tx,
            load_generation: 0,
            input_images: ImageCollection::default(),
            gesture_owner: None,
            overlays_dirty: true,
            overlay_activity_shown: false,
            last_output: ImageCollection::default(),
            view: Coordinates::default(),
            output_is_current: false,
            gpu: gpu_image_processor.clone(),
            fallback_compositor: GpuCompositor::new(gpu_image_processor.clone()).await?,
            colorspace_converter: GpuColorspaceConverter::new(gpu_image_processor).await?,
        };

        Ok((pipeline, command_tx, output_rx, frame_rx))
    }

    /// Collect overlays from all tools
    fn collect_overlays(&self) -> overlay::OverlayCollection {
        let mut collection = overlay::OverlayCollection::new();
        for (id, tool) in &self.tool_manager.tools {
            if tool.is_enabled()
                && let Some(overlay) = tool.get_overlays(*id)
            {
                collection.merge(overlay);
            }
        }
        collection
    }

    /// Process images through the pipeline starting from the given index.
    ///
    /// Callers must reconcile tool state first and include all resulting processing
    /// requests when choosing `start_index`.
    ///
    /// # Errors
    /// Returns an error if configuration, image processing, file I/O, or communication fails.
    async fn process_from(&mut self, start_index: usize) -> anyhow::Result<()> {
        self.output_is_current = false;
        let start_time = std::time::Instant::now();
        println!("Got process_from command!");
        if self.input_images.is_empty() {
            return Err(anyhow!("No input images provided"));
        }

        let mut earliest_processing_index = start_index;
        let mut resume_images = None;

        let now = Instant::now();

        // First pass: find the earliest index where we need to start processing.
        for (i, tool) in self.tool_manager.tools.values().enumerate() {
            if i >= start_index {
                break;
            }

            if tool.is_enabled() && tool.supports_processing() {
                if let Some(cached_output) = tool.valid_cached_output() {
                    resume_images = Some(cached_output);
                } else {
                    earliest_processing_index = i;
                    break;
                }
            }
        }

        let mut images = resume_images.unwrap_or_else(|| self.input_images.clone());

        let elapsed = now.elapsed();
        println!("Catching up on cache: {elapsed:.2?}");

        for tool in self
            .tool_manager
            .tools
            .values_mut()
            .skip(earliest_processing_index)
        {
            tool.invalidate_cache();
        }

        let total_stages = self
            .tool_manager
            .tools
            .values()
            .skip(earliest_processing_index)
            .filter(|tool| tool.is_enabled() && tool.supports_processing())
            .count();
        let mut stage: usize = 0;

        for tool in self
            .tool_manager
            .tools
            .values_mut()
            .skip(earliest_processing_index)
        {
            if tool.is_enabled() && tool.supports_processing() {
                let _ =
                    self.output_sender
                        .send(FrontendMessage::PipelineProgress(PipelineProgress {
                            stage,
                            total: total_stages,
                            name: format!("{:?}", tool.iam()),
                        }));
                stage = stage.saturating_add(1);
                let result = tool.process(&mut images).await;
                tool.acknowledge_processed_state();
                result.with_context(|| format!("{:?} processing failed", tool.iam()))?;
                self.gpu
                    .ensure_device_availability("Complete pipeline stage")?;
                tool.set_cached_output(images.clone());
            }
        }

        self.ensure_no_stage_gpu_errors("Publish pipeline output")?;
        self.last_output = images;
        self.clamp_view();
        self.output_is_current = true;
        self.publish_output_frame()?;

        let duration = start_time.elapsed();
        println!("Pipeline processing took: {duration:?}");

        Ok(())
    }

    /// Keep the view inside the output bounds after tools change its shape, moving tool edits with it.
    fn clamp_view(&mut self) {
        let shape = self.last_output.shape();
        for dimension in Dimension::ALL {
            let last = shape.extent(dimension).saturating_sub(1);
            self.view.set(dimension, self.view.get(dimension).min(last));
        }
        self.tool_manager.select_position(self.view.position());
    }

    /// Return the viewed output plane.
    fn get_displayed_image(&self) -> anyhow::Result<Arc<RgbaImage>> {
        self.last_output.plane(self.view).cloned().ok_or_else(|| {
            anyhow!(
                "No output image available for {}",
                self.view.describe(self.last_output.shape())
            )
        })
    }

    /// Publish the displayed image to the viewer.
    fn publish_output_frame(&self) -> anyhow::Result<()> {
        self.output_frame_sender.send_replace(Some(OutputFrame {
            load_generation: self.load_generation,
            coordinates: self.view,
            shape: self.last_output.shape(),
            image: self.get_displayed_image()?,
            is_current: self.output_is_current,
        }));
        Ok(())
    }

    /// Flag the displayed frame as out of date after a failed run, if it is, keeping its image on screen.
    fn flag_stale_output_frame(&self) {
        if self.output_is_current {
            return;
        }
        self.output_frame_sender
            .send_if_modified(|frame| match frame {
                Some(frame) if frame.is_current => {
                    frame.is_current = false;
                    true
                }
                _ => false,
            });
    }

    /// Composite the channels at the displayed position into one image.
    async fn composite_displayed_position(&self) -> anyhow::Result<Arc<RgbaImage>> {
        let channels: Vec<_> = self
            .last_output
            .channels_at(self.view.position())
            .cloned()
            .collect();
        match channels.as_slice() {
            [image] => Ok(image.clone()),
            _ => Ok(Arc::new(
                self.fallback_compositor.composite_images(&channels).await?,
            )),
        }
    }

    /// Convert an output plane from BGRA to RGBA, rejecting GPU failures during conversion.
    async fn convert_output_plane(&mut self, image: &RgbaImage) -> anyhow::Result<RgbaImage> {
        let converted = self
            .colorspace_converter
            .convert_bgra_to_rgba(image)
            .await?;
        self.ensure_no_stage_gpu_errors("Write pipeline output")?;
        Ok(converted)
    }

    /// Return every output plane, named by its coordinates for export.
    fn get_named_views(&self) -> Vec<(String, Arc<RgbaImage>)> {
        let shape = self.last_output.shape();
        shape
            .coordinates()
            .filter_map(|coordinates| {
                let name = coordinates
                    .spanned(shape)
                    .map(|(dimension, number)| {
                        let width = shape.extent(dimension).to_string().len();
                        format!("{}{number:0width$}", dimension.label().to_lowercase())
                    })
                    .collect::<Vec<_>>()
                    .join("_");
                self.last_output
                    .plane(coordinates)
                    .map(|image| (name, image.clone()))
            })
            .collect()
    }

    /// Convert from internal BGRA representation to external file format and write, per `ExportScope` reqs.
    async fn save_output(&mut self, file: &Path, scope: ExportScope) -> anyhow::Result<()> {
        ensure!(
            self.output_is_current,
            "No current pipeline output to save; process the image successfully first"
        );
        self.ensure_no_stage_gpu_errors("Save pipeline output")?;
        let image = match scope {
            ExportScope::CurrentView => self.get_displayed_image()?,
            ExportScope::CompositeView => self.composite_displayed_position().await?,
            ExportScope::AllViews => {
                // Converting and appending one plane at a time holds one RGBA plane, not the stack.
                let mut archive = ImageArchiveWriter::create(file)?;
                for (name, plane) in self.get_named_views() {
                    archive.append(&name, &self.convert_output_plane(&plane).await?)?;
                }
                return archive.finish();
            }
        };
        self.convert_output_plane(&image).await?.save(file)?;
        Ok(())
    }

    /// Apply queued commands, reconcile tool state, and process pending image changes.
    ///
    /// # Errors
    /// Command failures are reported to their reply channels and frontend feedback;
    /// this method currently always returns `Ok(())`.
    #[expect(
        clippy::too_many_lines,
        reason = "Keep ordered command dispatch and its processing barriers together"
    )]
    pub async fn process_commands(&mut self) -> anyhow::Result<()> {
        self.ensure_no_remaining_gpu_errors();
        let commands: Vec<_> =
            std::iter::from_fn(|| self.command_receiver.try_recv().ok()).collect();
        let mut pending_processing_from = PendingProcessing::default();
        for cmd in commands {
            match cmd {
                PipelineCommand::Load {
                    files,
                    arrangement,
                    reply,
                } => {
                    let result = self.load_images(&files, &arrangement);
                    if result.is_ok() {
                        pending_processing_from.request_processing(0);
                    } else {
                        self.finish_pipeline_activity();
                    }
                    self.complete_operation(reply, result, None);
                }
                PipelineCommand::AddTool { kind, reply } => {
                    let result = self.tool_manager.add_tool(&kind).await;
                    if result.is_ok() {
                        self.gesture_owner = None;
                    } else if let Err(error) = &result {
                        self.report_error("Tool initialization failed", error);
                    }
                    let _ = reply.send(result.map_err(|error| format!("{error:#}")));
                }
                PipelineCommand::RemoveTool(id) => {
                    if self.gesture_owner == Some(id) {
                        self.gesture_owner = None;
                    }
                    self.tool_manager.remove_tool(id);
                }
                PipelineCommand::MoveTool { id, before } => self.tool_manager.move_tool(id, before),
                PipelineCommand::SelectTool(id) => {
                    let previous = self.tool_manager.selected_tool;
                    self.tool_manager.select(id);
                    if previous != self.tool_manager.selected_tool {
                        self.gesture_owner = None;
                    }
                }
                PipelineCommand::SetEnabled { id, enabled } => {
                    if let Some(tool) = self.tool_manager.tools.get_mut(&id) {
                        tool.set_enabled(enabled);
                        if !enabled {
                            tool.cancel_interaction();
                            if self.gesture_owner == Some(id) {
                                self.gesture_owner = None;
                            }
                        }
                    }
                }
                PipelineCommand::ToolMessage { id, message } => {
                    if let Some(tool) = self.tool_manager.tools.get_mut(&id)
                        && let Err(error) = tool.handle_message(message)
                    {
                        self.report_error("Tool command failed", &error);
                    }
                    if let Some(tool) = self.tool_manager.tools.get_mut(&id)
                        && !tool.is_enabled()
                    {
                        tool.cancel_interaction();
                        if self.gesture_owner == Some(id) {
                            self.gesture_owner = None;
                        }
                    }
                }
                PipelineCommand::SubscribePipeline(reply) => {
                    self.overlays_dirty |= self.tool_manager.publish();
                    let _ = reply.send(self.tool_manager.subscribe_pipeline_state());
                }
                PipelineCommand::SubscribeTool { id, reply } => {
                    self.overlays_dirty |= self.tool_manager.publish();
                    let _ = reply.send(
                        self.tool_manager
                            .tools
                            .get(&id)
                            .map(ToolInstance::subscribe_state),
                    );
                }
                PipelineCommand::SavePipeline { path, reply } => {
                    let result = self.tool_manager.save_pipeline(&path);
                    self.complete_operation(reply, result, Some("Pipeline saved"));
                }
                PipelineCommand::LoadPipeline { path, cli, reply } => {
                    let result = self.tool_manager.load_pipeline(&path, cli).await;
                    if result.is_ok() {
                        self.gesture_owner = None;
                        pending_processing_from.request_processing(0);
                    }
                    self.complete_operation(reply, result, Some("Pipeline loaded"));
                }
                PipelineCommand::SaveOutput { path, scope, reply } => {
                    let result = match self.flush_processing(&mut pending_processing_from).await {
                        Ok(()) => self.save_output(&path, scope).await,
                        Err(error) => Err(error),
                    };
                    // Save replies already produce GUI feedback, unlike background processing.
                    if let Err(error) = &result {
                        eprintln!("Image save failed: {error:#}");
                    }
                    let _ = reply.send(result.map_err(|e| format!("{e:#}")));
                }
                PipelineCommand::SelectView(coordinates) => {
                    if let Err(error) = self.select_view(coordinates) {
                        self.report_error("Image selection failed", &error);
                    }
                }
                PipelineCommand::RequestOverlays => self.publish_overlays(),
                PipelineCommand::InteractionStart(x, y) => {
                    self.gesture_owner = self.tool_manager.selected_tool;
                    self.route_interaction_input(
                        self.gesture_owner,
                        ToolInput::InteractionStart(x, y),
                    );
                }
                PipelineCommand::HandleClick(x, y) => self.route_interaction_input(
                    self.tool_manager.selected_tool,
                    ToolInput::HandleClick(x, y),
                ),
                PipelineCommand::InteractionMove(x, y) => {
                    self.route_interaction_input(
                        self.gesture_owner,
                        ToolInput::InteractionMove(x, y),
                    );
                }
                PipelineCommand::InteractionEnd => {
                    self.route_interaction_input(self.gesture_owner, ToolInput::InteractionEnd);
                    self.gesture_owner = None;
                }
            }
        }
        if let Err(error) = self.flush_processing(&mut pending_processing_from).await {
            self.report_error("Pipeline processing failed", &error);
        }
        self.ensure_no_remaining_gpu_errors();
        self.flag_stale_output_frame();
        Ok(())
    }
    /// Report a file operation result and send its command reply.
    fn complete_operation(
        &self,
        reply: strawberry_types::CommandResult,
        result: anyhow::Result<()>,
        success: Option<&str>,
    ) {
        match &result {
            Ok(()) => {
                if let Some(text) = success {
                    let _ = self.output_sender.send(FrontendMessage::success(text));
                }
            }
            Err(error) => self.report_error("File operation failed", error),
        }
        let _ = reply.send(result.map_err(|error| format!("{error:#}")));
    }
    /// Load images as BGRA planes in the requested arrangement and invalidate previous processing.
    fn load_images(
        &mut self,
        files: &[PathBuf],
        arrangement: &ImageArrangement,
    ) -> anyhow::Result<()> {
        ensure!(!files.is_empty(), "No input images provided");
        let _ = self
            .output_sender
            .send(FrontendMessage::PipelineProgress(PipelineProgress {
                stage: 0,
                total: 1,
                name: "Loading images".into(),
            }));
        let mut images = Vec::with_capacity(files.len());
        for file in files {
            let mut image = ImageReader::open(file)
                .with_context(|| format!("Failed to open {}", file.display()))?
                .decode()
                .with_context(|| format!("Failed to decode {}", file.display()))?
                .into_rgba8();
            for pixel in image.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            images.push(Arc::new(image));
        }
        let images = ImageCollection::arrange(images, arrangement)?;
        let generation = self
            .load_generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("Load generation exhausted"))?;
        for tool in self.tool_manager.tools.values_mut() {
            tool.cancel_interaction();
            tool.invalidate_cache();
            tool.release_previous_input();
        }
        self.gesture_owner = None;
        self.view = Coordinates::default();
        self.tool_manager.select_position(self.view.position());
        self.last_output = ImageCollection::default();
        self.input_images = images;
        self.load_generation = generation;
        self.output_is_current = false;
        Ok(())
    }
    /// Show another plane and direct per-position tool edits to its position. After a failed
    /// run the plane comes from the last successful output, so edits stay on the plane shown.
    fn select_view(&mut self, coordinates: Coordinates) -> anyhow::Result<()> {
        let shape = self.last_output.shape();
        ensure!(
            shape.contains(coordinates),
            "Image {} is not loaded",
            coordinates.describe(shape)
        );
        if coordinates.position() != self.view.position() {
            self.gesture_owner = None;
            self.tool_manager.select_position(coordinates.position());
        }
        self.view = coordinates;
        self.publish_output_frame()
    }
    /// Route canvas input to the selected enabled tool.
    fn route_interaction_input(&mut self, id: Option<ToolId>, input: ToolInput) {
        if id != self.tool_manager.selected_tool {
            return;
        }
        if let Some(tool) = id.and_then(|id| self.tool_manager.tools.get_mut(&id))
            && tool.is_enabled()
            && let Err(error) = tool.handle_input(input)
        {
            self.report_error("Canvas input failed", &error);
        }
    }
    /// Log an error and publish it as frontend feedback.
    fn report_error(&self, context: &str, error: &anyhow::Error) {
        eprintln!("{context}: {error:#}");
        let _ = self
            .output_sender
            .send(FrontendMessage::error(format!("{context}: {error:#}")));
    }
    /// Mark output stale and discard every tool output cache.
    fn invalidate_output(&mut self) {
        self.output_is_current = false;
        for tool in self.tool_manager.tools.values_mut() {
            tool.invalidate_cached_output();
        }
    }
    /// Reject GPU failures and invalidate output before returning an error.
    fn ensure_no_stage_gpu_errors(&mut self, operation: &str) -> anyhow::Result<()> {
        if let Some(error) = self.gpu.take_uncaptured_error() {
            self.invalidate_output();
            return Err(anyhow::Error::new(error).context(operation.to_owned()));
        }
        if let Err(error) = self.gpu.ensure_device_availability(operation) {
            self.invalidate_output();
            return Err(error);
        }
        Ok(())
    }
    /// Drain uncaptured GPU errors and report them after invalidating output.
    fn ensure_no_remaining_gpu_errors(&mut self) {
        while let Some(error) = self.gpu.take_uncaptured_error() {
            self.invalidate_output();
            self.report_error("GPU failure", &error.into());
            self.finish_pipeline_activity();
        }
    }
    /// Tell the frontend that pipeline activity has ended, including background overlay work.
    fn finish_pipeline_activity(&mut self) {
        self.overlay_activity_shown = false;
        let _ = self.output_sender.send(FrontendMessage::PipelineFinished);
    }
    /// Show a tool's background overlay work as pipeline activity, as when it ran inside a
    /// pipeline stage, and end the activity once no enabled tool has overlay work pending.
    fn publish_overlay_activity(&mut self) {
        let pending = self
            .tool_manager
            .tools
            .values()
            .find(|tool| tool.is_enabled() && tool.has_pending_overlay())
            .map(|tool| format!("{:?}", tool.iam()));
        match pending {
            Some(name) if !self.overlay_activity_shown => {
                self.overlay_activity_shown = true;
                let _ =
                    self.output_sender
                        .send(FrontendMessage::PipelineProgress(PipelineProgress {
                            stage: 0,
                            total: 1,
                            name,
                        }));
            }
            None if self.overlay_activity_shown => self.finish_pipeline_activity(),
            _ => {}
        }
    }
    /// Publish the merged tool overlays to the frontend.
    fn publish_overlays(&self) {
        let _ = self
            .output_sender
            .send(FrontendMessage::ReturnOverlays(self.collect_overlays()));
    }
    /// Publish tool events, watched state changes, and updated overlays.
    fn publish_events(&mut self) {
        for (id, tool) in &mut self.tool_manager.tools {
            self.overlays_dirty |= tool.take_overlay_changes();
            for event in tool.drain_events() {
                if matches!(event, FrontendMessage::SnapToPosition(..))
                    && self.tool_manager.selected_tool != Some(*id)
                {
                    continue;
                }
                let _ = self.output_sender.send(FrontendMessage::ToolEvent {
                    id: *id,
                    event: Box::new(event),
                });
            }
        }
        self.overlays_dirty |= self.tool_manager.publish();
        if std::mem::take(&mut self.overlays_dirty) {
            self.publish_overlays();
        }
    }
    /// Poll background work, reconcile tool changes, and run pending processing.
    async fn flush_processing(&mut self, pending: &mut PendingProcessing) -> anyhow::Result<()> {
        for tool in self.tool_manager.tools.values_mut() {
            if let Err(error) = tool.poll_background() {
                eprintln!("Background task failed: {error:#}");
                let _ = self.output_sender.send(FrontendMessage::error(format!(
                    "Background task failed: {error}"
                )));
            }
        }
        self.tool_manager.reconcile(pending);
        for (index, tool) in self.tool_manager.tools.values_mut().enumerate() {
            if tool.reconcile_state() && tool.is_enabled() && tool.supports_processing() {
                pending.request_processing(index);
            }
        }
        let result = if let Some(index) = pending.take()
            && !self.input_images.is_empty()
        {
            let result = self.process_from(index).await;
            self.finish_pipeline_activity();
            result
        } else {
            Ok(())
        };
        self.publish_events();
        self.publish_overlay_activity();
        result
    }
}
