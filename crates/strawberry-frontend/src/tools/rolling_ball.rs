use crate::gui::theme;
use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, RollingBallMessage,
    RollingBallState, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct RollingBallView {
    pub state: Arc<RwLock<RollingBallState>>,
    pub ball_radius_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for RollingBallView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl RollingBallView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(RollingBallState::default())),
            ball_radius_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::RollingBall,
            ),
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
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
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::RollingBallMessage(RollingBallMessage::SetBallRadius(
                ball_radius,
            )),
        )) {
            eprintln!("Failed to send ball radius update: {e}");
        }
    }

    fn adjust_ball_radius(&mut self, new_radius: u32) {
        let clamped_radius = new_radius.clamp(1, 200);
        self.update_ball_radius(clamped_radius);
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
            .bg(theme::CONTROL)
            .cursor_pointer().hover(|style| style.bg(theme::CONTROL_HOVER))
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

impl FrontendMessageHandler for RollingBallView {
    type State = RollingBallState;

    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    fn base(&self) -> &crate::tools::ToolBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut crate::tools::ToolBase {
        &mut self.base
    }

    fn extract_state(tool_state: ToolState) -> Option<Self::State> {
        match tool_state {
            ToolState::RollingBallState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &RollingBallState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.ball_radius_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.ball_radius as f32;
                cx.notify();
            });
        }
    }
}

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
                    .bg(theme::WINDOW_BG)
                    .size_full()
                    .text_sm()
                    .p_4()
                    .shadow_lg()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read rolling ball state");
            }
        };

        let is_enabled = state.is_enabled;

        div()
            .track_focus(&self.base.focus_handle.clone().expect("initialized"))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .size_full()
            .text_sm()
            .p_4()
            .shadow_lg()
            .text_color(theme::TEXT)
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

impl_tool_view_basics!(RollingBallView, "Rolling Ball");

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
        self.base.receive_channel = Some(receiver);
        self.base.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self
            .base
            .command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::RollingBall,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | RollingBall");

        cx.bind_keys([KeyBinding::new("cmd-t", RollingBallToggleActivity, None)]);

        // Create and initialize ValueControl component
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
