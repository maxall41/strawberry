// wgpu auto-trait checks exceed the default compiler recursion depth.
#![recursion_limit = "256"]

use std::{sync::Arc, time::Instant};

use image::{ImageReader, RgbaImage};
use tokio::sync::{broadcast, mpsc, watch};

use anyhow::{Context as _, anyhow, ensure};
use strawberry_processing::{
    GpuImageProcessor, colorspace::GpuColorspaceConverter, compositor::GpuCompositor,
};

// Import all types from strawberry-types
use strawberry_types::{
    CommandSender, FrontendMessage, OutputFrame, PendingProcessing, PipelineCommand,
    PipelineProgress, ReconcilliationTicket, ToolId, ToolInput, ValidReconcilliationTicket,
    overlay,
};

use tools::{ToolManager, manager::ToolInstance};

pub mod counter;
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
    input_images: Vec<Arc<RgbaImage>>,
    /// Captured owner of the current canvas gesture
    gesture_owner: Option<ToolId>,
    overlays_dirty: bool,
    /// Last output images
    last_output: Option<Arc<RgbaImage>>,
    /// The canvas may retain an older successful frame after a failed run.
    output_is_current: bool,
    gpu: Arc<GpuImageProcessor>,
    /// Colorspace conversions
    colorspace_converter: GpuColorspaceConverter,
    /// Composite all remaining layers independently, if the user compositor is disabled and multiple images remain.
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
            input_images: vec![],
            gesture_owner: None,
            overlays_dirty: true,
            last_output: None,
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
    async fn process_from(&mut self, ticket: ValidReconcilliationTicket) -> anyhow::Result<()> {
        self.output_is_current = false;
        let start_time = std::time::Instant::now();
        println!("Got process_from command!");
        if self.input_images.is_empty() {
            return Err(anyhow!("No input images provided"));
        }

        let start_index = ticket.take();
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

        if images.len() > 1 {
            println!("OUTPUT CONTAINS >1 IMAGE");
            let composited = self.fallback_compositor.composite_images(&images).await?;
            images = vec![Arc::new(composited)];
        }

        let output_image = images
            .first()
            .ok_or_else(|| anyhow::anyhow!("No output image available from pipeline"))?;

        self.ensure_no_stage_gpu_errors("Publish pipeline output")?;
        self.output_frame_sender.send_replace(Some(OutputFrame {
            load_generation: self.load_generation,
            image: output_image.clone(),
        }));

        self.last_output = Some(output_image.clone());
        self.output_is_current = true;

        let duration = start_time.elapsed();
        println!("Pipeline processing took: {duration:?}");

        Ok(())
    }

    /// Convert the last output back out of the internal BGRA representation and
    /// write it to `file`.
    async fn save_output(&mut self, file: &std::path::Path) -> anyhow::Result<()> {
        ensure!(
            self.output_is_current,
            "No current pipeline output to save; process the image successfully first"
        );
        self.ensure_no_stage_gpu_errors("Save pipeline output")?;
        let last_output = self
            .last_output
            .as_ref()
            .ok_or_else(|| anyhow!("no image in the output buffer"))?;
        let out_img = self
            .colorspace_converter
            .convert_bgra_to_rgba(last_output)
            .await?;
        self.ensure_no_stage_gpu_errors("Write pipeline output")?;
        out_img.save(file)?;
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
                PipelineCommand::Load { files, reply } => {
                    let result = self.load_images(&files);
                    if result.is_ok() {
                        pending_processing_from.request_processing(0);
                    } else {
                        let _ = self.output_sender.send(FrontendMessage::PipelineFinished);
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
                PipelineCommand::SaveOutput { path, reply } => {
                    let result = match self.flush_processing(&mut pending_processing_from).await {
                        Ok(()) => self.save_output(&path).await,
                        Err(error) => Err(error),
                    };
                    // Save replies already produce GUI feedback, unlike background processing.
                    if let Err(error) = &result {
                        eprintln!("Image save failed: {error:#}");
                    }
                    let _ = reply.send(result.map_err(|e| format!("{e:#}")));
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
        Ok(())
    }
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
    fn load_images(&mut self, files: &[std::path::PathBuf]) -> anyhow::Result<()> {
        use anyhow::{Context as _, ensure};
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
            if let Some(first) = images.first() {
                ensure!(
                    image.dimensions() == RgbaImage::dimensions(first),
                    "All images must have the same dimensions"
                );
            }
            for pixel in image.pixels_mut() {
                pixel.0.swap(0, 2);
            }
            images.push(image);
        }
        let generation = self
            .load_generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("Load generation exhausted"))?;
        for tool in self.tool_manager.tools.values_mut() {
            tool.cancel_interaction();
            tool.invalidate_cache();
        }
        self.gesture_owner = None;
        self.input_images = images.into_iter().map(Arc::new).collect();
        self.load_generation = generation;
        self.output_is_current = false;
        Ok(())
    }
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
    fn report_error(&self, context: &str, error: &anyhow::Error) {
        eprintln!("{context}: {error:#}");
        let _ = self
            .output_sender
            .send(FrontendMessage::error(format!("{context}: {error:#}")));
    }
    fn invalidate_output(&mut self) {
        self.output_is_current = false;
        for tool in self.tool_manager.tools.values_mut() {
            tool.invalidate_cached_output();
        }
    }
    fn ensure_no_stage_gpu_errors(&mut self, operation: &str) -> anyhow::Result<()> {
        if let Err(error) = self.gpu.ensure_device_availability(operation) {
            self.invalidate_output();
            return Err(error);
        }
        if let Some(error) = self.gpu.take_uncaptured_error() {
            self.invalidate_output();
            return Err(anyhow::Error::new(error).context(operation.to_owned()));
        }
        Ok(())
    }
    fn ensure_no_remaining_gpu_errors(&mut self) {
        while let Some(error) = self.gpu.take_uncaptured_error() {
            self.invalidate_output();
            self.report_error("GPU failure", &error.into());
            let _ = self.output_sender.send(FrontendMessage::PipelineFinished);
        }
    }
    fn publish_overlays(&self) {
        let _ = self
            .output_sender
            .send(FrontendMessage::ReturnOverlays(self.collect_overlays()));
    }
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
        let ticket = ReconcilliationTicket {
            index: pending.take(),
        };
        let result = if let Some(ticket) = ticket.validate()
            && !self.input_images.is_empty()
        {
            let result = self.process_from(ticket).await;
            let _ = self.output_sender.send(FrontendMessage::PipelineFinished);
            result
        } else {
            Ok(())
        };
        self.publish_events();
        result
    }
}
