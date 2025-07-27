// Implements UI for morphology tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::pipeline::{
    morphology::MorphologyState, BackendToolType, FrontendMessage, MorphologyMessage,
    PipelineCommand, PipelineObjectMessage, ToolState,
};
use crate::processing::morphology::MorphologyOperation;

use super::ToolFrontend;

pub struct MorphologyView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<MorphologyState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub element_size_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for MorphologyView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone().unwrap()
    }
}

impl MorphologyView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(MorphologyState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            element_size_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Morphology,
            ),
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Morphology,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Morphology");

        // Create and initialize ValueControl component
        let state = self.state.read().unwrap().clone();
        let element_size_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.0}", state.element_size),
                state.element_size as f32,
                "Element Size".to_string(),
                1.0,
                "element_size".to_string(),
                1.0,
                50.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &element_size_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "element_size" {
                    this.adjust_element_size_value(event.value as u32);
                }
            },
        )
        .detach();

        self.element_size_control = Some(element_size_control);

        self.process_messages(window, cx);
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
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
                            ToolState::MorphologyState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        let element_size_ref = new_state.element_size;
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| {
                                            this.element_size_control.as_ref().unwrap().update(
                                                cx,
                                                |control, cx| {
                                                    control.value = element_size_ref as f32;
                                                    cx.notify();
                                                },
                                            );
                                            cx.notify();
                                        }) {
                                            eprintln!(
                                                "Failed to update UI after state change: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on morphology state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Morphology");
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

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
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

    fn update_operation_and_size(
        &mut self,
        operation: MorphologyOperation,
        element_size: u32,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetOperation(
                    operation,
                )),
            )) {
                eprintln!("Failed to send operation update command: {}", e);
            }
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetElementSize(
                    element_size,
                )),
            )) {
                eprintln!("Failed to send element size update command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn set_operation(
        &mut self,
        operation: MorphologyOperation,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let element_size = match self.state.read() {
            Ok(state) => state.element_size,
            Err(e) => {
                eprintln!("Failed to read state for operation change: {}", e);
                return;
            }
        };
        self.update_operation_and_size(operation, element_size, _window, _cx);
    }

    fn adjust_element_size_value(&mut self, new_size: u32) {
        let current_state = self.state.read().unwrap();
        let operation = current_state.operation;
        drop(current_state);

        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetOperation(
                    operation,
                )),
            )) {
                eprintln!("Failed to send operation update command: {}", e);
            }

            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetElementSize(
                    new_size,
                )),
            )) {
                eprintln!("Failed to send element size update command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_element_size(&mut self, delta: i32, _window: &mut Window, _cx: &mut Context<Self>) {
        let (operation, element_size) = match self.state.read() {
            Ok(state) => (state.operation, state.element_size),
            Err(e) => {
                eprintln!("Failed to read state for element size adjustment: {}", e);
                return;
            }
        };
        let new_element_size = ((element_size as i32) + delta).max(1) as u32;
        self.update_operation_and_size(operation, new_element_size, _window, _cx);
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Morphological Operations")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &MorphologyState, cx: &mut Context<Self>) -> impl IntoElement {
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
                            move |this: &mut MorphologyView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<MorphologyView>| {
                                this.toggle_enabled(is_enabled, window, cx);
                            },
                        )
                    }),
            )
    }

    // Operation selection component
    fn render_operation_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read().unwrap();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Operation:"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.render_operation_button(
                        "Dilation".to_string(),
                        MorphologyOperation::Dilation,
                        state.operation == MorphologyOperation::Dilation,
                        cx,
                    ))
                    .child(self.render_operation_button(
                        "Erosion".to_string(),
                        MorphologyOperation::Erosion,
                        state.operation == MorphologyOperation::Erosion,
                        cx,
                    ))
                    .child(self.render_operation_button(
                        "Opening".to_string(),
                        MorphologyOperation::Opening,
                        state.operation == MorphologyOperation::Opening,
                        cx,
                    ))
                    .child(self.render_operation_button(
                        "Closing".to_string(),
                        MorphologyOperation::Closing,
                        state.operation == MorphologyOperation::Closing,
                        cx,
                    )),
            )
    }

    fn render_operation_button(
        &self,
        label: String,
        operation: MorphologyOperation,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(if is_selected {
                rgb(0x0066cc)
            } else {
                rgb(0x666666)
            })
            .text_sm()
            .text_center()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut MorphologyView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<MorphologyView>| {
                        this.set_operation(operation, window, cx);
                    },
                ),
            )
    }

    // Element size control component
    fn render_element_size_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        let esc = self.element_size_control.clone().unwrap();
        esc
    }

    fn adjust_element_size_wrapper(
        &mut self,
        delta: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adjust_element_size(delta as i32, window, cx);
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        operation: MorphologyOperation,
        element_size: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x666666))
            .text_sm()
            .text_center()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut MorphologyView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<MorphologyView>| {
                        this.update_operation_and_size(operation, element_size, window, cx);
                    },
                ),
            )
    }

    // Presets section component
    fn render_presets_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(gpui::FontWeight::BOLD).child("Presets:"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.render_preset_button(
                        "Small Dilation".to_string(),
                        MorphologyOperation::Dilation,
                        3,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Medium Erosion".to_string(),
                        MorphologyOperation::Erosion,
                        5,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Small Opening".to_string(),
                        MorphologyOperation::Opening,
                        3,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Medium Closing".to_string(),
                        MorphologyOperation::Closing,
                        5,
                        cx,
                    )),
            )
    }

    // Controls section component
    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_operation_control(cx))
            .child(self.render_element_size_control(cx, _window))
    }
}

impl Render for MorphologyView {
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
                    .child("Error: Unable to read morphology state");
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut MorphologyView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<MorphologyView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_controls_section(cx, _window))
            .child(self.render_presets_section(cx))
    }
}

impl ToolFrontend for MorphologyView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Morphological Operations")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(300.), px(500.)),
                origin: Point::new(px(470.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Morphology,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Morphology");

        // Create and initialize ValueControl component
        let state = self.state.read().unwrap().clone();
        let element_size_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.0}", state.element_size),
                state.element_size as f32,
                "Element Size".to_string(),
                1.0,
                "element_size".to_string(),
                1.0,
                50.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &element_size_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "element_size" {
                    this.adjust_element_size_value(event.value as u32);
                }
            },
        )
        .detach();

        self.element_size_control = Some(element_size_control);

        self.process_messages(window, cx);
    }
}
