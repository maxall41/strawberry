use crate::gui::Button;
use crate::gui::actions::actions;
use crate::gui::theme;
use strawberry_types::{CommandSender, ToolId};
// Implements UI for contrast tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{ContrastMessage, ContrastState, PipelineObjectMessage, ToolState};

use strawberry_types::ToolFrontend;

pub struct ContrastView {
    pub state: Arc<RwLock<ContrastState>>,
    pub min_control: Option<Entity<ValueControl>>,
    pub max_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for ContrastView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl ContrastView {
    /// Create the contrast tool view and its initial controls.
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(ContrastState::default())),
            min_control: None,
            max_control: None,
            base: crate::tools::base::ToolBase::new(command_sender, id, cx.focus_handle()),
        }
    }

    /// Start receiving backend state updates for this tool view.
    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    /// Send the opposite enabled state to the backend.
    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.set_enabled(!current_value) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    /// Send the updated min max to the backend.
    fn update_min_max(&self, min: f32, max: f32) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::ContrastMessage(
                ContrastMessage::SetMinMax(min, max),
            ))
        {
            eprintln!("Failed to send min/max update command: {e}");
        }
    }

    /// Apply the new min value while retaining the other settings.
    fn adjust_min_value(&self, new_min: f32) {
        let current_state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let max_val = current_state.max;
        drop(current_state);

        self.update_min_max(new_min, max_val);
    }

    /// Apply the new max value while retaining the other settings.
    fn adjust_max_value(&self, new_max: f32) {
        let current_state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let min_val = current_state.min;
        drop(current_state);

        self.update_min_max(min_val, new_max);
    }

    // Status component showing enabled/disabled state
    /// Build the status for the contrast view.
    fn render_status(state: &ContrastState, cx: &Context<Self>) -> impl IntoElement + use<> {
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

    // Min value control component
    /// Build the min control for the contrast view.
    fn render_min_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.min_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    // Max value control component
    /// Build the max control for the contrast view.
    fn render_max_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.max_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    // Preset button component
    /// Build the preset button for the contrast view.
    fn render_preset_button(
        label: String,
        min: f32,
        max: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        Button::new(gpui::ElementId::Name(
            format!("render_preset_button-{label}").into(),
        ))
        .label(label)
        .on_click(cx.listener(
            move |this: &mut Self,
                  _event: &gpui::ClickEvent,
                  _window: &mut Window,
                  _cx: &mut Context<Self>| {
                this.update_min_max(min, max);
            },
        ))
    }

    // Presets section component
    /// Build the presets section for the contrast view.
    fn render_presets_section(cx: &Context<Self>) -> impl IntoElement + use<> {
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
                    .child(Self::render_preset_button(
                        "Reset (0.0 - 1.0)".to_string(),
                        0.0,
                        1.0,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "High Contrast (0.2 - 0.8)".to_string(),
                        0.2,
                        0.8,
                        cx,
                    )),
            )
    }

    /// Handle the keyboard action to toggle activity.
    fn toggle_activity_action(
        &mut self,
        _: &ContrastToggleActivity,
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

    // Controls section component
    /// Build the controls section for the contrast view.
    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_min_control(cx, window))
            .child(self.render_max_control(cx, window))
    }
}

impl FrontendMessageHandler for ContrastView {
    type State = ContrastState;

    /// Return the shared snapshot of this tool’s backend state.
    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    /// Return the shared tool connection and instance metadata.
    fn base(&self) -> &crate::tools::ToolBase {
        &self.base
    }

    /// Return mutable access to the tool connection and subscription state.
    fn base_mut(&mut self) -> &mut crate::tools::ToolBase {
        &mut self.base
    }

    /// Extract this tool’s settings from a backend state update.
    fn extract_state(tool_state: ToolState) -> Option<Self::State> {
        match tool_state {
            ToolState::ContrastState(s) => Some(s),
            _ => None,
        }
    }

    /// Synchronize local controls with the latest backend settings.
    fn on_state_changed(&mut self, new_state: &ContrastState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.min_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.min;
                cx.notify();
            });
        }
        if let Some(ref ctrl) = self.max_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.max;
                cx.notify();
            });
        }
    }
}

actions!([ContrastToggleActivity]);

impl Render for ContrastView {
    /// Build the contrast view and its interaction handlers.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
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
                    .child("Error: Unable to read contrast state");
            }
        };

        div()
            .key_context("ContrastView")
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
            .child(self.render_controls_section(cx, window))
            .child(Self::render_presets_section(cx))
    }
}

impl ToolFrontend for ContrastView {
    /// Connect the view’s subscriptions and event handlers.
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            ContrastToggleActivity,
            Some("ContrastView"),
        )]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let min_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.min),
                state.min,
                ValueControlConfig {
                    label: "Min".to_string(),
                    step: 0.01,
                    id: "min".to_string(),
                    min: 0.0,
                    max: 1.0,
                },
                cx,
            )
        });

        let max_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.max),
                state.max,
                ValueControlConfig {
                    label: "Max".to_string(),
                    step: 0.01,
                    id: "max".to_string(),
                    min: 0.0,
                    max: 1.0,
                },
                cx,
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
