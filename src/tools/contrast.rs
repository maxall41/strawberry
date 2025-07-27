// Implements UI for contrast tool

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use flume;
use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::pipeline::{
    contrast::ContrastState, BackendToolType, ContrastMessage, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolState,
};

use super::ToolFrontend;

pub struct ContrastView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<ContrastState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub min_control: Option<Entity<ValueControl>>,
    pub max_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for ContrastView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone().unwrap()
    }
}

impl ContrastView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(ContrastState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            min_control: None,
            max_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Contrast,
            ),
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Contrast,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Contrast");

        // Create and initialize ValueControl components
        let state = self.state.read().unwrap().clone();
        let min_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.min),
                state.min,
                "Min".to_string(),
                0.01,
                "min".to_string(),
                0.0,
                1.0,
            )
        });

        let max_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.max),
                state.max,
                "Max".to_string(),
                0.01,
                "max".to_string(),
                0.0,
                1.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &min_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min" {
                    this.adjust_min_value(event.value);
                }
            },
        )
        .detach();

        cx.subscribe(
            &max_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "max" {
                    this.adjust_max_value(event.value);
                }
            },
        )
        .detach();

        self.min_control = Some(min_control);
        self.max_control = Some(max_control);

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
                            ToolState::ContrastState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        let min_ref = new_state.min;
                                        let max_ref = new_state.max;
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| {
                                            this.min_control.as_ref().unwrap().update(
                                                cx,
                                                |control, cx| {
                                                    control.value = min_ref;
                                                    cx.notify();
                                                },
                                            );
                                            this.max_control.as_ref().unwrap().update(
                                                cx,
                                                |control, cx| {
                                                    control.value = max_ref;
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
                                    eprintln!("Failed to acquire write lock on contrast state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Contrast");
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

    fn update_min_max(
        &mut self,
        min: f32,
        max: f32,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ContrastMessage(ContrastMessage::SetMinMax(min, max)),
            )) {
                eprintln!("Failed to send min/max update command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_min_value(&mut self, new_min: f32) {
        let current_state = self.state.read().unwrap();
        let max_val = current_state.max;
        drop(current_state);

        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ContrastMessage(ContrastMessage::SetMinMax(
                    new_min, max_val,
                )),
            )) {
                eprintln!("Failed to send min/max update command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_max_value(&mut self, new_max: f32) {
        let current_state = self.state.read().unwrap();
        let min_val = current_state.min;
        drop(current_state);

        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ContrastMessage(ContrastMessage::SetMinMax(
                    min_val, new_max,
                )),
            )) {
                eprintln!("Failed to send min/max update command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_min(&mut self, delta: f32, _window: &mut Window, _cx: &mut Context<Self>) {
        let (min, max) = match self.state.read() {
            Ok(state) => (state.min, state.max),
            Err(e) => {
                eprintln!("Failed to read state for min adjustment: {}", e);
                return;
            }
        };
        let new_min = (min + delta).clamp(0.0, max - 0.01);
        self.update_min_max(new_min, max, _window, _cx);
    }

    fn adjust_max(&mut self, delta: f32, _window: &mut Window, _cx: &mut Context<Self>) {
        let (min, max) = match self.state.read() {
            Ok(state) => (state.min, state.max),
            Err(e) => {
                eprintln!("Failed to read state for max adjustment: {}", e);
                return;
            }
        };
        let new_max = (max + delta).clamp(min + 0.01, 1.0);
        self.update_min_max(min, new_max, _window, _cx);
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Contrast")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &ContrastState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut ContrastView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<ContrastView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .render(cx)
    }

    // Value control component (reusable for min/max)

    // Min value control component
    fn render_min_control(&self, cx: &mut Context<Self>, _window: &mut Window) -> impl IntoElement {
        let mc = self.min_control.clone().unwrap();
        mc
    }

    // Max value control component
    fn render_max_control(&self, cx: &mut Context<Self>, _window: &mut Window) -> impl IntoElement {
        let mc = self.max_control.clone().unwrap();
        mc
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        min: f32,
        max: f32,
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
                    move |this: &mut ContrastView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<ContrastView>| {
                        this.update_min_max(min, max, window, cx);
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
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Quick Presets:"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.render_preset_button("Reset (0.0 - 1.0)".to_string(), 0.0, 1.0, cx))
                    .child(self.render_preset_button(
                        "High Contrast (0.2 - 0.8)".to_string(),
                        0.2,
                        0.8,
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
            .child(self.render_min_control(cx, _window))
            .child(self.render_max_control(cx, _window))
    }
}

impl Render for ContrastView {
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
                    .child("Error: Unable to read contrast state");
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
                    move |this: &mut ContrastView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<ContrastView>| {
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

impl ToolFrontend for ContrastView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Contrast")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(400.)),
                origin: Point::new(px(460.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Contrast,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Contrast");

        // Create and initialize ValueControl components
        let state = self.state.read().unwrap().clone();
        let min_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.min),
                state.min,
                "Min".to_string(),
                0.01,
                "min".to_string(),
                0.0,
                1.0,
            )
        });

        let max_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.max),
                state.max,
                "Max".to_string(),
                0.01,
                "max".to_string(),
                0.0,
                1.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &min_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min" {
                    this.adjust_min_value(event.value);
                }
            },
        )
        .detach();

        cx.subscribe(
            &max_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "max" {
                    this.adjust_max_value(event.value);
                }
            },
        )
        .detach();

        self.min_control = Some(min_control);
        self.max_control = Some(max_control);

        self.process_messages(window, cx);
    }
}
