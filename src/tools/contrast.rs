// Implements UI for contrast tool

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use flume;
use gpui::{
    div, prelude::*, px, rgb, size, Bounds, Context, MouseDownEvent, Point, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

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
}

impl ContrastView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender,
            state: Arc::new(RwLock::new(ContrastState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::Contrast,
                sender,
            ))
            .unwrap();
        println!("Sent discovery request to backend | Contrast");
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
                                let mut locked_state = state.write().unwrap();
                                if *locked_state != new_state {
                                    *locked_state = new_state;
                                    this.update(cx, |this, cx| cx.notify()).unwrap();
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Contrast");
                            this.update(cx, |this, cx| {
                                *this.current_tool_index.write().unwrap() = Some(backend_ids[0]);
                                this.backend_tool_indicies = backend_ids;
                                this.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.send_channels.get(0) {
                                    sender.send(PipelineCommand::RequestState()).unwrap();
                                }
                            })
                            .unwrap();
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        })
        .detach();
    }

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = self.current_tool_index.read().unwrap().unwrap();
        self.send_channels[current_tool_index as usize]
            .send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ToggleActive(!current_value),
            ))
            .unwrap();
    }

    fn update_min_max(
        &mut self,
        min: f32,
        max: f32,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = self.current_tool_index.read().unwrap().unwrap();
        self.send_channels[current_tool_index as usize]
            .send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ContrastMessage(ContrastMessage::SetMinMax(min, max)),
            ))
            .unwrap();
    }

    fn adjust_min(&mut self, delta: f32, _window: &mut Window, _cx: &mut Context<Self>) {
        let (min, max) = {
            let state = self.state.read().unwrap();
            (state.min, state.max)
        };
        let new_min = (min + delta).clamp(0.0, max - 0.01);
        self.update_min_max(new_min, max, _window, _cx);
    }

    fn adjust_max(&mut self, delta: f32, _window: &mut Window, _cx: &mut Context<Self>) {
        let (min, max) = {
            let state = self.state.read().unwrap();
            (state.min, state.max)
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
                            move |this: &mut ContrastView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<ContrastView>| {
                                this.toggle_enabled(is_enabled, window, cx);
                            },
                        )
                    }),
            )
    }

    // Value control component (reusable for min/max)
    fn render_value_control(
        &self,
        label: String,
        value: f32,
        adjust_callback: impl Fn(&mut Self, f32, &mut Window, &mut Context<Self>) + 'static + Clone,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x666666))
                            .text_sm()
                            .child(format!("{:.3}", value)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(0x444444))
                                    .text_sm()
                                    .child("-")
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        {
                                            let callback = adjust_callback.clone();
                                            cx.listener(move |this: &mut ContrastView, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<ContrastView>| {
                                                callback(this, -0.01, window, cx);
                                            })
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(0x444444))
                                    .text_sm()
                                    .child("+")
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        {
                                            let callback = adjust_callback;
                                            cx.listener(move |this: &mut ContrastView, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<ContrastView>| {
                                                callback(this, 0.01, window, cx);
                                            })
                                        },
                                    ),
                            ),
                    ),
            )
    }

    // Min value control component
    fn render_min_control(
        &self,
        state: &ContrastState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.render_value_control("Min Value:".to_string(), state.min, Self::adjust_min, cx)
    }

    // Max value control component
    fn render_max_control(
        &self,
        state: &ContrastState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.render_value_control("Max Value:".to_string(), state.max, Self::adjust_max, cx)
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
        state: &ContrastState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_min_control(state, cx))
            .child(self.render_max_control(state, cx))
    }
}

impl Render for ContrastView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read().unwrap();

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
            .child(self.render_status(&state, cx))
            .child(self.render_controls_section(&state, cx))
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
}
