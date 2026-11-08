use strawberry_types::PipelineCommand;

/// Common infrastructure shared by all tools
/// Use composition: tools embed this struct as a field
pub struct ToolBase {
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
}

impl ToolBase {
    #[must_use]
    pub fn new(global_sender: tokio::sync::broadcast::Sender<PipelineCommand>) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            global_sender,
            sender,
            receiver,
            frontend_senders: Vec::new(),
            index: 0,
        }
    }

    /// # Errors
    /// Returns an error if a command fails or a communication channel is disconnected.
    pub fn send_to_frontend(&self, message: &PipelineCommand) -> anyhow::Result<()> {
        for sender in &self.frontend_senders {
            sender
                .try_send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send to frontend: {e}"))?;
        }
        Ok(())
    }

    #[must_use]
    pub fn collect_commands(&self) -> Vec<PipelineCommand> {
        std::iter::from_fn(|| self.receiver.try_recv().ok()).collect()
    }
}

/// Macro to implement the repetitive parts of `ToolBackend` trait
/// Tools only need to implement: `process()`, `receive_messages()`, `get_state()`, `apply_state()`, `iam()`
#[macro_export]
macro_rules! impl_tool_backend_basics {
    ($tool_type:ty) => {
        fn is_enabled(&self) -> bool {
            self.state.is_enabled
        }

        fn set_enabled(&mut self, enabled: bool) {
            self.state.is_enabled = enabled;
        }

        fn get_sender(&self) -> flume::Sender<strawberry_types::PipelineCommand> {
            self.base.sender.clone()
        }

        fn add_frontend_sender(
            &mut self,
            sender: flume::Sender<strawberry_types::PipelineCommand>,
        ) {
            self.base.frontend_senders.push(sender);
        }

        fn disconnect(&mut self) {
            self.base.frontend_senders.clear();
        }

        fn set_index(&mut self, index: usize) {
            self.base.index = index;
        }

        $crate::impl_tool_cache!();

        fn sync_frontend(&mut self) -> anyhow::Result<()> {
            if self.cache.sync.needs_frontend_sync(&self.state) {
                self.base
                    .send_to_frontend(&strawberry_types::PipelineCommand::FrontendMessage(
                        strawberry_types::FrontendMessage::ReturnState(self.get_state()),
                    ))?;
                self.cache.sync.frontend_latest_state = Some(self.state.clone());
            }
            Ok(())
        }
    };
}

/// Shared cache and reconciliation lifecycle, also used by interactive tools.
#[macro_export]
macro_rules! impl_tool_cache {
    () => {
        fn reconcile_state(&mut self) -> bool {
            self.cache.reconcile(&self.state)
        }

        fn acknowledge_processed_state(&mut self) {
            self.cache.sync.acknowledge(&self.state);
        }

        fn valid_cached_output(&self) -> Option<Vec<std::sync::Arc<image::RgbaImage>>> {
            self.cache.valid_output(&self.state)
        }

        fn set_cached_output(&mut self, output: Vec<std::sync::Arc<image::RgbaImage>>) {
            self.cache.cached_output = Some(output);
            self.cache.cached_state = Some(self.state.clone());
        }

        fn invalidate_cache(&mut self) {
            self.cache.invalidate();
        }
    };
}
