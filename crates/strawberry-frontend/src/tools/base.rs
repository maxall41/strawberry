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
    pub focus_handle: FocusHandle,
    pub cleanup_guard: crate::tools::CleanupGuard,
    pub processor_spawned: bool,
}

impl ToolBase {
    pub fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        backend_type: BackendToolType,
        focus_handle: FocusHandle,
    ) -> Self {
        Self {
            command_sender: command_sender.clone(),
            backend_type: backend_type.clone(),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            focus_handle,
            cleanup_guard: crate::tools::CleanupGuard::new(command_sender, backend_type),
            processor_spawned: false,
        }
    }

    /// Set up the backend connection: allocate the message channel and announce
    /// ourselves so the pipeline hands back per-tool channels (answered via
    /// `ReturnBackendTools`). Returns false if the pipeline is unreachable.
    pub fn connect(&mut self) -> bool {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            self.backend_type.clone(),
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return false;
        }
        self.cleanup_guard.mark_connected();
        println!(
            "Sent discovery request to backend | {:?}",
            self.backend_type
        );
        true
    }

    /// Get current backend tool index - 100% identical across all tools
    pub fn get_current_tool_index(&self) -> Option<u16> {
        match self.current_tool_index.read() {
            Ok(guard) => *guard,
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

        self.send_channels
            .get(usize::from(current_tool_index))
            .ok_or_else(|| format!("Invalid tool index: {current_tool_index}"))?
            .send(message)
            .map_err(|e| format!("Failed to send to backend: {e}"))
    }
}
