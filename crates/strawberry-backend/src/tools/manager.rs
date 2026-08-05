use std::collections::HashMap;

use strawberry_types::{
    FrontendMessage, ManagerMessage, ManagerState, PipelineCommand, PipelineConfig,
    PipelineObjectMessage, ToolBackend, ToolProxy, ToolState,
};

/// Orchestrates the pipeline's tools. Note that unlike the tools it manages,
/// the manager itself is NOT a `ToolBackend`: it never appears in the pipeline
/// tool list, doesn't process images, and has no cache. The `Pipeline`
/// special-cases it for discovery/disconnect instead.
pub struct ToolManager {
    pub tools: Vec<Box<dyn ToolBackend>>,
    pub state: ManagerState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub frontend_latest_state: Option<ManagerState>,
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
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
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
            global_sender,
            sender,
            receiver,
            frontend_latest_state: None,
        }
    }

    pub fn get_sender(&self) -> flume::Sender<PipelineCommand> {
        self.sender.clone()
    }

    fn get_state(&self) -> ToolState {
        ToolState::ManagerState(self.state.clone())
    }

    pub fn add_frontend_sender(&mut self, sender: flume::Sender<PipelineCommand>) {
        self.frontend_senders.push(sender);
    }

    pub fn disconnect(&mut self) {
        self.frontend_senders.clear();
    }

    pub async fn receive_messages(&mut self) -> anyhow::Result<()> {
        let commands: Vec<PipelineCommand> =
            std::iter::from_fn(|| self.receiver.try_recv().ok()).collect();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(PipelineCommand::FrontendMessage(
                        FrontendMessage::ReturnState(self.get_state()),
                    ))?;
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::ManagerMessage(message) => match message {
                        ManagerMessage::SavePipeline(path) => {
                            let mut configs: HashMap<String, ToolState> = HashMap::new();
                            for tool in &self.tools {
                                if !tool.is_enabled() {
                                    continue;
                                }
                                let tool_name = format!("{:?}", tool.iam());
                                configs.insert(tool_name, tool.get_state());
                            }
                            let pipeline_config = PipelineConfig { tools: configs };
                            let toml_string = match toml::to_string(&pipeline_config) {
                                Ok(s) => s,
                                Err(e) => {
                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::error(format!(
                                            "Failed to serialize pipeline: {e}"
                                        )),
                                    ))?;
                                    return Err(anyhow::anyhow!(e));
                                }
                            };
                            if let Err(e) = std::fs::write(&path, toml_string) {
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(format!(
                                        "Failed to save pipeline to file: {e}"
                                    )),
                                ))?;
                                return Err(anyhow::anyhow!("Failed to write to file: {}", e));
                            }
                            self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::success(
                                    "Pipeline saved successfully!".to_string(),
                                ),
                            ))?;
                        }
                        ManagerMessage::LoadPipeline(path) => {
                            let toml_content = match std::fs::read_to_string(&path) {
                                Ok(content) => content,
                                Err(e) => {
                                    self.global_sender.send(PipelineCommand::FrontendMessage(
                                        FrontendMessage::error(format!(
                                            "Failed to read pipeline file: {e}"
                                        )),
                                    ))?;
                                    return Err(anyhow::anyhow!("Failed to read file: {}", e));
                                }
                            };
                            let pipeline_config: PipelineConfig =
                                match toml::from_str(&toml_content) {
                                    Ok(config) => config,
                                    Err(e) => {
                                        self.global_sender.send(
                                            PipelineCommand::FrontendMessage(
                                                FrontendMessage::error(format!(
                                                    "Failed to parse pipeline file: {e}"
                                                )),
                                            ),
                                        )?;
                                        return Err(anyhow::anyhow!("Failed to parse TOML: {}", e));
                                    }
                                };

                            // Apply the loaded configurations to the tools
                            for tool in self.tools.iter_mut() {
                                let tool_name = format!("{:?}", tool.iam());
                                if let Some(config) = pipeline_config.tools.get(&tool_name) {
                                    tool.apply_state(config.clone());
                                }
                            }

                            self.command_sender
                                .send(PipelineCommand::Refresh(0))
                                .map_err(|e| {
                                    anyhow::anyhow!("Failed to send refresh command: {}", e)
                                })?;

                            self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::success(
                                    "Pipeline loaded successfully!".to_string(),
                                ),
                            ))?;
                        }
                        ManagerMessage::ReorderTool(start_index, new_index) => {
                            let start_idx = start_index as usize;
                            let new_idx = new_index as usize;

                            if start_idx >= self.tools.len() {
                                let error_msg = format!(
                                    "Manager: Start index {} out of bounds (max: {})",
                                    start_idx,
                                    self.tools.len()
                                );
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(error_msg.clone()),
                                ))?;
                                return Err(anyhow::anyhow!(error_msg));
                            }

                            let tool = self.tools.remove(start_idx);
                            let insert_idx = std::cmp::min(new_idx, self.tools.len());
                            self.tools.insert(insert_idx, tool);

                            // Reassign indices to all tools after reordering
                            for (index, tool) in self.tools.iter_mut().enumerate() {
                                tool.set_index(index);
                            }

                            self.command_sender
                                .send(PipelineCommand::Refresh(0))
                                .map_err(|e| {
                                    anyhow::anyhow!("Failed to send refresh command: {}", e)
                                })?;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        let new_proxy_tools = update_proxy_tools(&self.tools);
        if new_proxy_tools != self.state.proxy_tools {
            self.state.proxy_tools = new_proxy_tools;
        }

        if self.frontend_latest_state.is_none()
            || self
                .frontend_latest_state
                .as_ref()
                .expect("Just checked that frontend_latest_state is some")
                != &self.state
        {
            self.update_state()?;
        }

        Ok(())
    }

    fn update_state(&mut self) -> anyhow::Result<()> {
        self.send_to_frontend(PipelineCommand::FrontendMessage(
            FrontendMessage::ReturnState(self.get_state()),
        ))?;
        self.frontend_latest_state = Some(self.state.clone());
        Ok(())
    }

    fn send_to_frontend(&self, message: PipelineCommand) -> anyhow::Result<()> {
        for sender in &self.frontend_senders {
            sender
                .try_send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send message to frontend: {}", e))?;
        }
        Ok(())
    }
}
