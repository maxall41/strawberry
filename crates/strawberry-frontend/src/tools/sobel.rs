use crate::gui::Button;
use crate::gui::actions::actions;
use crate::gui::theme;
use std::sync::{Arc, RwLock};
use strawberry_types::{CommandSender, ToolId};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{PipelineObjectMessage, SobelMessage, SobelState, ToolState};

use strawberry_types::ToolFrontend;

pub struct SobelView {
    pub state: Arc<RwLock<SobelState>>,
    pub threshold_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for SobelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl SobelView {
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(SobelState::default())),
            threshold_control: None,
            base: crate::tools::base::ToolBase::new(command_sender, id, cx.focus_handle()),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.set_enabled(!current_value) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    fn toggle_activity_action(
        &mut self,
        _: &SobelToggleActivity,
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

    fn toggle_threshold_enabled(&self, current_value: bool) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::SobelMessage(
                SobelMessage::ToggleThreshold(!current_value),
            ))
        {
            eprintln!("Failed to send toggle threshold command: {e}");
        }
    }

    fn update_threshold_value(&self, threshold_value: f32) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::SobelMessage(
                SobelMessage::SetThreshold(threshold_value),
            ))
        {
            eprintln!("Failed to send threshold value update: {e}");
        }
    }

    fn adjust_threshold(&self, new_threshold: f32) {
        let clamped_threshold = new_threshold.clamp(0.0, 1.0);
        self.update_threshold_value(clamped_threshold);
    }

    fn render_status(state: &SobelState, cx: &Context<Self>) -> impl IntoElement + use<> {
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

    fn render_threshold_toggle(state: &SobelState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let threshold_enabled = state.use_threshold;
        ToggleButton::new(
            "Use Threshold".to_string(),
            threshold_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  _window: &mut Window,
                  _cx: &mut Context<Self>| {
                this.toggle_threshold_enabled(threshold_enabled);
            },
        )
        .render(cx)
    }

    fn render_threshold_control(
        &self,
        state: &SobelState,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        if state.use_threshold {
            div().child(self.threshold_control.clone().map_or_else(
                || div().into_any_element(),
                gpui::IntoElement::into_any_element,
            ))
        } else {
            div()
        }
    }

    fn render_preset_button(
        label: String,
        threshold_value: f32,
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
                this.update_threshold_value(threshold_value);
            },
        ))
    }

    fn render_presets_section(state: &SobelState, cx: &Context<Self>) -> impl IntoElement + use<> {
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
                        .child(Self::render_preset_button("Low (0.1)".to_string(), 0.1, cx))
                        .child(Self::render_preset_button(
                            "Medium (0.3)".to_string(),
                            0.3,
                            cx,
                        ))
                        .child(Self::render_preset_button(
                            "High (0.5)".to_string(),
                            0.5,
                            cx,
                        ))
                        .child(Self::render_preset_button(
                            "Very High (0.7)".to_string(),
                            0.7,
                            cx,
                        )),
                )
        } else {
            div()
        }
    }

    fn render_controls_section(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::render_threshold_toggle(state, cx))
            .child(self.render_threshold_control(state, cx, window))
    }
}

actions!([SobelToggleActivity]);

impl FrontendMessageHandler for SobelView {
    type State = SobelState;

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
            ToolState::SobelState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &SobelState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.threshold_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.threshold_value;
                cx.notify();
            });
        }
    }
}

impl Render for SobelView {
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
                    .child("Error: Unable to read sobel state");
            }
        };

        div()
            .key_context("SobelView")
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
                        this.base.select();
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(Self::render_status(&state, cx))
            .child(self.render_controls_section(&state, cx, window))
            .child(Self::render_presets_section(&state, cx))
    }
}

impl ToolFrontend for SobelView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            SobelToggleActivity,
            Some("SobelView"),
        )]);

        // Create and initialize ValueControl component
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let threshold_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.threshold_value),
                state.threshold_value,
                ValueControlConfig {
                    label: "Threshold".to_string(),
                    step: 0.001,
                    id: "threshold".to_string(),
                    min: 0.0,
                    max: 1.0,
                },
                cx,
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
