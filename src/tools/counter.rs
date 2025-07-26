// Implements UI for counter tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, Bounds, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use rfd::FileDialog;

use crate::{
    gui::value_control::{render_button_value_control, ButtonControlConfig},
    pipeline::{
        counter::{CountData, CounterMode, CounterState},
        BackendToolType, CountMessage, CrossFrontendMessage, FilterMessage, FrontendMessage,
        PipelineCommand, PipelineObjectMessage, ToolState,
    },
    utils::consts::{image_rgba_to_gpui_rgba, CIRCLE_COLORS},
};

use super::ToolFrontend;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum CounterTool {
    #[default]
    Place,
    Delete,
    LassoDelete,
    Review,
}

pub struct CounterView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<CounterState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub next_count_type_id: Arc<RwLock<u16>>,
    _cleanup_guard: crate::tools::CleanupGuard,
}

impl CounterView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(CounterState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            next_count_type_id: Arc::new(RwLock::new(0)),
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Counter,
            ),
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver.clone());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Counter,
            sender.clone(),
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Counter");
        self.process_messages(window, cx);
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let next_count_type_id = self.next_count_type_id.clone();
        let channel_receiver_ref = self
            .receive_channel
            .clone()
            .expect("Should never fire: Created during init");

        // Spawn message processing thread
        cx.spawn_in(window, async move |this, cx| loop {
            if let Ok(command) = channel_receiver_ref.recv_async().await {
                match command {
                    PipelineCommand::FrontendMessage(message) => match message {
                        FrontendMessage::ReturnState(tool_state) => match tool_state {
                            ToolState::CounterState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        // Update next_count_type_id based on existing count types
                                        if let Ok(mut next_id) = next_count_type_id.write() {
                                            *next_id = new_state
                                                .manual_counts
                                                .keys()
                                                .max()
                                                .map(|&k| k + 1)
                                                .unwrap_or(0);
                                        }

                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |_this, cx| cx.notify()) {
                                            eprintln!(
                                                "Failed to update UI after state change: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on counter state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Counter");
                            if let Err(e) = this.update(cx, |this, cx| {
                                if let Ok(mut current_tool_index) = this.current_tool_index.write()
                                {
                                    *current_tool_index = backend_ids.get(0).copied();
                                } else {
                                    eprintln!("Failed to acquire write lock on current tool index");
                                    return;
                                }
                                this.backend_tool_indicies = backend_ids;
                                this.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.send_channels.get(0) {
                                    if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                        eprintln!("Failed to request initial state: {}", e);
                                    }
                                }
                            }) {
                                eprintln!("Failed to update backend tools: {}", e);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            } else {
                break;
            }
        })
        .detach();
    }

    fn get_current_tool_index(&self) -> Option<u16> {
        match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => Some(index),
                None => {
                    eprintln!("No current tool index available");
                    None
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                None
            }
        }
    }

    fn save_counts(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("xml", &["xml"])
                .set_directory("/")
                .save_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::SaveCounts(file)),
                        )) {
                            eprintln!("Failed to send save counts command: {}", e);
                            return;
                        }
                    } else {
                        eprintln!("Invalid tool index: {}", current_tool_index);
                        return;
                    }
                }) {
                    eprintln!("Failed to update count types: {}", e);
                    return;
                }
            }
        })
        .detach();
    }

    fn load_counts(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("xml", &["xml"])
                .set_directory("/")
                .pick_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::LoadCounts(file)),
                        )) {
                            eprintln!("Failed to send load counts command: {}", e);
                            return;
                        }
                    } else {
                        eprintln!("Invalid tool index: {}", current_tool_index);
                        return;
                    }
                }) {
                    eprintln!("Failed to update count types: {}", e);
                    return;
                }
            }
        })
        .detach();
    }

    fn add_count_type(&mut self) {
        let new_id = match self.next_count_type_id.read() {
            Ok(guard) => *guard,
            Err(e) => {
                eprintln!("Failed to read next count type ID: {}", e);
                return;
            }
        };

        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::NewCountType(new_id)),
            )) {
                eprintln!("Failed to send new count type command: {}", e);
                return;
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
            return;
        }

        // Increment the next ID
        if let Ok(mut next_id) = self.next_count_type_id.write() {
            *next_id += 1;
        }

        // Set as selected by sending message to backend
        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetActiveCountType(new_id)),
            )) {
                eprintln!("Failed to send set active count type command: {}", e);
            }
        }

        println!("Added count type {}", new_id);
    }

    fn delete_count_type(&mut self, count_type_id: u16) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::DeleteCountType(count_type_id)),
            )) {
                eprintln!("Failed to send delete count type command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }

        // Clear selection if this was the selected type
        if let Ok(state) = self.state.read() {
            if state.active_count_type == Some(count_type_id) {
                if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
                    // Set active count type to None by finding another type or clearing
                    let remaining_types: Vec<u16> = state
                        .manual_counts
                        .keys()
                        .filter(|&&id| id != count_type_id)
                        .copied()
                        .collect();
                    let new_active = remaining_types.first().copied();

                    if let Some(new_active_id) = new_active {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::SetActiveCountType(
                                new_active_id,
                            )),
                        )) {
                            eprintln!("Failed to send set active count type command: {}", e);
                        }
                    }
                }
            }
        }
    }

    fn select_count_type(
        &mut self,
        count_type_id: u16,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetActiveCountType(
                    count_type_id,
                )),
            )) {
                eprintln!("Failed to send set active count type command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn copy_to_manual(&self) {
        let current_tool_index = match self.get_current_tool_index() {
            Some(index) => index,
            None => {
                eprintln!("No current tool index available for copy to manual");
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::CopyToManual),
            )) {
                eprintln!("Failed to send copy to manual command: {}", e);
            }
        } else {
            eprintln!("No sender available for copy to manual command");
        }
    }

    fn set_tool(&mut self, tool: CounterTool, _window: &mut Window, _cx: &mut Context<Self>) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetActiveTool(tool)),
            )) {
                eprintln!("Failed to send set active tool command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ToggleActive(!current_value),
            )) {
                eprintln!("Failed to send toggle enabled command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn toggle_mode(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        let new_mode = match self.state.read() {
            Ok(state) => match state.mode {
                CounterMode::Manual => CounterMode::Auto,
                CounterMode::Auto => CounterMode::Manual,
            },
            Err(e) => {
                eprintln!("Failed to read state for mode toggle: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetMode(new_mode)),
            )) {
                eprintln!("Failed to send set mode command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Counter")
    }

    // Subheader component
    fn render_subheader(&self) -> impl IntoElement {
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_2()
            .child("Counts:")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .mb_3()
            .child("Enabled:")
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(if state.is_enabled {
                        rgb(0x00aa00)
                    } else {
                        rgb(0xaa0000)
                    })
                    .child(if state.is_enabled { "Yes" } else { "No" })
                    .on_mouse_down(gpui::MouseButton::Left, {
                        let is_enabled = state.is_enabled;
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<CounterView>| {
                                this.toggle_enabled(is_enabled, window, cx);
                            },
                        )
                    }),
            )
    }

    // Mode toggle component
    fn render_mode_toggle(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .mb_3()
            .child("Mode:")
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x444444))
                    .hover(|style| style.bg(rgb(0x555555)))
                    .child(match state.mode {
                        CounterMode::Manual => "Manual",
                        CounterMode::Auto => "Auto",
                    })
                    .on_mouse_down(gpui::MouseButton::Left, {
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<CounterView>| {
                                this.toggle_mode(window, cx);
                            },
                        )
                    }),
            )
    }

    // Individual count type item component
    fn render_count_type_item(
        &self,
        count_type_id: u16,
        count_data: &Vec<crate::pipeline::counter::CountData>,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ii = (count_type_id as usize % CIRCLE_COLORS.len()) as usize;
        let color = image_rgba_to_gpui_rgba(CIRCLE_COLORS[ii]);
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .mb_1()
            .bg(rgb(0x444444))
            .rounded_md()
            .child(
                // X button
                div()
                    .w_4()
                    .h_4()
                    .bg(rgb(0xaa0000))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(0xffffff))
                    .child("×")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<CounterView>| {
                                this.delete_count_type(count_type_id);
                            },
                        ),
                    ),
            )
            .child(
                // Type label and count
                div()
                    .flex_1()
                    .child(format!("Type {} ({})", count_type_id, count_data.len())),
            )
            .child(
                // Selection indicator
                div()
                    .w_3()
                    .h_3()
                    .rounded_full()
                    .bg(if is_selected { color } else { rgb(0x888888) })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<CounterView>| {
                                this.select_count_type(count_type_id, window, cx);
                            },
                        ),
                    ),
            )
    }

    // Count types list component
    fn render_count_types_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(_) => {
                return div().child("Error reading state");
            }
        };

        let selected_type = state.active_count_type;

        div().flex().flex_col().gap_1().children({
            let mut sorted_counts: Vec<_> = state.manual_counts.iter().collect();
            sorted_counts.sort_by_key(|(id, _)| **id);
            sorted_counts
                .into_iter()
                .map(|(&count_type_id, count_data)| {
                    let is_selected = selected_type == Some(count_type_id);
                    self.render_count_type_item(count_type_id, count_data, is_selected, cx)
                })
        })
    }

    // Plus button component
    fn render_add_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .p_2()
            .mt_2()
            .bg(rgb(0x666666))
            .hover(|style| style.bg(rgb(0x777777)))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .text_lg()
            .child("+")
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    |this: &mut CounterView,
                     _event: &MouseDownEvent,
                     _window: &mut Window,
                     _cx: &mut Context<CounterView>| {
                        this.add_count_type();
                    },
                ),
            )
    }

    // Tool button component
    fn render_tool_button(
        &self,
        tool: CounterTool,
        label: String,
        is_active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_1()
            .p_2()
            .bg(if is_active {
                rgb(0x0066cc)
            } else {
                rgb(0x444444)
            })
            .hover(|style| {
                style.bg(if is_active {
                    rgb(0x0077dd)
                } else {
                    rgb(0x555555)
                })
            })
            .text_center()
            .text_sm()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut CounterView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<CounterView>| {
                        this.set_tool(tool.clone(), window, cx);
                    },
                ),
            )
    }

    // Tools subheader component
    fn render_tools_subheader(&self) -> impl IntoElement {
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_2()
            .child("Tools:")
    }

    // Tools bar component
    fn render_tools_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(_) => {
                return div().child("Error reading state");
            }
        };
        let current_tool = &state.active_tool;

        div()
            .flex()
            .bg(rgb(0x333333))
            .rounded_md()
            .overflow_hidden()
            .mb_4()
            .child(self.render_tool_button(
                CounterTool::Place,
                "Place".to_string(),
                *current_tool == CounterTool::Place,
                cx,
            ))
            .child(self.render_tool_button(
                CounterTool::Delete,
                "Delete".to_string(),
                *current_tool == CounterTool::Delete,
                cx,
            ))
            .child(self.render_tool_button(
                CounterTool::LassoDelete,
                "Lasso | Delete".to_string(),
                *current_tool == CounterTool::LassoDelete,
                cx,
            ))
            .child(self.render_tool_button(
                CounterTool::Review,
                "Review".to_string(),
                *current_tool == CounterTool::Review,
                cx,
            ))
    }

    // Save button component
    fn render_storage_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x006600))
                    .hover(|style| style.bg(rgb(0x007700)))
                    .rounded_md()
                    .text_sm()
                    .child("Save")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut CounterView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<CounterView>| {
                                _this.save_counts(_window, _cx);
                            },
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x0066cc))
                    .hover(|style| style.bg(rgb(0x0077dd)))
                    .rounded_md()
                    .text_sm()
                    .child("Load")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut CounterView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<CounterView>| {
                                // TODO: Implement load functionality
                                _this.load_counts(_window, _cx);
                            },
                        ),
                    ),
            )
    }

    // Manual UI rendering
    fn render_manual_ui(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        let mut ui = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_subheader())
            .child(self.render_count_types_list(cx))
            .child(self.render_add_button(cx))
            .child(self.render_tools_subheader())
            .child(self.render_tools_bar(cx));

        // Add review UI when Review tool is active
        if state.active_tool == CounterTool::Review {
            ui = ui.child(self.render_review_ui(state, cx));
        }

        ui
    }

    // Auto UI rendering (placeholder)
    fn render_auto_ui(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Auto Detection"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mb_3()
                    .child("Detected Counts:")
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x444444))
                            .child(format!("{}", state.auto_counts.len())),
                    ),
            )
            .child(self.render_filters_section(state, cx))
    }

    fn render_filters_section(
        &self,
        state: &CounterState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_md()
            .bg(rgb(0x2a2a2a))
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_lg()
                    .child("Filters"),
            )
            .when(state.filter_proxys.is_empty(), |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xaaaaaa))
                        .child("No filters available"),
                )
            })
            .when(!state.filter_proxys.is_empty(), |el| {
                el.child(
                    div().flex().flex_col().gap_2().children(
                        state
                            .filter_proxys
                            .iter()
                            .enumerate()
                            .map(|(index, filter_proxy)| {
                                self.render_filter_item(filter_proxy, index, cx)
                            }),
                    ),
                )
            })
    }

    fn render_filter_item(
        &self,
        filter_proxy: &crate::pipeline::counter::FilterProxy,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_enabled = filter_proxy.is_enabled;
        let filter_index = index as u16;

        let is_size_filter = filter_proxy.name == "Filter by Size";

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(if is_enabled {
                rgb(0x404040)
            } else {
                rgb(0x333333)
            })
            .hover(|style| {
                style.bg(if is_enabled {
                    rgb(0x505050)
                } else {
                    rgb(0x404040)
                })
            })
            .border_1()
            .border_color(if is_enabled {
                rgb(0x00aa00)
            } else {
                rgb(0x555555)
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(filter_proxy.name.clone()),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .bg(if is_enabled {
                                        rgb(0x00aa00)
                                    } else {
                                        rgb(0xaa0000)
                                    })
                                    .child(if is_enabled { "ACTIVE" } else { "DISABLED" }),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xaaaaaa))
                            .child("Click to toggle"),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<CounterView>| {
                                this.toggle_filter(filter_index, !is_enabled);
                            },
                        ),
                    ),
            )
            .when(is_size_filter, |el| {
                let current_value = filter_proxy
                    .values
                    .get(0)
                    .map(|v| *v as f32)
                    .unwrap_or(10.0);

                let config = ButtonControlConfig {
                    step: 1.0,
                    decimal_places: 0,
                };

                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(rgb(0x555555))
                        .child(render_button_value_control(
                            "Minimum Object Size".to_string(),
                            current_value,
                            config,
                            |this: &mut CounterView,
                             delta: f32,
                             _window: &mut Window,
                             _cx: &mut Context<CounterView>| {
                                this.adjust_size_filter(delta as i16);
                            },
                            cx,
                        )),
                )
            })
    }

    fn toggle_filter(&mut self, filter_index: u16, enabled: bool) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::ToggleFilter(
                    filter_index,
                    enabled,
                )),
            )) {
                eprintln!("Failed to send toggle filter command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_size_filter(&mut self, delta: i16) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            let current_value = {
                let state = self.state.read().unwrap();
                state
                    .filter_proxys
                    .iter()
                    .find(|f| f.name == "Filter by Size")
                    .and_then(|f| f.values.get(0))
                    .copied()
                    .unwrap_or(10.0)
            };

            let new_min_size = ((current_value as i16) + delta).max(1) as u16;

            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::FilterMessage(
                    FilterMessage::ConfigureSizeFilter(new_min_size),
                )),
            )) {
                eprintln!("Failed to send size filter configuration: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn render_review_ui(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        let current_index = state.review_current_index;

        let review_counts = if let Some(active_count_type) = state.active_count_type {
            state.manual_counts.get(&active_count_type).unwrap() //TODO: Handle error
        } else {
            &vec![]
        };

        if review_counts.is_empty() {
            return div().p_3().bg(rgb(0x333333)).rounded_md().child(
                div()
                    .text_center()
                    .text_sm()
                    .child("No counts to review. Add some counts first."),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(0x333333))
            .rounded_md()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_sm()
                            .child("Review Mode"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(format!("{} / {}", current_index + 1, review_counts.len())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(rgb(0x666666))
                            .hover(|style| style.bg(rgb(0x777777)))
                            .rounded_md()
                            .text_sm()
                            .child("← Prev")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    |this: &mut CounterView,
                                     _event: &MouseDownEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<CounterView>| {
                                        this.send_review_navigate_previous();
                                        _cx.notify();
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(rgb(0x666666))
                            .hover(|style| style.bg(rgb(0x777777)))
                            .rounded_md()
                            .text_sm()
                            .child("Next →")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    |this: &mut CounterView,
                                     _event: &MouseDownEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<CounterView>| {
                                        this.send_review_navigate_next();
                                        _cx.notify();
                                    },
                                ),
                            ),
                    )
                    .child(if current_index < review_counts.len() {
                        let count = &review_counts[current_index];
                        div()
                            .flex_1()
                            .text_right()
                            .text_sm()
                            .child(format!("({}, {})", count.position.0, count.position.1))
                    } else {
                        div()
                    }),
            )
    }

    // Review functionality methods - now sends messages to backend
    fn send_review_navigate_previous(&self) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::ReviewNavigatePrevious),
            )) {
                eprintln!("Failed to send review navigate previous command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn send_review_navigate_next(&self) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::ReviewNavigateNext),
            )) {
                eprintln!("Failed to send review navigate next command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }
}

impl Render for CounterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(e) => {
                eprintln!("Failed to read state for rendering: {}", e);
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(0x505050))
                    .size_full()
                    .p_4()
                    .shadow_lg()
                    .text_color(rgb(0xffffff))
                    .child("Error: Unable to read counter state");
            }
        };

        div()
            .flex()
            .flex_col()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_mode_toggle(&state, cx))
            .child(match state.mode {
                CounterMode::Manual => div().child(self.render_manual_ui(&state, cx)),
                CounterMode::Auto => div().child(self.render_auto_ui(&state, cx)),
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .mt_auto()
                    .child(self.render_storage_buttons(cx))
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .bg(rgb(0x0066cc))
                            .hover(|style| style.bg(rgb(0x0077dd)))
                            .rounded_md()
                            .text_sm()
                            .child("Copy to Manual")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    |this: &mut CounterView,
                                     _event: &MouseDownEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<CounterView>| {
                                        this.copy_to_manual();
                                    },
                                ),
                            )
                    ),
            )
    }
}

impl ToolFrontend for CounterView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Counter")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(400.), px(700.)),
                origin: Point::new(px(720.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver.clone());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Counter,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Counter");
        self.process_messages(window, cx);
    }
}
