use std::sync::Arc;

use async_trait::async_trait;
use compositor::{Compositor, CompositorState};

use contrast::{Contrast, ContrastState};
use image::RgbaImage;

use tokio::sync::broadcast;

use crate::gpu::GpuImageProcessor;

pub mod cell_counter;
pub mod compositor;
pub mod contrast;

#[derive(Debug, Clone)]
pub enum CompositorMessage {
    ToggleVisibility(bool, u16),
}

#[derive(Debug, Clone)]
pub enum ContrastMessage {
    SetMinMax(f32, f32),
}

#[derive(Debug, Clone)]
pub enum PipelineObjectMessage {
    CompositorMessage(CompositorMessage),
    ContrastMessage(ContrastMessage),
    ToggleActive(bool),
}

#[derive(Debug, Clone)]
pub enum FrontendMessage {
    ReturnState(ToolState),
    ReturnBackendTools(BackendToolType),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackendToolType {
    Compositor(Vec<u16>),
    Contrast(Vec<u16>),
}

impl BackendToolType {
    /// Create a new instance of the same variant with different data
    pub fn with_data(&self, data: Vec<u16>) -> Self {
        match self {
            BackendToolType::Compositor(_) => BackendToolType::Compositor(data),
            BackendToolType::Contrast(_) => BackendToolType::Contrast(data),
        }
    }
}

/// Commands that can be sent to the pipeline
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// Refresh the pipeline starting from the given tool index
    Refresh(usize),
    /// Load input images into the pipeline
    Load(Vec<RgbaImage>),
    /// Return output image from the pipeline
    Output(RgbaImage),
    /// Message to pipeline object
    PipelineObject(u16, PipelineObjectMessage),
    /// Message to frontend
    FrontendMessage(FrontendMessage),
    /// Request state from pipeline object
    RequestState(u16),
    /// Discover
    DiscoverBackend(BackendToolType),
}

/// Result of pipeline processing
#[derive(Debug, Clone)]
pub enum ToolState {
    CompositorState(CompositorState),
    ContrastState(ContrastState),
}

/// Trait for image processing tools
#[async_trait]
pub trait ToolBackend: Send + Sync {
    /// Process input images and return modified images
    /// If the tool is disabled, it should return the input unchanged
    async fn process(&mut self, input: Vec<RgbaImage>) -> anyhow::Result<Vec<RgbaImage>>;

    /// Check if this tool is currently enabled
    fn is_enabled(&self) -> bool;

    /// Receive messages from the pipeline
    async fn receive_message(&mut self, message: &PipelineObjectMessage) -> anyhow::Result<()>;

    /// Enable or disable this tool
    fn set_enabled(&mut self, enabled: bool);

    /// Get the current state of the tool for synchronization
    fn get_state(&self) -> ToolState;

    /// Identify
    fn iam(&self) -> BackendToolType;
}

/// Pipeline that processes images through a series of tools
pub struct Pipeline {
    /// Tools in the pipeline, processed in order
    tools: Vec<Box<dyn ToolBackend>>,
    /// Channel for receiving commands
    command_receiver: broadcast::Receiver<PipelineCommand>,
    /// Channel for sending output results
    output_sender: broadcast::Sender<PipelineCommand>,
    /// Input images to process
    input_images: Vec<RgbaImage>,
}

impl Pipeline {
    /// Create a new pipeline with the given tools
    pub async fn new() -> (
        Self,
        broadcast::Sender<PipelineCommand>,
        broadcast::Receiver<PipelineCommand>,
    ) {
        let (command_tx, command_rx) = broadcast::channel(1000);
        let (output_tx, output_rx) = broadcast::channel(1000);

        let gpu_image_processor = Arc::new(GpuImageProcessor::new().await);

        let tools: Vec<Box<dyn ToolBackend>> = vec![
            Box::new(Compositor::new(command_tx.clone(), gpu_image_processor.clone()).await),
            Box::new(Contrast::new(command_tx.clone(), gpu_image_processor.clone()).await),
        ];

        let pipeline = Self {
            tools,
            command_receiver: command_rx,
            output_sender: output_tx,
            input_images: vec![],
        };

        (pipeline, command_tx, output_rx)
    }

    /// Process images through the pipeline starting from the given index
    pub async fn process_from(&mut self, start_index: usize) -> anyhow::Result<()> {
        let start_time = std::time::Instant::now();
        println!("Got process_from command!");
        let mut images = self.input_images.clone(); //TODO: Performance

        // Process through each tool starting from start_index
        for tool in self.tools.iter_mut().skip(start_index) {
            if tool.is_enabled() {
                images = tool.process(images).await?;
            }
        }
        if images.len() > 1 {
            return Err(anyhow::anyhow!(
                "Pipeline output should have only one image"
            ));
        }

        let _ = self
            .output_sender
            .send(PipelineCommand::Output(images[0].clone()));

        let duration = start_time.elapsed();
        println!("Pipeline processing took: {:?}", duration);

        Ok(())
    }

    /// Process all pending commands
    pub async fn process_commands(&mut self) {
        while let Ok(cmd) = self.command_receiver.try_recv() {
            match &cmd {
                PipelineCommand::Refresh(index) => self.process_from(*index).await.unwrap(),
                PipelineCommand::Load(images) => {
                    println!("Got load command!");
                    self.input_images = images.clone(); //TODO: Performance
                    if images.len() > 1 {
                        self.tools[0].set_enabled(true);
                    }
                    self.process_from(0).await.unwrap();
                }
                PipelineCommand::PipelineObject(object_index, message) => {
                    println!("Got pipeline object command!");
                    let i = *object_index as usize;
                    self.tools[i].receive_message(message).await.unwrap();
                }
                PipelineCommand::RequestState(index) => {
                    let state = self.tools[*index as usize].get_state();
                    self.output_sender
                        .send(PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnState(state),
                        ))
                        .unwrap();
                }
                PipelineCommand::DiscoverBackend(backend_tool_type) => {
                    println!(
                        "Received backend tools request | Pipeline | {:?}",
                        backend_tool_type
                    );
                    let mut discovered = vec![];
                    for (i, tool) in self.tools.iter().enumerate() {
                        let iam = tool.iam();
                        println!("Tool {}: {:?}", i, iam);
                        if iam == *backend_tool_type {
                            discovered.push(i as u16);
                        }
                    }
                    self.output_sender
                        .send(PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnBackendTools(
                                backend_tool_type.with_data(discovered),
                            ),
                        ))
                        .unwrap();
                }
                _ => {}
            };
        }
    }
}
