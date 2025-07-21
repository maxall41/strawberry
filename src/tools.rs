use async_trait::async_trait;
use gpui::{ImageSource, RenderImage};
use image::{Frame, RgbaImage};
use std::{sync::Arc, time::Duration};

pub mod cell_counter;
pub mod compositor;
pub mod contrast;

/// Commands that can be sent to the pipeline
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// Refresh the pipeline starting from the given tool index
    Refresh(usize),
    /// Load input images into the pipeline
    Load(Vec<RgbaImage>),
    /// Return output image from the pipeline
    Output(RgbaImage),
}

/// Result of pipeline processing
#[derive(Debug, Clone)]
pub struct PipelineOutput {
    pub images: Vec<RgbaImage>,
    pub processing_time_ms: u128,
}

/// Trait for image processing tools
#[async_trait]
pub trait Tool: Send + Sync {
    /// Get the name of this tool for UI display
    fn name(&self) -> &str;

    /// Process input images and return modified images
    /// If the tool is disabled, it should return the input unchanged
    async fn process(&mut self, input: Vec<RgbaImage>) -> anyhow::Result<Vec<RgbaImage>>;

    /// Check if this tool is currently enabled
    fn is_enabled(&self) -> bool;

    /// Get a unique identifier for this tool instance
    fn id(&self) -> &str;
}

/// Pipeline that processes images through a series of tools
pub struct Pipeline {
    /// Tools in the pipeline, processed in order
    tools: Vec<Box<dyn Tool>>,
    /// Channel for receiving commands
    command_receiver: flume::Receiver<PipelineCommand>,
    /// Channel for sending output results
    output_sender: flume::Sender<PipelineCommand>,
    /// Input images to process
    input_images: Vec<RgbaImage>,
}

impl Pipeline {
    /// Create a new pipeline with the given tools
    pub fn new(
        tools: Vec<Box<dyn Tool>>,
    ) -> (
        Self,
        flume::Sender<PipelineCommand>,
        flume::Receiver<PipelineCommand>,
    ) {
        let (command_tx, command_rx) = flume::unbounded();
        let (output_tx, output_rx) = flume::unbounded();

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

        self.output_sender
            .send(PipelineCommand::Output(images[0].clone()))
            .unwrap();

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
                    self.process_from(0).await.unwrap();
                }
                _ => {}
            };
        }
    }
}
