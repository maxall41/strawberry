use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::pipeline::{
    sobel::SobelState, BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage,
    SobelMessage, ToolState,
};

use super::ToolFrontend;

pub struct SobelView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<SobelState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub threshold_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for SobelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone().unwrap()
    }
}

impl SobelView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(SobelState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            threshold_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Sobel,
            ),
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Sobel,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Sobel");

        let state = self.state.read().unwrap().clone();
        let threshold_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.threshold_value),
                state.threshold_value,
                "Threshold".to_string(),
                0.001,
                "threshold".to_string(),
                0.0,
                1.0,
            )
        });

        cx.subscribe(
            &threshold_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "threshold" {
                    this.adjust_threshold(event.value);
                }
            },
        )
        .detach();

        self.threshold_control = Some(threshold_control);

        self.process_messages(window, cx);
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let channel_receiver_ref = self
            .receive_channel
            .clone()
            .expect("Should never fire: Created during init");

        cx.spawn_in(window, async move |this, cx| loop {
            if let Ok(command) = channel_receiver_ref.recv_async().await {
                match command {
                    PipelineCommand::FrontendMessage(message) => match message {
                        FrontendMessage::ReturnState(tool_state) => match tool_state {
                            ToolState::SobelState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        let threshold_value = new_state.threshold_value;
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| {
                                            this.threshold_control.as_ref().unwrap().update(
                                                cx,
                                                |control, cx| {
                                                    control.value = threshold_value;
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
                                    eprintln!("Failed to acquire write lock on sobel state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Sobel");
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

    fn toggle_threshold_enabled(&mut self, current_value: bool) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::SobelMessage(SobelMessage::ToggleThreshold(!current_value)),
            )) {
                eprintln!("Failed to send toggle threshold command: {}", e);
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
                PipelineObjectMessage::SobelMessage(SobelMessage::SetThreshold(threshold_value)),
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

    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Sobel Edge Detection")
    }

    fn render_status(&self, state: &SobelState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut SobelView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<SobelView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .render(cx)
    }

    fn render_threshold_toggle(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let threshold_enabled = state.use_threshold;
        ToggleButton::new(
            "Use Threshold".to_string(),
            threshold_enabled,
            move |this: &mut SobelView,
                  _event: &MouseDownEvent,
                  _window: &mut Window,
                  _cx: &mut Context<SobelView>| {
                this.toggle_threshold_enabled(threshold_enabled);
            },
        )
        .render(cx)
    }

    fn render_threshold_control(
        &self,
        state: &SobelState,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        if state.use_threshold {
            div().child(self.threshold_control.clone().unwrap())
        } else {
            div()
        }
    }

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
                    move |this: &mut SobelView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<SobelView>| {
                        this.update_threshold_value(threshold_value);
                    },
                ),
            )
    }

    fn render_presets_section(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if state.use_threshold {
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
                        .child(self.render_preset_button("Low (0.1)".to_string(), 0.1, cx))
                        .child(self.render_preset_button("Medium (0.3)".to_string(), 0.3, cx))
                        .child(self.render_preset_button("High (0.5)".to_string(), 0.5, cx))
                        .child(self.render_preset_button("Very High (0.7)".to_string(), 0.7, cx)),
                )
        } else {
            div()
        }
    }

    fn render_controls_section(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_threshold_toggle(state, cx))
            .child(self.render_threshold_control(state, cx, _window))
    }
}

impl Render for SobelView {
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
                    .child("Error: Unable to read sobel state");
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
                    move |this: &mut SobelView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<SobelView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_controls_section(&state, cx, _window))
            .child(self.render_presets_section(&state, cx))
    }
}

impl ToolFrontend for SobelView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Sobel Edge Detection")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(400.)),
                origin: Point::new(px(1050.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Sobel,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Sobel");

        let state = self.state.read().unwrap().clone();
        let threshold_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.threshold_value),
                state.threshold_value,
                "Threshold".to_string(),
                0.001,
                "threshold".to_string(),
                0.0,
                1.0,
            )
        });

        cx.subscribe(
            &threshold_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "threshold" {
                    this.adjust_threshold(event.value);
                }
            },
        )
        .detach();

        self.threshold_control = Some(threshold_control);

        self.process_messages(window, cx);
    }
}
