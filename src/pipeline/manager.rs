use std::cmp::{max, min};

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
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
    pub cached_output: Option<Vec<image::RgbaImage>>,
    pub cached_state: Option<ManagerState>,
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
        mut input: Vec<Box<dyn ToolBackend>>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();

        // Assign indices to all tools
        for (index, tool) in input.iter_mut().enumerate() {
            tool.set_index(index);
        }

        ToolManager {
            state: ManagerState {
                proxy_tools: update_proxy_tools(&input),
                is_enabled: true,
            },
            tools: input,
            frontend_senders: Vec::new(),
            command_sender,
            sender,
            receiver,
            index: 0,
            cached_output: None,
            cached_state: None,
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
    fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }
    fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }
    async fn process(&mut self, input: &mut Vec<image::RgbaImage>) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Not implemented"))
    }

    async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let old_state = self.state.clone();

        if let Ok(message) = self.receiver.try_recv() {
            match message {
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    ));
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::ManagerMessage(message) => match message {
                        ManagerMessage::ReorderTool(start_index, new_index) => {
                            let tool = self.tools.remove(max(start_index as usize, 0));
                            self.tools
                                .insert(min(new_index as usize, self.tools.len()), tool);

                            // Reassign indices to all tools after reordering
                            for (index, tool) in self.tools.iter_mut().enumerate() {
                                tool.set_index(index);
                            }

                            if let Err(e) = self
                                .command_sender
                                .send(PipelineCommand::Refresh(self.index))
                            {
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
            // Invalidate cache when state changes
            self.invalidate_cache();

            // Push state update to frontend
            self.send_to_frontend(PipelineCommand::FrontendMessage(
                FrontendMessage::ReturnState(self.get_state()),
            ));
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Manager
    }

    fn supports_processing(&self) -> bool {
        false
    }

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    fn has_valid_cache(&self) -> bool {
        if let Some(cached_state) = &self.cached_state {
            cached_state == &self.state
        } else {
            false
        }
    }

    fn get_cached_output(&self) -> Option<Vec<image::RgbaImage>> {
        self.cached_output.clone()
    }

    fn set_cached_output(&mut self, output: Vec<image::RgbaImage>) {
        self.cached_output = Some(output);
        self.cached_state = Some(self.state.clone());
    }

    fn invalidate_cache(&mut self) {
        self.cached_output = None;
        self.cached_state = None;
    }
}

impl ToolManager {
    fn send_to_frontend(&self, message: PipelineCommand) {
        for sender in &self.frontend_senders {
            if let Err(e) = sender.try_send(message.clone()) {
                eprintln!("Failed to send message to frontend: {}", e);
            }
        }
    }
}
