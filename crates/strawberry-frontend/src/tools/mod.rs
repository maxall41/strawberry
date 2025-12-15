use gpui::{Context, Window, WindowOptions};

pub mod compositor;
pub mod contrast;
pub mod counter;
pub mod decomposer;
pub mod gaussian_blur;
pub mod grayscale;
pub mod local_maxima;
pub mod manager;
pub mod morphology;
pub mod nervetracer;
pub mod rolling_ball;
pub mod sobel;
pub mod threshold;
pub mod watershed;
/// RAII guard that automatically sends disconnect command when dropped
pub struct CleanupGuard {
    command_sender: tokio::sync::broadcast::Sender<strawberry_types::PipelineCommand>,
    backend_tool_type: strawberry_types::BackendToolType,
}

impl CleanupGuard {
    pub fn new(
        command_sender: tokio::sync::broadcast::Sender<strawberry_types::PipelineCommand>,
        backend_tool_type: strawberry_types::BackendToolType,
    ) -> Self {
        Self {
            command_sender,
            backend_tool_type,
        }
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        println!(
            "CleanupGuard dropping for {:?} - tool window closing",
            self.backend_tool_type
        );
        if let Err(e) =
            self.command_sender
                .send(strawberry_types::PipelineCommand::DisconnectFrontend(
                    self.backend_tool_type.clone(),
                ))
        {
            eprintln!(
                "Failed to send disconnect command for {:?}: {}",
                self.backend_tool_type, e
            );
        }
    }
}
