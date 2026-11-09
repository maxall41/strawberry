use std::collections::HashMap;

use strawberry_types::{
    FrontendMessage, ManagerMessage, ManagerState, PipelineCommand, PipelineConfig,
    PipelineObjectMessage, ToolBackend, ToolProxy, ToolState,
};

pub(crate) fn serialize_pipeline_config(
    config: &PipelineConfig,
    indices: &HashMap<String, u16>,
) -> anyhow::Result<String> {
    let mut value = toml::Value::try_from(config)?;
    if let Some(tools) = value.get_mut("tools").and_then(toml::Value::as_table_mut) {
        for (name, tool) in tools {
            let Some(index) = indices.get(name) else {
                continue;
            };
            let Some(state) = tool
                .as_table_mut()
                .and_then(|variants| variants.iter_mut().next().map(|(_, value)| value))
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            state.insert("index".to_string(), toml::Value::Integer((*index).into()));
        }
    }
    Ok(toml::to_string(&value)?)
}

pub(crate) fn deserialize_pipeline_config(
    content: &str,
) -> anyhow::Result<(PipelineConfig, HashMap<String, u16>)> {
    let value: toml::Value = toml::from_str(content)?;
    let indices = value
        .get("tools")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(name, tool)| {
            let index = tool
                .as_table()?
                .values()
                .next()?
                .as_table()?
                .get("index")?
                .as_integer()?;
            u16::try_from(index).ok().map(|index| (name.clone(), index))
        })
        .collect();
    Ok((value.try_into()?, indices))
}

/// Orchestrates the pipeline's tools.
///
/// Note that unlike the tools it manages,
/// the manager itself is NOT a `ToolBackend`: it never appears in the pipeline
/// tool list, doesn't process images, and has no cache. The `Pipeline`
/// special-cases it for discovery/disconnect instead.
pub struct ToolManager {
    pub tools: Vec<Box<dyn ToolBackend>>,
    pub state: ManagerState,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub frontend_latest_state: Option<ManagerState>,
    previous_enabled: Vec<bool>,
    ever_enabled: Vec<bool>,
}

fn update_proxy_tools(input: &Vec<Box<dyn ToolBackend>>) -> Vec<ToolProxy> {
    input
        .iter()
        .enumerate()
        .filter(|(_, tool)| tool.is_enabled())
        .filter_map(|(index, tool)| {
            Some(ToolProxy {
                name: tool.iam().to_string(),
                position: u16::try_from(index).ok()?,
            })
        })
        .collect()
}

impl ToolManager {
    #[must_use]
    pub fn new(
        mut input: Vec<Box<dyn ToolBackend>>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();

        // Assign indices to all tools
        for (index, tool) in input.iter_mut().enumerate() {
            tool.set_index(index);
        }
        let previous_enabled: Vec<bool> = input.iter().map(|tool| tool.is_enabled()).collect();
        let ever_enabled = previous_enabled.clone();

        Self {
            state: ManagerState {
                proxy_tools: update_proxy_tools(&input),
                is_enabled: true,
            },
            tools: input,
            frontend_senders: Vec::new(),
            global_sender,
            sender,
            receiver,
            frontend_latest_state: None,
            previous_enabled,
            ever_enabled,
        }
    }

    fn reindex_tools(&mut self) {
        for (index, tool) in self.tools.iter_mut().enumerate() {
            tool.set_index(index);
        }
    }

    pub(crate) fn restore_tool_indices(&mut self, indices: &HashMap<String, u16>) {
        if indices.is_empty() {
            return;
        }

        let mut ordered: Vec<_> = self
            .tools
            .drain(..)
            .zip(self.previous_enabled.drain(..))
            .zip(self.ever_enabled.drain(..))
            .enumerate()
            .collect();
        ordered.sort_by_key(|(current_index, ((tool, _), _))| {
            indices
                .get(&format!("{:?}", tool.iam()))
                .copied()
                .map_or((1, *current_index), |index| (0, usize::from(index)))
        });

        for (_, ((tool, was_enabled), was_ever_enabled)) in ordered {
            self.tools.push(tool);
            self.previous_enabled.push(was_enabled);
            self.ever_enabled.push(was_ever_enabled);
        }
        self.reindex_tools();
    }

    pub(crate) fn synchronize_enabled_history(&mut self) {
        self.previous_enabled = self.tools.iter().map(|tool| tool.is_enabled()).collect();
        self.ever_enabled
            .iter_mut()
            .zip(&self.previous_enabled)
            .for_each(|(ever_enabled, enabled)| *ever_enabled |= *enabled);
    }

    /// Move tools that became enabled during this cycle to the end of the pipeline.
    fn move_newly_enabled_tools_to_end(&mut self) -> bool {
        let mut retained_tools = Vec::with_capacity(self.tools.len());
        let mut retained_enabled = Vec::with_capacity(self.previous_enabled.len());
        let mut retained_ever_enabled = Vec::with_capacity(self.ever_enabled.len());
        let mut newly_enabled_tools = Vec::new();
        let mut newly_enabled_states = Vec::new();
        let mut newly_enabled_ever_states = Vec::new();

        for ((tool, was_enabled), was_ever_enabled) in self
            .tools
            .drain(..)
            .zip(self.previous_enabled.drain(..))
            .zip(self.ever_enabled.drain(..))
        {
            if tool.is_enabled() && !was_enabled && !was_ever_enabled {
                newly_enabled_states.push(true);
                newly_enabled_ever_states.push(true);
                newly_enabled_tools.push(tool);
            } else {
                retained_enabled.push(tool.is_enabled());
                retained_ever_enabled.push(was_ever_enabled || tool.is_enabled());
                retained_tools.push(tool);
            }
        }

        let order_changed = !newly_enabled_tools.is_empty();
        retained_tools.append(&mut newly_enabled_tools);
        retained_enabled.append(&mut newly_enabled_states);
        retained_ever_enabled.append(&mut newly_enabled_ever_states);
        self.tools = retained_tools;
        self.previous_enabled = retained_enabled;
        self.ever_enabled = retained_ever_enabled;

        if order_changed {
            self.reindex_tools();
        }
        order_changed
    }

    #[must_use]
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

    /// # Errors
    /// Returns an error if a command fails or a communication channel is disconnected.
    #[expect(
        clippy::too_many_lines,
        reason = "Command dispatch as one function is cleaner"
    )]
    pub fn receive_messages(&mut self) -> anyhow::Result<bool> {
        let mut processing_required = false;
        let commands: Vec<PipelineCommand> =
            std::iter::from_fn(|| self.receiver.try_recv().ok()).collect();

        for message in commands {
            match message {
                PipelineCommand::RequestState() => {
                    self.send_to_frontend(&PipelineCommand::FrontendMessage(
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
                            let mut indices = HashMap::new();
                            let mut enabled_index = 0u16;
                            for tool in &self.tools {
                                if !tool.is_enabled() {
                                    continue;
                                }
                                let tool_name = format!("{:?}", tool.iam());
                                indices.insert(tool_name.clone(), enabled_index);
                                configs.insert(tool_name, tool.get_state());
                                enabled_index = enabled_index
                                    .checked_add(1)
                                    .ok_or_else(|| anyhow::anyhow!("Too many pipeline tools"))?;
                            }
                            let pipeline_config = PipelineConfig { tools: configs };
                            let toml_string =
                                match serialize_pipeline_config(&pipeline_config, &indices) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        self.global_sender.send(
                                            PipelineCommand::FrontendMessage(
                                                FrontendMessage::error(format!(
                                                    "Failed to serialize pipeline: {e}"
                                                )),
                                            ),
                                        )?;
                                        return Err(anyhow::anyhow!(e));
                                    }
                                };
                            if let Err(e) = std::fs::write(&path, toml_string) {
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(format!(
                                        "Failed to save pipeline to file: {e}"
                                    )),
                                ))?;
                                return Err(anyhow::anyhow!("Failed to write to file: {e}"));
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
                                    return Err(anyhow::anyhow!("Failed to read file: {e}"));
                                }
                            };
                            let (pipeline_config, indices) =
                                match deserialize_pipeline_config(&toml_content) {
                                    Ok(config) => config,
                                    Err(e) => {
                                        self.global_sender.send(
                                            PipelineCommand::FrontendMessage(
                                                FrontendMessage::error(format!(
                                                    "Failed to parse pipeline file: {e}"
                                                )),
                                            ),
                                        )?;
                                        return Err(anyhow::anyhow!("Failed to parse TOML: {e}"));
                                    }
                                };

                            // A loaded pipeline replaces the active configuration.
                            for tool in &mut self.tools {
                                tool.set_enabled(false);
                            }
                            for tool in &mut self.tools {
                                let tool_name = format!("{:?}", tool.iam());
                                if let Some(config) = pipeline_config.tools.get(&tool_name) {
                                    tool.apply_loaded_persisted_state(config.clone())?;
                                }
                            }
                            self.restore_tool_indices(&indices);
                            self.synchronize_enabled_history();

                            processing_required = true;

                            self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::success(
                                    "Pipeline loaded successfully!".to_string(),
                                ),
                            ))?;
                        }
                        ManagerMessage::ReorderTool(start_index, new_index) => {
                            let start_idx = usize::from(start_index);
                            let new_idx = usize::from(new_index);

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

                            if start_idx == new_idx.min(self.tools.len().saturating_sub(1)) {
                                continue;
                            }
                            let tool = self.tools.remove(start_idx);
                            let was_enabled = self.previous_enabled.remove(start_idx);
                            let was_ever_enabled = self.ever_enabled.remove(start_idx);
                            let insert_idx = std::cmp::min(new_idx, self.tools.len());
                            self.tools.insert(insert_idx, tool);
                            self.previous_enabled.insert(insert_idx, was_enabled);
                            self.ever_enabled.insert(insert_idx, was_ever_enabled);

                            // Reassign indices to all tools after reordering
                            self.reindex_tools();

                            processing_required = true;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        processing_required |= self.move_newly_enabled_tools_to_end();

        let new_proxy_tools = update_proxy_tools(&self.tools);
        if new_proxy_tools != self.state.proxy_tools {
            self.state.proxy_tools = new_proxy_tools;
        }

        if self.frontend_latest_state.as_ref() != Some(&self.state) {
            self.update_state()?;
        }

        Ok(processing_required)
    }

    fn update_state(&mut self) -> anyhow::Result<()> {
        self.send_to_frontend(&PipelineCommand::FrontendMessage(
            FrontendMessage::ReturnState(self.get_state()),
        ))?;
        self.frontend_latest_state = Some(self.state.clone());
        Ok(())
    }

    fn send_to_frontend(&self, message: &PipelineCommand) -> anyhow::Result<()> {
        for sender in &self.frontend_senders {
            sender
                .try_send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send message to frontend: {e}"))?;
        }
        Ok(())
    }
}
