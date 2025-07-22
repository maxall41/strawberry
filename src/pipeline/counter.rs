use std::{collections::HashMap, sync::Arc};

use crate::processing::{contrast::GpuContrast, GpuImageProcessor};

use super::{
    BackendToolType, ContrastMessage, CountMessage, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolBackend, ToolState,
};
use async_trait::async_trait;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CountData {
    pub position: (u64, u64), // X, Y
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum CounterMode {
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CounterState {
    pub is_enabled: bool,
    pub counts: HashMap<u16, Vec<CountData>>,
    pub mode: CounterMode,
}

pub struct Counter {
    pub state: CounterState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_sender: Option<flume::Sender<PipelineCommand>>,
}

#[async_trait]
impl ToolBackend for Counter {
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
        ToolState::CounterState(self.state.clone())
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
        // Forward images
        Ok(input)
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
                    PipelineObjectMessage::CountMessage(count_message) => match count_message {
                        CountMessage::NewCountType(id) => {
                            self.state.counts.insert(id, vec![]);
                        }
                        CountMessage::PlaceCount(id, x, y) => {
                            if self.state.counts.contains_key(&id) {
                                let mut count_type = self
                                    .state
                                    .counts
                                    .get(&id)
                                    .expect("Should never fire: Confirmed that key exists")
                                    .clone();
                                count_type.push(CountData { position: (x, y) });
                                self.state.counts.insert(id, count_type);
                            } else {
                                self.state
                                    .counts
                                    .insert(id, vec![CountData { position: (x, y) }]);
                            }
                        }
                        CountMessage::DeleteCount(id, x, y) => {
                            if self.state.counts.contains_key(&id) {
                                let counts = self
                                    .state
                                    .counts
                                    .get(&id)
                                    .expect("Should never fire: Confirmed that key exists")
                                    .clone();
                                let new_counts: Vec<CountData> = counts
                                    .iter()
                                    .filter(|count| count.position != (x, y))
                                    .cloned()
                                    .collect();
                                self.state.counts.insert(id, new_counts);
                            } else {
                                eprintln!("Count type not found!")
                            }
                        }
                        CountMessage::DeleteLasso(id, polygon) => {
                            eprintln!("Not implemented")
                        }
                        CountMessage::DeleteCountType(id) => {
                            self.state.counts.remove(&id);
                        }
                        CountMessage::SetMode(new_mode) => {
                            self.state.mode = new_mode;
                        }
                    },
                    _ => {}
                },
                _ => {}
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
        }

        Ok(())
    }

    fn iam(&self) -> BackendToolType {
        BackendToolType::Counter
    }
}

impl Counter {
    pub async fn new(command_sender: tokio::sync::broadcast::Sender<PipelineCommand>) -> Self {
        let (sender, receiver) = flume::unbounded();
        Self {
            state: CounterState {
                is_enabled: false,
                counts: HashMap::new(),
                mode: CounterMode::Manual,
            },
            frontend_sender: None,
            command_sender,
            sender,
            receiver,
        }
    }
}
