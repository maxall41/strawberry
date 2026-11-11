// wgpu auto-trait checks exceed the default compiler recursion depth.
#![recursion_limit = "256"]

use std::{sync::Arc, time::Instant};

use image::{ImageReader, RgbaImage};
use tokio::sync::{broadcast, watch};

use anyhow::anyhow;
use strawberry_processing::{
    GpuImageProcessor, colorspace::GpuColorspaceConverter, compositor::GpuCompositor,
};

// Import all types from strawberry-types
use strawberry_types::{
    BackendToolType, FrontendMessage, OutputFrame, PendingProcessing, PipelineCommand,
    PipelineProgress, ReconcilliationTicket, ValidReconcilliationTicket, overlay,
};

// Import tool implementations from tools module
use tools::{
    Compositor, Contrast, Counter, Decomposer, DifferenceOfGaussians, GaussianBlur, Grayscale,
    LocalMaxima, Morphology, RollingBall, Sobel, Threshold, ToolManager, Watershed,
};

use crate::tools::nervetracer::NerveTracer;

pub mod counter;
pub mod nervetracer;
mod pipeline_io;
pub mod tools;

/// Helper macros for tool registration
macro_rules! create_gpu_tool {
    ($tool:ident, $out:expr, $gpu:expr) => {
        Box::new($tool::new($out.clone(), $gpu.clone()))
    };
}

macro_rules! create_cpu_tool {
    ($tool:ident, $cmd:expr, $out:expr) => {
        Box::new($tool::new($cmd.clone(), $out.clone())?)
    };
}

/// Pipeline that processes images through a series of tools
pub struct Pipeline {
    /// Tools in the pipeline, processed in order
    tool_manager: ToolManager,
    /// Channel for receiving commands
    command_receiver: broadcast::Receiver<PipelineCommand>,
    /// Channel for sending output results
    output_sender: broadcast::Sender<PipelineCommand>,
    /// Latest-only channel for output frames, so the broadcast ring never pins full-resolution images
    output_frame_sender: watch::Sender<Option<OutputFrame>>,
    /// Bumped per Load command; published with each frame so consumers can match results to the load that produced them
    load_generation: u64,
    /// Input images to process
    input_images: Vec<Arc<RgbaImage>>,
    /// Click receivers
    click_receivers: Vec<flume::Sender<PipelineCommand>>,
    /// Last output images
    last_output: Option<Arc<RgbaImage>>,
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
        broadcast::Sender<PipelineCommand>,
        broadcast::Receiver<PipelineCommand>,
        watch::Receiver<Option<OutputFrame>>,
    )> {
        let (command_tx, command_rx) = broadcast::channel(1000);
        let (output_tx, output_rx) = broadcast::channel(1000);
        let (frame_tx, frame_rx) = watch::channel(None);

        let gpu_image_processor = Arc::new(GpuImageProcessor::new().await?);

        let tool_manager = ToolManager::new(
            vec![
                Box::new(tools::automatic_segmentation::AutomaticSegmentation::new(
                    command_tx.clone(),
                    output_tx.clone(),
                    gpu_image_processor.clone(),
                )),
                // GPU-accelerated tools
                create_gpu_tool!(Compositor, output_tx, gpu_image_processor),
                create_gpu_tool!(Contrast, output_tx, gpu_image_processor),
                create_gpu_tool!(GaussianBlur, output_tx, gpu_image_processor),
                create_gpu_tool!(LocalMaxima, output_tx, gpu_image_processor),
                create_gpu_tool!(Morphology, output_tx, gpu_image_processor),
                create_gpu_tool!(RollingBall, output_tx, gpu_image_processor),
                create_gpu_tool!(Threshold, output_tx, gpu_image_processor),
                create_gpu_tool!(Watershed, output_tx, gpu_image_processor),
                create_cpu_tool!(Counter, command_tx, output_tx),
                create_gpu_tool!(Decomposer, output_tx, gpu_image_processor),
                create_gpu_tool!(Sobel, output_tx, gpu_image_processor),
                create_gpu_tool!(Grayscale, output_tx, gpu_image_processor),
                Box::new(NerveTracer::new(command_tx.clone(), output_tx.clone())?),
                create_gpu_tool!(DifferenceOfGaussians, output_tx, gpu_image_processor),
            ],
            output_tx.clone(),
        );
        let pipeline = Self {
            tool_manager,
            command_receiver: command_rx,
            output_sender: output_tx,
            output_frame_sender: frame_tx,
            load_generation: 0,
            input_images: vec![],
            click_receivers: vec![],
            last_output: None,
            fallback_compositor: GpuCompositor::new(gpu_image_processor.clone()),
            colorspace_converter: GpuColorspaceConverter::new(gpu_image_processor),
        };

        Ok((pipeline, command_tx, output_rx, frame_rx))
    }

    /// Collect overlays from all tools
    fn collect_overlays(&self) -> overlay::OverlayCollection {
        let mut collection = overlay::OverlayCollection::new();
        for tool in &self.tool_manager.tools {
            if let Some(overlay) = tool.get_overlays() {
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
        for (i, tool) in self.tool_manager.tools.iter().enumerate() {
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
            .iter_mut()
            .skip(earliest_processing_index)
        {
            tool.invalidate_cache();
        }

        let total_stages = self
            .tool_manager
            .tools
            .iter()
            .skip(earliest_processing_index)
            .filter(|tool| tool.is_enabled() && tool.supports_processing())
            .count();
        let mut stage: usize = 0;

        for tool in self
            .tool_manager
            .tools
            .iter_mut()
            .skip(earliest_processing_index)
        {
            if tool.is_enabled() && tool.supports_processing() {
                let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                    FrontendMessage::PipelineProgress(PipelineProgress {
                        stage,
                        total: total_stages,
                        name: format!("{:?}", tool.iam()),
                    }),
                ));
                stage = stage.saturating_add(1);
                let result = tool.process(&mut images).await;
                tool.acknowledge_processed_state();
                result?;
                tool.set_cached_output(images.clone());
            }
        }

        if images.len() > 1 {
            println!("OUTPUT CONTAINS >1 IMAGE");
            if let Some(compositor_tool) = self
                .tool_manager
                .tools
                .iter_mut()
                .find(|tool| tool.iam() == BackendToolType::Compositor && tool.is_enabled())
            {
                let result = compositor_tool.process(&mut images).await;
                compositor_tool.acknowledge_processed_state();
                result?;
            } else {
                let composited = self.fallback_compositor.composite_images(&images).await?;
                images = vec![Arc::new(composited)];
            }
        }

        let output_image = images
            .first()
            .ok_or_else(|| anyhow::anyhow!("No output image available from pipeline"))?;

        self.output_frame_sender.send_replace(Some(OutputFrame {
            load_generation: self.load_generation,
            image: output_image.clone(),
        }));

        self.last_output = Some(output_image.clone());

        let duration = start_time.elapsed();
        println!("Pipeline processing took: {duration:?}");

        Ok(())
    }

    /// Convert the last output back out of the internal BGRA representation and
    /// write it to `file`.
    async fn save_output(&self, file: &std::path::Path) -> anyhow::Result<()> {
        let last_output = self
            .last_output
            .as_ref()
            .ok_or_else(|| anyhow!("no image in the output buffer"))?;
        let out_img = self
            .colorspace_converter
            .convert_bgra_to_rgba(last_output)
            .await?;
        out_img.save(file)?;
        Ok(())
    }

    /// # Errors
    /// Returns an error if configuration, image processing, file I/O, or communication fails.
    #[expect(
        clippy::too_many_lines,
        reason = "Command dispatch as one function is cleaner"
    )]
    pub async fn process_commands(&mut self) -> anyhow::Result<()> {
        let commands: Vec<_> =
            std::iter::from_fn(|| self.command_receiver.try_recv().ok()).collect();
        let mut pending_processing_from = PendingProcessing::default();

        'commands: for cmd in commands {
            match &cmd {
                PipelineCommand::SaveOutput(file) => {
                    self.flush_processing(&mut pending_processing_from).await;
                    let message = match self.save_output(file).await {
                        Ok(()) => FrontendMessage::success("Image saved successfully!".to_string()),
                        Err(e) => FrontendMessage::error(format!("Failed to save image: {e}")),
                    };
                    let _ = self
                        .output_sender
                        .send(PipelineCommand::FrontendMessage(message));
                }
                PipelineCommand::Load(files) => {
                    self.flush_processing(&mut pending_processing_from).await;
                    // Cache is cleared later during process_from anyways, but clearing here first reduces peak memory.
                    for tool in &mut self.tool_manager.tools {
                        tool.invalidate_cache();
                    }

                    let mut loaded_images = vec![];
                    let mut sizes = vec![];

                    println!("Loading images: {files:?}");
                    let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                        FrontendMessage::PipelineProgress(PipelineProgress {
                            stage: 0,
                            total: 1,
                            name: "Loading images".to_string(),
                        }),
                    ));
                    for file in files {
                        match ImageReader::open(file) {
                            Ok(reader) => match reader.decode() {
                                Ok(decoded) => {
                                    let data = decoded.into_rgba8();
                                    sizes.push(data.dimensions());
                                    loaded_images.push(data);
                                }
                                Err(e) => {
                                    eprintln!("Error decoding image: {e}");
                                    let _ =
                                        self.output_sender.send(PipelineCommand::FrontendMessage(
                                            FrontendMessage::PipelineFinished,
                                        ));
                                    let _ =
                                        self.output_sender.send(PipelineCommand::FrontendMessage(
                                            FrontendMessage::error(format!(
                                                "Failed to decode image {}: {e}",
                                                file.display()
                                            )),
                                        ));
                                    continue 'commands;
                                }
                            },
                            Err(e) => {
                                eprintln!("Error loading image: {e}");
                                let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::PipelineFinished,
                                ));
                                let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(format!(
                                        "Failed to load image {}: {e}",
                                        file.display()
                                    )),
                                ));
                                continue 'commands;
                            }
                        }
                    }
                    // Check that all image dimensions match
                    if !sizes.is_empty() {
                        let first_size = sizes.first().copied().unwrap_or_default();
                        if !sizes.iter().all(|&size| size == first_size) {
                            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::PipelineFinished,
                            ));
                            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::error(
                                    "Error: All images must have the same dimensions".to_string(),
                                ),
                            ));
                            continue 'commands;
                        }
                    }
                    //
                    if loaded_images.len() > 1
                        && let Some(compositor_tool) = self
                            .tool_manager
                            .tools
                            .iter_mut()
                            .find(|tool| tool.iam() == BackendToolType::Compositor)
                    {
                        compositor_tool.set_enabled(true);
                    }
                    // Convert all images from RGBA to BGRA by swapping R and B channels
                    for image in &mut loaded_images {
                        for pixel in image.pixels_mut() {
                            pixel.0.swap(0, 2);
                        }
                    }
                    //
                    self.input_images = loaded_images.into_iter().map(Arc::new).collect();
                    self.load_generation = self
                        .load_generation
                        .checked_add(1)
                        .ok_or_else(|| anyhow!("Load generation exhausted"))?;

                    println!("Images loaded");

                    pending_processing_from.request_processing(0);
                }
                PipelineCommand::DiscoverBackend(backend_tool_type, sender) => {
                    let mut discovered = vec![];
                    let mut channels = vec![];
                    let mut i = 0u16;

                    if *backend_tool_type == BackendToolType::Manager {
                        self.tool_manager.add_frontend_sender(sender.clone());
                        let r = FrontendMessage::ReturnBackendTools(
                            vec![0],
                            vec![self.tool_manager.get_sender()],
                        );
                        if let Err(e) = sender.send(PipelineCommand::FrontendMessage(r)) {
                            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::error(format!(
                                    "Communication error with frontend: {e}"
                                )),
                            ));
                        }
                        continue 'commands;
                    }

                    for tool in &mut self.tool_manager.tools {
                        let iam = tool.iam();
                        if iam == *backend_tool_type {
                            discovered.push(i);
                            tool.add_frontend_sender(sender.clone());
                            channels.push(tool.get_sender());
                            i = i
                                .checked_add(1)
                                .ok_or_else(|| anyhow!("Too many backend instances"))?;
                        }
                    }

                    let r = FrontendMessage::ReturnBackendTools(discovered, channels);

                    println!("Returning discovery request {r:?}");

                    if let Err(e) = sender.send(PipelineCommand::FrontendMessage(r)) {
                        let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                            FrontendMessage::error(format!(
                                "Communication error with frontend: {e}"
                            )),
                        ));
                    }
                }
                PipelineCommand::DisconnectFrontend(backend_tool_type) => {
                    if *backend_tool_type == BackendToolType::Manager {
                        self.tool_manager.disconnect();
                        continue 'commands;
                    }
                    for tool in &mut self.tool_manager.tools {
                        if tool.iam() == *backend_tool_type {
                            tool.disconnect();
                        }
                    }
                }
                PipelineCommand::CLILoadPipeline(path) => {
                    if let Err(error) = self.tool_manager.load_pipeline(path, true) {
                        self.report_error("Failed to load pipeline", &error);
                        continue 'commands;
                    }
                    println!("CLI Pipeline load");
                    pending_processing_from.request_processing(0);
                }
                PipelineCommand::RegisterClickReceiver(click_sender) => {
                    self.click_receivers.push(click_sender.clone()); //TODO: Ehhhhh
                }
                PipelineCommand::HandleClick(image_pixel_x, image_pixel_y) => {
                    for recv in &self.click_receivers {
                        if let Err(e) =
                            recv.send(PipelineCommand::HandleClick(*image_pixel_x, *image_pixel_y))
                        {
                            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::error(format!(
                                    "Failed to process click event: {e}"
                                )),
                            ));
                        }
                    }
                }
                PipelineCommand::RequestOverlays => {
                    let overlays = self.collect_overlays();
                    if let Err(e) = self.output_sender.send(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnOverlays(overlays),
                    )) {
                        eprintln!("Failed to send overlays: {e}");
                    }
                }
                PipelineCommand::InteractionStart(x, y) => {
                    // Route interaction start to all click receivers (tools can filter based on their mode)
                    for recv in &self.click_receivers {
                        let _ = recv.send(PipelineCommand::InteractionStart(*x, *y));
                    }
                }
                PipelineCommand::InteractionMove(x, y) => {
                    // Route interaction move to all click receivers
                    for recv in &self.click_receivers {
                        let _ = recv.send(PipelineCommand::InteractionMove(*x, *y));
                    }
                }
                PipelineCommand::InteractionEnd => {
                    // Route interaction end to all click receivers
                    for recv in &self.click_receivers {
                        let _ = recv.send(PipelineCommand::InteractionEnd);
                    }
                }
                _ => {}
            }
        }

        self.flush_processing(&mut pending_processing_from).await;
        Ok(())
    }

    fn report_error(&self, context: &str, error: &anyhow::Error) {
        let _ = self
            .output_sender
            .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                format!("{context}: {error}"),
            )));
    }

    fn reconcile_tools(&mut self, pending: &mut PendingProcessing) -> ReconcilliationTicket {
        for (index, tool) in self.tool_manager.tools.iter_mut().enumerate() {
            // The manager schedules removed stages; still reconcile disabled tools to clear stale caches.
            if tool.reconcile_state() && tool.is_enabled() && tool.supports_processing() {
                pending.request_processing(index);
            }
        }
        ReconcilliationTicket {
            index: pending.take(),
        }
    }

    /// A batch boundary: apply received mutations, reconcile tool state with last pipeline run, then process and publish frames if needed.
    async fn flush_processing(&mut self, pending: &mut PendingProcessing) {
        // Keep draining other tools even when one rejects a message.
        for tool in &mut self.tool_manager.tools {
            if let Err(error) = tool.receive_messages() {
                let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                    FrontendMessage::error(format!("Error processing tool messages: {error}")),
                ));
            }
        }
        if let Err(error) = self.tool_manager.receive_messages(pending) {
            self.report_error("Tool manager communication error", &error);
        }
        if let Some(ticket) = self.reconcile_tools(pending).validate()
            && !self.input_images.is_empty()
        {
            let result = self.process_from(ticket).await;
            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                FrontendMessage::PipelineFinished,
            ));
            if let Err(error) = result {
                self.report_error("Pipeline processing failed", &error);
            }
        }
        for tool in &mut self.tool_manager.tools {
            if let Err(error) = tool.sync_frontend() {
                let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                    FrontendMessage::error(format!("Tool state synchronization failed: {error}")),
                ));
            }
        }
    }
}
