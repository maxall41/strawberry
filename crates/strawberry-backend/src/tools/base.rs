use crate::utils::commands::collect_and_deduplicate_commands;
use strawberry_types::PipelineCommand;

/// Common infrastructure shared by all tools
/// Use composition: tools embed this struct as a field
pub struct ToolBase {
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_senders: Vec<flume::Sender<PipelineCommand>>,
    pub index: usize,
}

impl ToolBase {
    pub fn new(
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        global_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            command_sender,
            global_sender,
            sender,
            receiver,
            frontend_senders: Vec::new(),
            index: 0,
        }
    }

    pub fn send_to_frontend(&self, message: PipelineCommand) -> anyhow::Result<()> {
        for sender in &self.frontend_senders {
            sender
                .try_send(message.clone())
                .map_err(|e| anyhow::anyhow!("Failed to send to frontend: {}", e))?;
        }
        Ok(())
    }

    pub fn collect_commands(&self) -> Vec<PipelineCommand> {
        collect_and_deduplicate_commands(&self.receiver)
    }

    pub fn trigger_refresh(&self) -> anyhow::Result<()> {
        self.command_sender
            .send(PipelineCommand::Refresh(self.index))
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("Failed to send refresh: {}", e))
    }
}

/// Macro to implement the repetitive parts of ToolBackend trait
/// Tools only need to implement: process(), receive_messages(), get_state(), apply_state(), iam()
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

        fn get_index(&self) -> usize {
            self.base.index
        }

        fn set_index(&mut self, index: usize) {
            self.base.index = index;
        }

        fn has_valid_cache(&self) -> bool {
            self.cache.cached_state.as_ref() == Some(&self.state)
        }

        fn get_cached_output(&self) -> Option<Vec<image::RgbaImage>> {
            self.cache.cached_output.clone()
        }

        fn set_cached_output(&mut self, output: Vec<image::RgbaImage>) {
            self.cache.cached_output = Some(output);
            self.cache.cached_state = Some(self.state.clone());
        }

        fn invalidate_cache(&mut self) {
            self.cache.cached_output = None;
            self.cache.cached_state = None;
        }
    };
}
