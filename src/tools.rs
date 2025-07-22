use gpui::WindowOptions;

pub mod compositor;
pub mod contrast;
pub mod counter;
pub mod gaussian_blur;
pub mod local_maxima;
pub mod manager;
pub mod threshold;

/// RAII guard that automatically sends disconnect command when dropped
pub struct CleanupGuard {
    command_sender: tokio::sync::broadcast::Sender<crate::pipeline::PipelineCommand>,
    backend_tool_type: crate::pipeline::BackendToolType,
}

impl CleanupGuard {
    pub fn new(
        command_sender: tokio::sync::broadcast::Sender<crate::pipeline::PipelineCommand>,
        backend_tool_type: crate::pipeline::BackendToolType,
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
                .send(crate::pipeline::PipelineCommand::DisconnectFrontend(
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

pub trait ToolFrontend {
    fn get_window_options(&self) -> WindowOptions;
}
