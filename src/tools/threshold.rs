// Implements UI for threshold tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, Bounds, Context, MouseDownEvent, Point, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{render_button_value_control, ButtonControlConfig};
use crate::pipeline::{
    threshold::ThresholdState, BackendToolType, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ThresholdMessage, ToolState,
};

use super::ToolFrontend;

pub struct ThresholdView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<ThresholdState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    _cleanup_guard: crate::tools::CleanupGuard,
}

impl ThresholdView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(ThresholdState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Threshold,
            ),
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Threshold,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Threshold");
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
                            ToolState::ThresholdState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| cx.notify()) {
                                            eprintln!(
                                                "Failed to update UI after state change: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on threshold state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Threshold");
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

    fn toggle_enabled(&mut self, current_value: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    fn update_threshold_value(&mut self, threshold_value: f32) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ThresholdMessage(ThresholdMessage::SetThresholdValue(
                    threshold_value,
                )),
            )) {
                eprintln!("Failed to send threshold value update: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_threshold(&mut self, new_threshold: f32) {
        let clamped_threshold = new_threshold.clamp(0.0, 1.0);
        self.update_threshold_value(clamped_threshold);
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Threshold")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &ThresholdState) -> impl IntoElement {
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
                    .child(if state.is_enabled { "Yes" } else { "No" }),
            )
    }

    // Threshold value control component
    fn render_threshold_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read().unwrap();
        let config = ButtonControlConfig {
            step: 0.01,
            decimal_places: 3,
        };

        render_button_value_control(
            "Threshold".to_string(),
            state.threshold_value,
            config,
            |this: &mut ThresholdView,
             delta: f32,
             _window: &mut Window,
             _cx: &mut Context<ThresholdView>| {
                let current_threshold = {
                    let current_state = this.state.read().unwrap();
                    current_state.threshold_value
                };
                let new_threshold = (current_threshold + delta).clamp(0.0, 1.0);
                this.adjust_threshold(new_threshold);
            },
            cx,
        )
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        threshold_value: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .bg(rgb(0x444444))
            .hover(|style| style.bg(rgb(0x555555)))
            .rounded_md()
            .text_sm()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut ThresholdView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<ThresholdView>| {
                        this.update_threshold_value(threshold_value);
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
                    .child(self.render_preset_button("Dark (0.25)".to_string(), 0.25, cx))
                    .child(self.render_preset_button("Mid (0.5)".to_string(), 0.5, cx))
                    .child(self.render_preset_button("Light (0.75)".to_string(), 0.75, cx))
                    .child(self.render_preset_button("Bright (0.9)".to_string(), 0.9, cx)),
            )
    }

    // Controls section component
    fn render_controls_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_threshold_control(cx))
    }
}

impl Render for ThresholdView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state.clone(),
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
                    .child("Error: Unable to read threshold state");
            }
        };

        let is_enabled = state.is_enabled;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
            .child(self.render_header())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.render_status(&state))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut ThresholdView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<ThresholdView>| {
                                this.toggle_enabled(is_enabled, window, cx);
                            },
                        ),
                    ),
            )
            .child(self.render_controls_section(cx))
            .child(self.render_presets_section(cx))
    }
}

impl ToolFrontend for ThresholdView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Threshold")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(350.)),
                origin: Point::new(px(750.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Threshold,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Threshold");
        self.process_messages(window, cx);
    }
}
