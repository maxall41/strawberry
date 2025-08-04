use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::pipeline::{
    rolling_ball::RollingBallState, BackendToolType, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, RollingBallMessage, ToolState,
};

use super::ToolFrontend;

pub struct RollingBallView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<RollingBallState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub ball_radius_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for RollingBallView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl RollingBallView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(RollingBallState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            ball_radius_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::RollingBall,
            ),
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::RollingBall,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | RollingBall");

        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let ball_radius_control = cx.new(|cx| {
            ValueControl::new(
                format!("{}", state.ball_radius),
                state.ball_radius as f32,
                "Ball Radius".to_string(),
                1.0,
                "ball_radius".to_string(),
                1.0,
                200.0,
            )
        });

        cx.subscribe(
            &ball_radius_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "ball_radius" {
                    this.adjust_ball_radius(event.value as u32);
                }
            },
        )
        .detach();

        self.ball_radius_control = Some(ball_radius_control);

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
                if let PipelineCommand::FrontendMessage(message) = command { match message {
                    FrontendMessage::ReturnState(tool_state) => if let ToolState::RollingBallState(new_state) = tool_state {
                        if let Ok(mut locked_state) = state.write() {
                            if *locked_state != new_state {
                                let ball_radius_ref = new_state.ball_radius;
                                *locked_state = new_state;
                                if let Err(e) = this.update(cx, |this, cx| {
                                    this.ball_radius_control
                                        .as_ref()
                                        .expect("ball_radius_control should be initialized before use")
                                        .update(cx, |control, cx| {
                                            control.value = ball_radius_ref as f32;
                                            cx.notify();
                                        });
                                    cx.notify();
                                }) {
                                    eprintln!(
                                        "Failed to update UI after state change: {e}"
                                    );
                                }
                            }
                        } else {
                            eprintln!("Failed to acquire write lock on rolling ball state");
                        }
                    },
                    FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                        println!("Received backend tools | Frontend RollingBall");
                        if let Err(e) = this.update(cx, |this, cx| {
                            if let Ok(mut current_tool_index) = this.current_tool_index.write()
                            {
                                *current_tool_index = backend_ids.first().copied();
                            } else {
                                eprintln!("Failed to acquire write lock on current tool index");
                                return;
                            }
                            this.backend_tool_indicies = backend_ids;
                            this.send_channels = senders;

                            if let Some(sender) = this.send_channels.first() {
                                if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                    eprintln!("Failed to request initial state: {e}");
                                }
                            }
                        }) {
                            eprintln!("Failed to update backend tools: {e}");
                        }
                    }
                    _ => {}
                } }
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
                eprintln!("Failed to read current tool index: {e}");
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
                eprintln!("Failed to send toggle enabled command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn toggle_activity_action(
        &mut self,
        _: &RollingBallToggleActivity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_enabled = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .is_enabled;

        self.toggle_enabled(current_enabled, window, cx);
    }

    fn update_ball_radius(&mut self, ball_radius: u32) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::RollingBallMessage(RollingBallMessage::SetBallRadius(
                    ball_radius,
                )),
            )) {
                eprintln!("Failed to send ball radius update: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn adjust_ball_radius(&mut self, new_radius: u32) {
        let clamped_radius = new_radius.clamp(1, 200);
        self.update_ball_radius(clamped_radius);
    }

    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Rolling Ball")
    }

    fn render_status(&self, state: &RollingBallState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut RollingBallView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<RollingBallView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    fn render_ball_radius_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.ball_radius_control
            .clone()
            .expect("ball_radius_control should be initialized before use")
    }

    fn render_preset_button(
        &self,
        label: String,
        ball_radius: u32,
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
                    move |this: &mut RollingBallView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<RollingBallView>| {
                        this.update_ball_radius(ball_radius);
                    },
                ),
            )
    }

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
                    .child(self.render_preset_button("Small (10px)".to_string(), 10, cx))
                    .child(self.render_preset_button("Medium (25px)".to_string(), 25, cx))
                    .child(self.render_preset_button("Large (50px)".to_string(), 50, cx))
                    .child(self.render_preset_button("Extra Large (100px)".to_string(), 100, cx)),
            )
    }

    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_ball_radius_control(cx, _window))
    }
}

actions!([RollingBallToggleActivity]);

impl Render for RollingBallView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for rendering: {e}");
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(rgb(0x505050))
                    .size_full()
                    .p_4()
                    .shadow_lg()
                    .text_color(rgb(0xffffff))
                    .child("Error: Unable to read rolling ball state");
            }
        };

        let is_enabled = state.is_enabled;

        div()
            .track_focus(&self.focus_handle(cx))
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
                    move |this: &mut RollingBallView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<RollingBallView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_controls_section(cx, _window))
            .child(self.render_presets_section(cx))
    }
}

impl ToolFrontend for RollingBallView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Rolling Ball")),
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
            BackendToolType::RollingBall,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | RollingBall");

        cx.bind_keys([KeyBinding::new("cmd-t", RollingBallToggleActivity, None)]);

        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let ball_radius_control = cx.new(|cx| {
            ValueControl::new(
                format!("{}", state.ball_radius),
                state.ball_radius as f32,
                "Ball Radius".to_string(),
                1.0,
                "ball_radius".to_string(),
                1.0,
                200.0,
            )
        });

        cx.subscribe(
            &ball_radius_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "ball_radius" {
                    this.adjust_ball_radius(event.value as u32);
                }
            },
        )
        .detach();

        self.ball_radius_control = Some(ball_radius_control);

        self.process_messages(window, cx);
    }
}
