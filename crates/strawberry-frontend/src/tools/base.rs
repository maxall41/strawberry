use gpui::FocusHandle;
use std::sync::{Arc, RwLock};
use strawberry_types::{BackendToolType, PipelineCommand};

/// Common infrastructure shared by all frontend tools
pub struct ToolBase {
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub backend_type: BackendToolType,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub focus_handle: Option<FocusHandle>,
    pub cleanup_guard: crate::tools::CleanupGuard,
}

impl ToolBase {
    pub fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        backend_type: BackendToolType,
    ) -> Self {
        Self {
            command_sender: command_sender.clone(),
            backend_type: backend_type.clone(),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            focus_handle: None,
            cleanup_guard: crate::tools::CleanupGuard::new(command_sender, backend_type),
        }
    }

    /// Set up the backend connection: allocate the message channel and announce
    /// ourselves so the pipeline hands back per-tool channels (answered via
    /// `ReturnBackendTools`). Returns false if the pipeline is unreachable.
    pub fn connect(&mut self, focus_handle: FocusHandle) -> bool {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(focus_handle);
        if let Err(e) = self
            .command_sender
            .send(PipelineCommand::DiscoverBackend(
                self.backend_type.clone(),
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return false;
        }
        println!(
            "Sent discovery request to backend | {:?}",
            self.backend_type
        );
        true
    }

    /// Get current backend tool index - 100% identical across all tools
    pub fn get_current_tool_index(&self) -> Option<u16> {
        match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => Some(index),
                None => {
                    eprintln!("No current tool index available");
                    None
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                None
            }
        }
    }

    /// Send command to backend - common helper to reduce duplication
    pub fn send_to_backend(&self, message: PipelineCommand) -> Result<(), String> {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return Err("No current tool index".to_string());
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            sender
                .send(message)
                .map_err(|e| format!("Failed to send to backend: {e}"))
        } else {
            Err(format!("Invalid tool index: {current_tool_index}"))
        }
    }
}
