use std::{sync::Arc, time::Instant};

use futures::future::try_join_all;
use image::{ImageReader, RgbaImage};
use tokio::sync::broadcast;

use anyhow::anyhow;
use strawberry_processing::{colorspace::GpuColorspaceConverter, GpuImageProcessor};

// Import all types from strawberry-types
use strawberry_types::*;

// Import tool implementations from tools module
use tools::*;

use crate::tools::nervetracer::NerveTracer;

pub mod counter;
pub mod nervetracer;
pub mod tools;
pub mod utils;

/// Helper macros for tool registration
macro_rules! create_gpu_tool {
    ($tool:ident, $cmd:expr, $out:expr, $gpu:expr) => {
        Box::new($tool::new($cmd.clone(), $out.clone(), $gpu.clone()).await) as Box<dyn ToolBackend>
    };
}

macro_rules! create_cpu_tool {
    ($tool:ident, $cmd:expr, $out:expr) => {
        Box::new($tool::new($cmd.clone(), $out.clone()).await?) as Box<dyn ToolBackend>
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
    /// Input images to process
    input_images: Vec<RgbaImage>,
    /// Click receivers
    click_receivers: Vec<flume::Sender<PipelineCommand>>,
    /// Last output images
    last_output: Option<RgbaImage>,
    /// Colorspace conversions
    colorspace_converter: GpuColorspaceConverter,
}

impl Pipeline {
    /// Create a new pipeline with the given tools
    pub async fn new() -> anyhow::Result<(
        Self,
        broadcast::Sender<PipelineCommand>,
        broadcast::Receiver<PipelineCommand>,
    )> {
        let (command_tx, command_rx) = broadcast::channel(1000);
        let (output_tx, output_rx) = broadcast::channel(1000);

        let gpu_image_processor = Arc::new(GpuImageProcessor::new().await?);

        let tool_manager = ToolManager::new(
            vec![
                // GPU-accelerated tools
                create_gpu_tool!(Compositor, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(Contrast, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(GaussianBlur, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(LocalMaxima, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(Morphology, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(RollingBall, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(Threshold, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(Watershed, command_tx, output_tx, gpu_image_processor),
                create_cpu_tool!(Counter, command_tx, output_tx),
                create_gpu_tool!(Decomposer, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(Sobel, command_tx, output_tx, gpu_image_processor),
                create_gpu_tool!(Grayscale, command_tx, output_tx, gpu_image_processor),
                Box::new(NerveTracer::new(command_tx.clone(), output_tx.clone())?)
                    as Box<dyn ToolBackend>,
            ],
            command_tx.clone(),
            output_tx.clone(),
        );
        let pipeline = Self {
            tool_manager,
            command_receiver: command_rx,
            output_sender: output_tx,
            input_images: vec![],
            click_receivers: vec![],
            last_output: None,
            colorspace_converter: GpuColorspaceConverter::new(gpu_image_processor.clone()).await,
        };

        Ok((pipeline, command_tx, output_rx))
    }

    /// Collect overlays from all tools
    fn collect_overlays(&mut self) -> overlay::OverlayCollection {
        let mut collection = overlay::OverlayCollection::new();
        for tool in &self.tool_manager.tools {
            if let Some(overlay) = tool.get_overlays() {
                collection.merge(overlay);
            }
        }
        collection
    }

    /// Process images through the pipeline starting from the given index
    pub async fn process_from(&mut self, start_index: usize) -> anyhow::Result<()> {
        let start_time = std::time::Instant::now();
        println!("Got process_from command!");
        if self.input_images.is_empty() {
            return Err(anyhow!("No input images provided"));
        }

        let mut earliest_processing_index = start_index;
        let mut last_valid_cache_index = None;
        let mut images = vec![];

        let now = Instant::now();

        // First pass: find the earliest index where we need to start processing
        for (i, tool) in self.tool_manager.tools.iter().enumerate() {
            if i >= start_index {
                break;
            }

            if tool.is_enabled() && tool.supports_processing() {
                if tool.has_valid_cache() {
                    last_valid_cache_index = Some(i);
                } else {
                    earliest_processing_index = i;
                    break;
                }
            }
        }

        // Get the cached output from the last valid cache
        if let Some(cache_index) = last_valid_cache_index {
            if let Some(cached_output) = self.tool_manager.tools[cache_index].get_cached_output() {
                images = cached_output;
            } else {
                images = self.input_images.clone();
            }
        } else {
            images = self.input_images.clone();
        }

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
        let mut stage = 0;

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
                stage += 1;
                tool.process(&mut images).await?;
                tool.set_cached_output(images.clone());
            }
        }

        if images.len() > 1 {
            println!("OUTPUT CONTAINS >1 IMAGE");
            if let Some(compositor_tool) = self.tool_manager.tools.get_mut(0) {
                //TODO: Very dangerous. Assumes compositor at index 0.
                compositor_tool.process(&mut images).await?;
            }
        }

        let output_image = images
            .first()
            .ok_or_else(|| anyhow::anyhow!("No output image available from pipeline"))?;

        self.output_sender
            .send(PipelineCommand::Output(output_image.clone()))
            .map_err(|e| anyhow::anyhow!("Failed to send output image: {}", e))?;

        self.last_output = Some(output_image.clone());

        let duration = start_time.elapsed();
        println!("Pipeline processing took: {duration:?}");

        Ok(())
    }

    async fn run_pipeline(&mut self, start_index: usize, error_context: &str) {
        let result = self.process_from(start_index).await;
        let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
            FrontendMessage::PipelineFinished,
        ));
        if let Err(e) = result {
            let _ =
                self.output_sender
                    .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                        format!("{error_context}: {e}"),
                    )));
        }
    }

    pub async fn process_commands(&mut self) -> anyhow::Result<()> {
        // Collect all available messages and deduplicate Refresh commands
        let commands: Vec<_> =
            std::iter::from_fn(|| self.command_receiver.try_recv().ok()).collect();

        let commands = if commands.len() > 1 {
            if let Some(last_refresh) = commands
                .iter()
                .rev()
                .find(|cmd| matches!(cmd, PipelineCommand::Refresh(_)))
                .cloned()
            {
                let mut filtered: Vec<_> = commands
                    .into_iter()
                    .filter(|cmd| !matches!(cmd, PipelineCommand::Refresh(_)))
                    .collect();
                filtered.push(last_refresh);
                filtered
            } else {
                commands
            }
        } else {
            commands
        };

        // Process all commands
        for cmd in commands {
            match &cmd {
                PipelineCommand::Refresh(index) => {
                    println!("Refreshing!");
                    if self.input_images.is_empty() {
                        return Ok(());
                    }
                    self.run_pipeline(
                        *index,
                        &format!("Pipeline processing failed at stage {index}"),
                    )
                    .await;
                }
                PipelineCommand::SaveOutput(file) => {
                    if let Some(last_output) = self.last_output.clone() {
                        let out_img = self
                            .colorspace_converter
                            .convert_bgra_to_rgba(&last_output)
                            .await?;
                        out_img.save(file)?;
                        self.output_sender.send(PipelineCommand::FrontendMessage(
                            FrontendMessage::success("Image saved successfully!".to_string()),
                        ))?;
                    } else {
                        self.output_sender.send(PipelineCommand::FrontendMessage(
                            FrontendMessage::error(
                                "No image in output buffer to save!".to_string(),
                            ),
                        ))?;
                    }
                }
                PipelineCommand::Load(files) => {
                    let mut loaded_images = vec![];
                    let mut sizes = vec![];
                    //TODO: Maybe we should do this in a separate thread?
                    println!("Loading images: {:?}", files);
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
                                    let data = decoded.to_rgba8();
                                    sizes.push(data.dimensions());
                                    loaded_images.push(data);
                                }
                                Err(e) => {
                                    eprintln!("Error decoding image: {}", e);
                                    let _ =
                                        self.output_sender.send(PipelineCommand::FrontendMessage(
                                            FrontendMessage::PipelineFinished,
                                        ));
                                    self.output_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::error(
                                            "Failed to decode image!".to_string(),
                                        ),
                                    ))?;
                                    return Ok(());
                                }
                            },
                            Err(e) => {
                                eprintln!("Error loading image: {}", e);
                                let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::PipelineFinished,
                                ));
                                self.output_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error("Failed to load image!".to_string()),
                                ))?;
                                return Ok(());
                            }
                        }
                    }
                    // Check that all image dimensions match
                    if !sizes.is_empty() {
                        let first_size = sizes[0];
                        if !sizes.iter().all(|&size| size == first_size) {
                            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::PipelineFinished,
                            ));
                            self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::error(
                                    "Error: All images must have the same dimensions".to_string(),
                                ),
                            ))?;
                            return Ok(());
                        }
                    }
                    //
                    if loaded_images.len() > 1 {
                        self.tool_manager.tools[0].set_enabled(true);
                    } else if !loaded_images.is_empty() {
                        let mut rgba_image = loaded_images[0].clone();
                        // Convert RGBA to BGRA by swapping R and B channels
                        for pixel in rgba_image.pixels_mut() {
                            let temp = pixel[0]; // Store R
                            pixel[0] = pixel[2]; // R = B
                            pixel[2] = temp; // B = R
                        }
                        loaded_images[0] = rgba_image;
                    }
                    self.input_images = loaded_images;

                    println!("Images loaded");

                    self.run_pipeline(0, "Failed to process images").await;
                }
                PipelineCommand::DiscoverBackend(backend_tool_type, sender) => {
                    let mut discovered = vec![];
                    let mut channels = vec![];
                    let mut i = 0;

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
                        return Ok(());
                    }

                    for tool in self.tool_manager.tools.iter_mut() {
                        let iam = tool.iam();
                        if iam == *backend_tool_type {
                            discovered.push(i as u16);
                            tool.add_frontend_sender(sender.clone());
                            channels.push(tool.get_sender());
                            i += 1;
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
                        return Ok(());
                    }
                    for tool in self.tool_manager.tools.iter_mut() {
                        if tool.iam() == *backend_tool_type {
                            tool.disconnect();
                        }
                    }
                }
                PipelineCommand::CLILoadPipeline(path) => {
                    let toml_content = std::fs::read_to_string(path)
                        .map_err(|e| anyhow::anyhow!("Failed to read file: {}", e))?;
                    let pipeline_config: PipelineConfig = toml::from_str(&toml_content)
                        .map_err(|e| anyhow::anyhow!("Failed to parse TOML: {}", e))?;

                    // Apply the loaded configurations to the tools
                    for tool in self.tool_manager.tools.iter_mut() {
                        let tool_name = format!("{:?}", tool.iam());
                        if let Some(config) = pipeline_config.tools.get(&tool_name) {
                            tool.apply_state(config.clone());
                        }
                    }
                    println!("CLI Pipeline load");
                    self.run_pipeline(0, "Failed to process loaded pipeline")
                        .await;
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
            };
        }

        let futures = self
            .tool_manager
            .tools
            .iter_mut()
            .map(|tool| tool.receive_messages());
        if let Err(e) = try_join_all(futures).await {
            let _ =
                self.output_sender
                    .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                        format!("Error processing tool messages: {e}"),
                    )));
        }

        if let Err(e) = self.tool_manager.receive_messages().await {
            let _ =
                self.output_sender
                    .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                        format!("Tool manager communication error: {e}"),
                    )));
        }

        Ok(())
    }
}
