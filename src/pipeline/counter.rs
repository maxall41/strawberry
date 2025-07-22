use std::{collections::HashMap, sync::Arc};

use crate::{
    consts::{flip_rgba_to_bgra, CIRCLE_COLORS},
    processing::{contrast::GpuContrast, GpuImageProcessor},
    tools::counter::CounterTool,
};

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
    pub active_tool: CounterTool,
    pub active_count_type: Option<u16>,
}

pub struct Counter {
    pub state: CounterState,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub sender: flume::Sender<PipelineCommand>,
    pub receiver: flume::Receiver<PipelineCommand>,
    pub frontend_sender: Option<flume::Sender<PipelineCommand>>,
    pub index: usize,
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
        let mut output_images = input.clone();
        let mut sorted_counts: Vec<_> = self.state.counts.iter().collect();
        sorted_counts.sort_by_key(|(key, _)| *key);

        for image in output_images.iter_mut() {
            for (index, (_, count_types)) in sorted_counts.iter().enumerate() {
                for count in count_types.iter() {
                    // Draw a circle around the count position
                    let (cx, cy) = count.position;
                    let radius = 10;
                    let color = flip_rgba_to_bgra(CIRCLE_COLORS[index % CIRCLE_COLORS.len()]);

                    // Draw circle by setting pixels in a circular pattern
                    for angle in 0..360 {
                        let radian = (angle as f32) * std::f32::consts::PI / 180.0;
                        let x = cx as i32 + (radius as f32 * radian.cos()) as i32;
                        let y = cy as i32 + (radius as f32 * radian.sin()) as i32;

                        if x >= 0
                            && y >= 0
                            && (x as u32) < image.width()
                            && (y as u32) < image.height()
                        {
                            image.put_pixel(x as u32, y as u32, color);
                        }
                    }
                }
            }
        }
        Ok(output_images)
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
                PipelineCommand::HandleClick(x, y) => {
                    if !self.state.is_enabled {
                        return Ok(());
                    }
                    if let Some(id) = self.state.active_count_type {
                        match self.state.active_tool {
                            CounterTool::Place => {
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
                            CounterTool::Delete => {
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
                                }
                            }
                            _ => {}
                        }
                    } else {
                        eprintln!("No active count type!");
                    };
                }
                PipelineCommand::PipelineObject(message) => match message {
                    PipelineObjectMessage::ToggleActive(value) => {
                        self.state.is_enabled = value;
                    }
                    PipelineObjectMessage::CountMessage(count_message) => match count_message {
                        CountMessage::NewCountType(id) => {
                            self.state.counts.insert(id, vec![]);
                        }
                        CountMessage::SetActiveCountType(id) => {
                            self.state.active_count_type = Some(id);
                        }
                        CountMessage::SetActiveTool(tool) => {
                            self.state.active_tool = tool;
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
                if let Err(e) = self
                    .command_sender
                    .send(PipelineCommand::Refresh(self.index))
                {
                    eprintln!("Failed to send refresh command: {}", e);
                }

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

    fn get_index(&self) -> usize {
        self.index
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
    }
}

impl Counter {
    pub async fn new(command_sender: tokio::sync::broadcast::Sender<PipelineCommand>) -> Self {
        let (sender, receiver) = flume::unbounded();
        if let Err(e) = command_sender.send(PipelineCommand::RegisterClickReceiver(sender.clone()))
        {
            eprintln!("Failed to register click receiver: {}", e);
        }
        Self {
            state: CounterState {
                is_enabled: false,
                counts: HashMap::new(),
                mode: CounterMode::Manual,
                active_tool: CounterTool::Place,
                active_count_type: None,
            },
            frontend_sender: None,
            command_sender,
            sender,
            receiver,
            index: 0,
        }
    }
}
