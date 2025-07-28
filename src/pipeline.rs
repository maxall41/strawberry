use core::fmt;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

use async_trait::async_trait;
use compositor::{Compositor, CompositorState};
use decomposer::{Decomposer, DecomposerState};
use serde::{Deserialize, Serialize};

use crate::processing::local_maxima::ThresholdType;
use anyhow::anyhow;
use contrast::{Contrast, ContrastState};
use counter::{Counter, CounterState, CounterTool};
use gaussian_blur::{GaussianBlur, GaussianBlurState};
use grayscale::{Grayscale, GrayscaleState};
use image::{ImageReader, RgbaImage};
use local_maxima::{LocalMaxima, LocalMaximaState};
use manager::{ManagerState, PipelineConfig, ToolManager};
use morphology::{Morphology, MorphologyState};
use rolling_ball::{RollingBall, RollingBallState};
use sobel::{Sobel, SobelState};
use watershed::{Watershed, WatershedState};

use futures::future::try_join_all;
use threshold::{Threshold, ThresholdState};
use tokio::sync::broadcast;

use crate::processing::GpuImageProcessor;

pub mod compositor;
pub mod contrast;
pub mod counter;
pub mod decomposer;
pub mod gaussian_blur;
pub mod grayscale;
pub mod local_maxima;
pub mod manager;
pub mod morphology;
pub mod rolling_ball;
pub mod sobel;
pub mod threshold;
pub mod watershed;

#[derive(Debug, Clone)]
pub enum CompositorMessage {
    ToggleVisibility(bool, u16),
}

#[derive(Debug, Clone)]
pub enum DecomposerMessage {
    ToggleVisibility(bool, u16),
}

#[derive(Debug, Clone)]
pub enum ContrastMessage {
    SetMinMax(f32, f32),
}

#[derive(Debug, Clone)]
pub enum GaussianBlurMessage {
    SetRadiusSigma(f32, f32),
}

#[derive(Debug, Clone)]
pub enum LocalMaximaMessage {
    SetParameters(u32, ThresholdType, bool), // min_distance, threshold, exclude_border
}

#[derive(Debug, Clone)]
pub enum MorphologyMessage {
    SetOperation(crate::processing::morphology::MorphologyOperation),
    SetElementSize(u32),
}

#[derive(Debug, Clone)]
pub enum ManagerMessage {
    ReorderTool(u16, u16),
    SavePipeline(PathBuf),
    LoadPipeline(PathBuf),
}

#[derive(Debug, Clone)]
pub enum RollingBallMessage {
    SetBallRadius(u32),
}

#[derive(Debug, Clone)]
pub enum ThresholdMessage {
    SetThresholdValue(f32),
}

#[derive(Debug, Clone)]
pub enum WatershedMessage {
    SetParameters(f32, u32), // sigma, min_distance
}

#[derive(Debug, Clone)]
pub enum SobelMessage {
    SetThreshold(f32),
    ToggleThreshold(bool),
}

#[derive(Debug, Clone)]
pub struct Polygon {
    vertices: Vec<(f32, f32)>,
}

#[derive(Debug, Clone)]
pub enum FilterMessage {
    ConfigureSizeFilter(u16), // Min size
}

#[derive(Debug, Clone)]
pub enum CountMessage {
    NewCountType(u16),
    DeleteCountType(u16),
    SetMode(counter::CounterMode),
    SetActiveCountType(u16),
    SetActiveTool(CounterTool),
    ToggleFilter(u16, bool),
    FilterMessage(FilterMessage),
    SaveCounts(PathBuf),
    LoadCounts(PathBuf),
    CopyToManual,
    ReviewNavigatePrevious,
    ReviewNavigateNext,
    DeletionLasso(Vec<(u64, u64)>),
}

#[derive(Debug, Clone)]
pub enum PipelineObjectMessage {
    DecomposerMessage(DecomposerMessage),
    CompositorMessage(CompositorMessage),
    ContrastMessage(ContrastMessage),
    GaussianBlurMessage(GaussianBlurMessage),
    LocalMaximaMessage(LocalMaximaMessage),
    MorphologyMessage(MorphologyMessage),
    RollingBallMessage(RollingBallMessage),
    SobelMessage(SobelMessage),
    ThresholdMessage(ThresholdMessage),
    WatershedMessage(WatershedMessage),
    ToggleActive(bool),
    ManagerMessage(ManagerMessage),
    CountMessage(CountMessage),
}

#[derive(Debug, Clone)]
pub enum FrontendMessage {
    ReturnState(ToolState),
    ReturnBackendTools(Vec<u16>, Vec<flume::Sender<PipelineCommand>>),
    UserFeedback(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendToolType {
    Compositor,
    Counter,
    Contrast,
    GaussianBlur,
    Grayscale,
    LocalMaxima,
    Manager,
    Morphology,
    RollingBall,
    Sobel,
    Threshold,
    Watershed,
    Decomposer,
}
impl fmt::Display for BackendToolType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Commands that can be sent to the pipeline
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// Refresh the pipeline starting from the given tool index
    Refresh(usize),
    /// Load input images into the pipeline
    Load(Vec<PathBuf>, bool),
    /// Return output image from the pipeline
    Output(RgbaImage),
    /// Message to pipeline object
    PipelineObject(PipelineObjectMessage),
    /// Message to frontend
    FrontendMessage(FrontendMessage),
    /// Request state from pipeline object
    RequestState(),
    /// Discover
    DiscoverBackend(BackendToolType, flume::Sender<PipelineCommand>),
    /// Disconnect frontend tool
    DisconnectFrontend(BackendToolType),
    /// Handle click at x and y image pixels
    HandleClick(u64, u64),
    /// Register click receiver
    RegisterClickReceiver(flume::Sender<PipelineCommand>),
    /// Command used by CLI to avoid discovery negoitation
    CLILoadPipeline(PathBuf),
}

/// Result of pipeline processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolState {
    CompositorState(CompositorState),
    ContrastState(ContrastState),
    GaussianBlurState(GaussianBlurState),
    GrayscaleState(GrayscaleState),
    LocalMaximaState(LocalMaximaState),
    ManagerState(ManagerState),
    MorphologyState(MorphologyState),
    RollingBallState(RollingBallState),
    SobelState(SobelState),
    ThresholdState(ThresholdState),
    CounterState(CounterState),
    DecomposerState(DecomposerState),
    WatershedState(WatershedState),
}

/// Trait for image processing tools
///Todo: seperate tool from backend proc trait everything except process
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

        let gpu_image_processor = Arc::new(GpuImageProcessor::new().await);

        let tool_manager = ToolManager::new(
            vec![
                Box::new(
                    Compositor::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    Contrast::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    GaussianBlur::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    LocalMaxima::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    Morphology::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    RollingBall::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    Threshold::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    Watershed::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(Counter::new(command_tx.clone(), output_tx.clone()).await?),
                Box::new(
                    Decomposer::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    Sobel::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
                Box::new(
                    Grayscale::new(
                        command_tx.clone(),
                        output_tx.clone(),
                        gpu_image_processor.clone(),
                    )
                    .await,
                ),
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
        };

        Ok((pipeline, command_tx, output_rx))
    }

    /// Process images through the pipeline starting from the given index
    pub async fn process_from(&mut self, start_index: usize) -> anyhow::Result<()> {
        let start_time = std::time::Instant::now();
        println!("Got process_from command!");
        if self.input_images.len() == 0 {
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
        println!("Catching up on cache: {:.2?}", elapsed);

        for tool in self
            .tool_manager
            .tools
            .iter_mut()
            .skip(earliest_processing_index)
        {
            tool.invalidate_cache();
        }

        for tool in self
            .tool_manager
            .tools
            .iter_mut()
            .skip(earliest_processing_index)
        {
            if tool.is_enabled() && tool.supports_processing() {
                tool.process(&mut images).await?;
                tool.set_cached_output(images.clone());
            }
        }

        if images.len() > 1 {
            if let Some(compositor_tool) = self.tool_manager.tools.get_mut(0) {
                compositor_tool.process(&mut images).await?;
            }
        }

        let output_image = images
            .get(0)
            .ok_or_else(|| anyhow::anyhow!("No output image available from pipeline"))?;

        self.output_sender
            .send(PipelineCommand::Output(output_image.clone()))
            .map_err(|e| anyhow::anyhow!("Failed to send output image: {}", e))?;

        let duration = start_time.elapsed();
        println!("Pipeline processing took: {:?}", duration);

        Ok(())
    }

    /// Process all pending commands
    pub async fn process_commands(&mut self) -> anyhow::Result<()> {
        if let Ok(cmd) = self.command_receiver.try_recv() {
            match &cmd {
                PipelineCommand::Refresh(index) => {
                    if let Err(e) = self.process_from(*index).await {
                        let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                            FrontendMessage::UserFeedback(format!(
                                "Pipeline processing failed at stage {}: {}",
                                index, e
                            )),
                        ));
                    }
                }
                PipelineCommand::Load(files, skip_processing) => {
                    println!("Got load command!");
                    let mut loaded_images = vec![];
                    //TODO: Maybe we should do this in a separate thread?
                    for file in files {
                        if let Ok(reader) = ImageReader::open(&file) {
                            if let Ok(decoded) = reader.decode() {
                                let data = decoded.to_rgba8();
                                loaded_images.push(data);
                            }
                        }
                    }
                    if loaded_images.len() > 1 {
                        self.tool_manager.tools[0].set_enabled(true);
                    } else {
                        if !loaded_images.is_empty() {
                            let mut rgba_image = loaded_images[0].clone();
                            // Convert RGBA to BGRA by swapping R and B channels
                            for pixel in rgba_image.pixels_mut() {
                                let temp = pixel[0]; // Store R
                                pixel[0] = pixel[2]; // R = B
                                pixel[2] = temp; // B = R
                            }
                            loaded_images[0] = rgba_image;
                        }
                    }
                    self.input_images = loaded_images;

                    if !skip_processing {
                        if let Err(e) = self.process_from(0).await {
                            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::UserFeedback(format!(
                                    "Failed to process images: {}",
                                    e
                                )),
                            ));
                        }
                    }
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
                                FrontendMessage::UserFeedback(format!(
                                    "Communication error with frontend: {}",
                                    e
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

                    println!("Returning discovery request {:?}", r);

                    if let Err(e) = sender.send(PipelineCommand::FrontendMessage(r)) {
                        let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                            FrontendMessage::UserFeedback(format!(
                                "Communication error with frontend: {}",
                                e
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
                    let toml_content = std::fs::read_to_string(&path)
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
                    if let Err(e) = self.process_from(0).await {
                        let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                            FrontendMessage::UserFeedback(format!(
                                "Failed to process loaded pipeline: {}",
                                e
                            )),
                        ));
                    }
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
                                FrontendMessage::UserFeedback(format!(
                                    "Failed to process click event: {}",
                                    e
                                )),
                            ));
                        }
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
            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                FrontendMessage::UserFeedback(format!("Error processing tool messages: {}", e)),
            ));
        }

        if let Err(e) = self.tool_manager.receive_messages().await {
            let _ = self.output_sender.send(PipelineCommand::FrontendMessage(
                FrontendMessage::UserFeedback(format!("Tool manager communication error: {}", e)),
            ));
        }

        Ok(())
    }
}
