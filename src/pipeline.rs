use std::sync::Arc;

use async_trait::async_trait;
use compositor::{Compositor, CompositorState};

use contrast::{Contrast, ContrastState};
use image::RgbaImage;

use futures::future::try_join_all;
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
    ReturnBackendTools(Vec<u16>, Vec<(flume::Sender<PipelineCommand>)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BackendToolType {
    Compositor,
    Contrast,
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
    PipelineObject(PipelineObjectMessage),
    /// Message to frontend
    FrontendMessage(FrontendMessage),
    /// Request state from pipeline object
    RequestState(),
    /// Discover
    DiscoverBackend(BackendToolType, flume::Sender<PipelineCommand>),
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
    async fn receive_messages(&mut self) -> anyhow::Result<()>;

    /// Enable or disable this tool
    fn set_enabled(&mut self, enabled: bool);

    /// Get the current state of the tool for synchronization
    fn get_state(&self) -> ToolState;

    /// Identify
    fn iam(&self) -> BackendToolType;

    fn get_sender(&self) -> flume::Sender<PipelineCommand>;

    fn set_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>);
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
    pub async fn new() -> anyhow::Result<(
        Self,
        broadcast::Sender<PipelineCommand>,
        broadcast::Receiver<PipelineCommand>,
    )> {
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

        Ok((pipeline, command_tx, output_rx))
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

        if let Err(e) = self
            .output_sender
            .send(PipelineCommand::Output(images[0].clone()))
        {
            eprintln!("Failed to send output image: {}", e);
        }

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
                        eprintln!("Failed to process pipeline from index {}: {}", index, e);
                    }
                }
                PipelineCommand::Load(images) => {
                    println!("Got load command!");
                    self.input_images = images.clone(); //TODO: Performance
                    if images.len() > 1 {
                        self.tools[0].set_enabled(true);
                    }
                    if let Err(e) = self.process_from(0).await {
                        eprintln!("Failed to process pipeline after loading images: {}", e);
                    }
                }
                PipelineCommand::DiscoverBackend(backend_tool_type, sender) => {
                    let mut discovered = vec![];
                    let mut channels = vec![];
                    let mut i = 0;
                    for tool in self.tools.iter_mut() {
                        let iam = tool.iam();
                        println!("Tool {}: {:?}", i, iam);
                        if iam == *backend_tool_type {
                            discovered.push(i as u16);
                            tool.set_frontend_sender(sender.clone());
                            channels.push(tool.get_sender());
                            i += 1;
                        }
                    }

                    let r = FrontendMessage::ReturnBackendTools(discovered, channels);

                    println!("Returning discovery request {:?}", r);

                    if let Err(e) = sender.send(PipelineCommand::FrontendMessage(r)) {
                        eprintln!("Failed to send backend discovery response: {}", e);
                    }
                }
                _ => {}
            };
        }
        let futures = self.tools.iter_mut().map(|tool| tool.receive_messages());
        if let Err(e) = try_join_all(futures).await {
            eprintln!("Failed to process tool messages: {}", e);
        }

        Ok(())
    }
}
