use async_trait::async_trait;

use super::{
    BackendToolType, CompositorMessage, FrontendMessage, ManagerMessage, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolProxy {
    pub name: String,
    pub position: u16,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManagerState {
    pub proxy_tools: Vec<ToolProxy>,
    pub is_enabled: bool,
}

pub struct ToolManager {
    pub tools: Vec<Box<dyn ToolBackend>>,
    pub state: ManagerState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_sender: Option<flume::Sender<PipelineCommand>>,
}

fn update_proxy_tools(input: &Vec<Box<dyn ToolBackend>>) -> Vec<ToolProxy> {
    input
        .iter()
        .enumerate()
        .filter(|(_, tool)| tool.is_enabled())
        .map(|(index, tool)| ToolProxy {
            name: tool.iam().to_string(),
            position: index as u16,
        })
        .collect()
}

impl ToolManager {
    pub fn new(
        input: Vec<Box<dyn ToolBackend>>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        ToolManager {
            state: ManagerState {
                proxy_tools: update_proxy_tools(&input),
                is_enabled: true,
            },
            tools: input,
            frontend_sender: None,
            command_sender,
            sender,
            receiver,
        }
    }
}

#[async_trait]
impl ToolBackend for ToolManager {
    fn is_enabled(&self) -> bool {
        self.state.is_enabled
    }
    fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }
    fn set_enabled(&mut self, enabled: bool) {
        self.state.is_enabled = enabled;
    }
    fn get_state(&self) -> ToolState {
        ToolState::ManagerState(self.state.clone())
    }
    fn set_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_sender = Some(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_sender = None;
    }
    async fn process(
        &mut self,
        input: Vec<image::RgbaImage>,
    ) -> anyhow::Result<Vec<image::RgbaImage>> {
        Err(anyhow::anyhow!("Not implemented"))
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let old_state = self.state.clone();

        if let Ok(message) = self.receiver.try_recv() {
            match message {
                PipelineCommand::RequestState() => {
                    if let Some(sender) = &self.frontend_sender {
                        if let Err(e) = sender.try_send(PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnState(self.get_state()),
                        )) {
                            eprintln!("Failed to send state to frontend: {}", e);
                        }
                    } else {
                        eprintln!("No frontend sender available for state request");
                    }
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::ManagerMessage(message) => match message {
                        ManagerMessage::ReorderTool(start_index, new_index) => {
                            let tool = self.tools.remove(start_index as usize);
                            self.tools.insert(new_index as usize, tool);

                            if let Err(e) = self.command_sender.send(PipelineCommand::Refresh(0)) {
                                eprintln!("Failed to send refresh command: {}", e);
                            }
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        };

        let new_proxy_tools = update_proxy_tools(&self.tools);
        if new_proxy_tools != self.state.proxy_tools {
            self.state.proxy_tools = new_proxy_tools;
        }
        if old_state != self.state {
            // Push state update to frontend
            if let Some(frontend_sender) = &self.frontend_sender {
                if let Err(e) = frontend_sender.try_send(PipelineCommand::FrontendMessage(
                    FrontendMessage::ReturnState(self.get_state()),
                )) {
                    eprintln!("Failed to send state update to frontend: {}", e);
                }
            }
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Manager
    }

    fn supports_processing(&self) -> bool {
        false
    }
}
