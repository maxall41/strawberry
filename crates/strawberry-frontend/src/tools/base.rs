use gpui::FocusHandle;
use strawberry_types::{CommandSender, PipelineCommand, PipelineObjectMessage, ToolId, ToolState};
use tokio::sync::{oneshot, watch};
pub struct ToolBase {
    pub command_sender: CommandSender,
    pub id: ToolId,
    pub focus_handle: FocusHandle,
    pub subscription: Option<oneshot::Receiver<Option<watch::Receiver<ToolState>>>>,
    pub processor_spawned: bool,
}
impl ToolBase {
    pub const fn new(command_sender: CommandSender, id: ToolId, focus_handle: FocusHandle) -> Self {
        Self {
            command_sender,
            id,
            focus_handle,
            subscription: None,
            processor_spawned: false,
        }
    }
    pub fn connect(&mut self) -> bool {
        let (reply, receiver) = oneshot::channel();
        self.subscription = Some(receiver);
        self.command_sender
            .send(PipelineCommand::SubscribeTool { id: self.id, reply })
            .is_ok()
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.command_sender
            .send(PipelineCommand::SetEnabled {
                id: self.id,
                enabled,
            })
            .map_err(|e| e.to_string())
    }
    pub fn select(&self) {
        let _ = self
            .command_sender
            .send(PipelineCommand::SelectTool(Some(self.id)));
    }
    pub fn send_to_backend(&self, message: PipelineObjectMessage) -> Result<(), String> {
        self.command_sender
            .send(PipelineCommand::ToolMessage {
                id: self.id,
                message,
            })
            .map_err(|e| e.to_string())
    }
}
