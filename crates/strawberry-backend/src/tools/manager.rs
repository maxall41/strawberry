use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use crate::pipeline_io::{self, LoadedPipeline};

use strawberry_types::{
    BackendToolType, FrontendMessage, ManagerMessage, ManagerState, PipelineCommand,
    PipelineConfig, PipelineObjectMessage, ToolAssets, ToolBackend, ToolProxy, ToolState,
};

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
    included: HashSet<BackendToolType>,
}

fn update_proxy_tools(
    input: &[Box<dyn ToolBackend>],
    included: &HashSet<BackendToolType>,
) -> Vec<ToolProxy> {
    input
        .iter()
        .enumerate()
        .filter(|(_, tool)| included.contains(&tool.iam()))
        .filter_map(|(index, tool)| {
            Some(ToolProxy {
                name: tool.iam().to_string(),
                position: u16::try_from(index).ok()?,
                is_enabled: tool.is_enabled(),
            })
        })
        .collect()
}

impl ToolManager {
    pub(crate) fn load_pipeline(&mut self, path: &Path, cli: bool) -> anyhow::Result<()> {
        self.apply_pipeline_config(pipeline_io::load(path)?, cli)
    }

    fn save_pipeline(&self, path: &Path) -> anyhow::Result<()> {
        let (config, indices) = self.pipeline_config()?;
        let mut assets = BTreeMap::new();
        for tool in &self.tools {
            if self.included.contains(&tool.iam()) {
                assets.insert(tool.iam().to_string(), tool.pipeline_assets()?);
            }
        }
        pipeline_io::save(path, &config, &indices, &assets)
    }

    /// Apply a loaded pipeline config to all tools.
    pub(crate) fn apply_pipeline_config(
        &mut self,
        mut config: LoadedPipeline,
        cli: bool,
    ) -> anyhow::Result<()> {
        // Presence in the file defines membership, including disabled tools.
        let included = self
            .tools
            .iter()
            .filter(|tool| config.tools.contains_key(&tool.iam().to_string()))
            .map(|tool| tool.iam())
            .collect();
        let mut updates = Vec::new();
        let mut loaded = Vec::with_capacity(self.tools.len());
        let empty_assets = ToolAssets::new();
        for tool in &mut self.tools {
            let name = format!("{:?}", tool.iam());
            let update = match config.tools.remove(&name) {
                Some(settings) => tool
                    .prepare_loaded_persisted_state(
                        settings,
                        config.assets.get(&name).unwrap_or(&empty_assets),
                        cli,
                    )
                    .map_err(|error| anyhow::anyhow!("Failed to load {name}: {error}"))?,
                None => None,
            };
            loaded.push(update.is_some());
            if let Some(update) = update {
                updates.push(update);
            }
        }
        // Check for unsupported tools
        if !config.tools.is_empty() {
            let mut unsupported_tools: Vec<_> = config.tools.keys().cloned().collect();
            unsupported_tools.sort();
            anyhow::bail!(
                "Unsupported pipeline tools: {}",
                unsupported_tools.join(", ")
            );
        }
        // Apply updates seperately such that pipelines are never partially loaded if one tool fails to parse into concrete types.
        for update in updates {
            update();
        }
        for (tool, loaded) in self.tools.iter_mut().zip(loaded) {
            if !loaded {
                tool.set_enabled(false);
            }
        }
        self.restore_tool_indices(&config.indices);
        self.included = included;
        Ok(())
    }

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
        let included = input
            .iter()
            .filter(|tool| tool.is_enabled())
            .map(|tool| tool.iam())
            .collect();

        Self {
            state: ManagerState {
                proxy_tools: update_proxy_tools(&input, &included),
                is_enabled: true,
            },
            tools: input,
            frontend_senders: Vec::new(),
            global_sender,
            sender,
            receiver,
            frontend_latest_state: None,
            included,
        }
    }

    fn reindex_tools(&mut self) {
        for (index, tool) in self.tools.iter_mut().enumerate() {
            tool.set_index(index);
        }
    }

    fn move_tool(&mut self, from: usize, to: usize) {
        let tool = self.tools.remove(from);
        let destination = to.min(self.tools.len());
        self.tools.insert(destination, tool);
        self.reindex_tools();
    }

    /// Adding appends the tool, including when restoring a removed tool.
    /// An existing item keeps its enabled state and position in the stack.
    fn add_tool(&mut self, kind: &BackendToolType) -> anyhow::Result<bool> {
        let Some(index) = self.tools.iter().position(|tool| tool.iam() == *kind) else {
            anyhow::bail!("Unknown pipeline tool: {kind}");
        };
        if self.included.contains(kind) {
            return Ok(false);
        }
        if let Some(tool) = self.tools.get_mut(index) {
            tool.set_enabled(true);
        }
        self.included.insert(kind.clone());
        self.move_tool(index, self.tools.len().saturating_sub(1));
        Ok(true)
    }

    fn remove_tool(&mut self, kind: &BackendToolType) -> anyhow::Result<bool> {
        let Some(index) = self.tools.iter().position(|tool| tool.iam() == *kind) else {
            anyhow::bail!("Unknown pipeline tool: {kind}");
        };
        if !self.included.contains(kind)
            && !self.tools.get(index).is_some_and(|tool| tool.is_enabled())
        {
            return Ok(false);
        }
        if let Some(tool) = self.tools.get_mut(index) {
            tool.set_enabled(false);
        }
        self.included.remove(kind);
        Ok(true)
    }

    fn pipeline_config(&self) -> anyhow::Result<(PipelineConfig, HashMap<String, u16>)> {
        let mut configs = HashMap::new();
        let mut indices = HashMap::new();
        let mut pipeline_index = 0u16;
        for tool in &self.tools {
            if !self.included.contains(&tool.iam()) {
                continue;
            }
            let name = tool.iam().to_string();
            indices.insert(name.clone(), pipeline_index);
            configs.insert(name, tool.get_state());
            pipeline_index = pipeline_index
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Too many pipeline tools"))?;
        }
        Ok((PipelineConfig { tools: configs }, indices))
    }

    pub(crate) fn restore_tool_indices(&mut self, indices: &HashMap<String, u16>) {
        if indices.is_empty() {
            return;
        }

        let mut ordered: Vec<_> = self.tools.drain(..).enumerate().collect();
        ordered.sort_by_key(|(current_index, tool)| {
            indices
                .get(&tool.iam().to_string())
                .copied()
                .map_or((1, *current_index), |index| (0, usize::from(index)))
        });
        self.tools = ordered.into_iter().map(|(_, tool)| tool).collect();
        self.reindex_tools();
    }

    /// Automatically include tools enabled outside Add Tool. Existing stack items
    /// keep their position when disabled or enabled again.
    fn move_newly_enabled_tools_to_end(&mut self) -> bool {
        let mut retained = Vec::with_capacity(self.tools.len());
        let mut added = Vec::new();
        for tool in self.tools.drain(..) {
            if tool.is_enabled() && self.included.insert(tool.iam()) {
                added.push(tool);
            } else {
                retained.push(tool);
            }
        }
        let order_changed = !added.is_empty();
        retained.append(&mut added);
        self.tools = retained;
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
                        ManagerMessage::AddTool(kind) => {
                            processing_required |= self.add_tool(&kind)?;
                        }
                        ManagerMessage::RemoveTool(kind) => {
                            processing_required |= self.remove_tool(&kind)?;
                        }
                        ManagerMessage::SavePipeline(path) => {
                            if let Err(error) = self.save_pipeline(&path) {
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(format!(
                                        "Failed to save pipeline: {error:#}"
                                    )),
                                ))?;
                                return Err(error);
                            }
                            self.global_sender.send(PipelineCommand::FrontendMessage(
                                FrontendMessage::success(
                                    "Pipeline saved successfully!".to_string(),
                                ),
                            ))?;
                        }
                        ManagerMessage::LoadPipeline(path) => {
                            if let Err(error) = self.load_pipeline(&path, false) {
                                self.global_sender.send(PipelineCommand::FrontendMessage(
                                    FrontendMessage::error(format!(
                                        "Failed to load pipeline: {error:#}"
                                    )),
                                ))?;
                                return Err(error);
                            }

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
                            self.move_tool(start_idx, new_idx);
                            processing_required = true;
                        }
                    },
                    _ => {}
                },
                _ => {}
            }
        }

        processing_required |= self.move_newly_enabled_tools_to_end();

        let new_proxy_tools = update_proxy_tools(&self.tools, &self.included);
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
