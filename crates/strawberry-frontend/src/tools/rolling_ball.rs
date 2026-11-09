use crate::gui::Button;
use crate::gui::actions::actions;
use num_traits::ToPrimitive;

use crate::gui::theme;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    PipelineCommand, PipelineObjectMessage, RollingBallMessage, RollingBallState, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct RollingBallView {
    pub state: Arc<RwLock<RollingBallState>>,
    pub ball_radius_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for RollingBallView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl RollingBallView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(RollingBallState::default())),
            ball_radius_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::RollingBall,
                cx.focus_handle(),
            ),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;

        self.toggle_enabled(current_enabled, window, cx);
    }

    fn update_ball_radius(&self, ball_radius: u32) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::RollingBallMessage(RollingBallMessage::SetBallRadius(
                ball_radius,
            )),
        )) {
            eprintln!("Failed to send ball radius update: {e}");
        }
    }

    fn adjust_ball_radius(&self, new_radius: u32) {
        let clamped_radius = new_radius.clamp(1, 200);
        self.update_ball_radius(clamped_radius);
    }

    fn render_status(state: &RollingBallState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    fn render_ball_radius_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.ball_radius_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    fn render_preset_button(
        label: String,
        ball_radius: u32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        Button::new(gpui::ElementId::Name(
            format!("render_preset_button-{label}").into(),
        ))
        .large()
        .label(label)
        .on_click(cx.listener(
            move |this: &mut Self,
                  _event: &gpui::ClickEvent,
                  _window: &mut Window,
                  _cx: &mut Context<Self>| {
                this.update_ball_radius(ball_radius);
            },
        ))
    }

    fn render_presets_section(cx: &Context<Self>) -> impl IntoElement + use<> {
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
                    .child(Self::render_preset_button(
                        "Small (10px)".to_string(),
                        10,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Medium (25px)".to_string(),
                        25,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Large (50px)".to_string(),
                        50,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Extra Large (100px)".to_string(),
                        100,
                        cx,
                    )),
            )
    }

    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_ball_radius_control(cx, window))
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
                // This approximation is only used to display the integer backend state.
                let Some(display_value) = new_state.ball_radius.to_f32() else {
                    eprintln!("Cannot display ball_radius in the numeric control");
                    return;
                };
                c.value = display_value;
                cx.notify();
            });
        }
    }
}

impl Render for RollingBallView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for rendering: {e}");
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(theme::WINDOW_BG)
                    .w_full()
                    .text_sm()
                    .p_4()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read rolling ball state");
            }
        };

        div()
            .key_context("RollingBallView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .w_full()
            .text_sm()
            .p_4()
            .text_color(theme::TEXT)
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(Self::render_status(&state, cx))
            .child(self.render_controls_section(cx, window))
            .child(Self::render_presets_section(cx))
    }
}

impl ToolFrontend for RollingBallView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            RollingBallToggleActivity,
            Some("RollingBallView"),
        )]);

        // Create and initialize ValueControl component
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(display_value) = state.ball_radius.to_f32() else {
            eprintln!("Cannot display ball_radius in the numeric control");
            return;
        };
        let ball_radius_control = cx.new(|cx| {
            ValueControl::new(
                format!("{}", state.ball_radius),
                display_value,
                ValueControlConfig {
                    label: "Ball Radius".to_string(),
                    step: 1.0,
                    id: "ball_radius".to_string(),
                    min: 1.0,
                    max: 200.0,
                },
                cx,
            )
        });

        cx.subscribe(
            &ball_radius_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "ball_radius"
                    && event.value.is_finite()
                    && event.value >= 0.0
                    && let Some(value) = event.value.to_u32()
                {
                    this.adjust_ball_radius(value);
                }
            },
        )
        .detach();

        self.ball_radius_control = Some(ball_radius_control);

        self.process_messages(window, cx);
    }
}
