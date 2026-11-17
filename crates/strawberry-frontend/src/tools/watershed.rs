use crate::gui::Button;
use crate::gui::actions::actions;
use num_traits::ToPrimitive;
use strawberry_types::{CommandSender, ToolId};

use crate::gui::theme;
// Implements UI for watershed tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{PipelineObjectMessage, ToolState, WatershedMessage, WatershedState};

use strawberry_types::ToolFrontend;

pub struct WatershedView {
    pub state: Arc<RwLock<WatershedState>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    pub min_distance_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for WatershedView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl WatershedView {
    /// Create the watershed tool view and its initial controls.
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(WatershedState::default())),
            sigma_control: None,
            min_distance_control: None,
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

    /// Send the updated parameters to the backend.
    fn update_parameters(&self, sigma: f32, min_distance: u32) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::WatershedMessage(
                WatershedMessage::SetParameters(sigma, min_distance),
            ))
        {
            eprintln!("Failed to send watershed parameters update: {e}");
        }
    }

    /// Apply the new sigma while retaining the other settings.
    fn adjust_sigma(&self, new_sigma: f32, current_min_distance: u32) {
        self.update_parameters(new_sigma, current_min_distance);
    }

    /// Apply the new min distance while retaining the other settings.
    fn adjust_min_distance(&self, current_sigma: f32, new_min_distance: u32) {
        self.update_parameters(current_sigma, new_min_distance);
    }

    /// Handle the keyboard action to toggle activity.
    fn toggle_activity_action(
        &mut self,
        _: &WatershedToggleActivity,
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

    // Status component showing enabled/disabled state
    /// Build the status for the watershed view.
    fn render_status(state: &WatershedState, cx: &Context<Self>) -> impl IntoElement + use<> {
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

    // Sigma control component
    /// Build the sigma control for the watershed view.
    fn render_sigma_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.sigma_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    // Min Distance control component
    /// Build the min distance control for the watershed view.
    fn render_min_distance_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.min_distance_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    // Preset button component
    /// Build the preset button for the watershed view.
    fn render_preset_button(
        label: String,
        sigma: f32,
        min_distance: u32,
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
                this.update_parameters(sigma, min_distance);
            },
        ))
    }

    // Presets section component
    /// Build the presets section for the watershed view.
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
                    .child(Self::render_preset_button("Fine".to_string(), 0.5, 5, cx))
                    .child(Self::render_preset_button(
                        "Normal".to_string(),
                        1.0,
                        10,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Coarse".to_string(),
                        2.0,
                        20,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Very Coarse".to_string(),
                        3.0,
                        30,
                        cx,
                    )),
            )
    }

    // Controls section component
    /// Build the controls section for the watershed view.
    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_sigma_control(cx, window))
            .child(self.render_min_distance_control(cx, window))
    }
}

actions!([WatershedToggleActivity]);

impl FrontendMessageHandler for WatershedView {
    type State = WatershedState;

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
            ToolState::WatershedState(s) => Some(s),
            _ => None,
        }
    }

    /// Synchronize local controls with the latest backend settings.
    fn on_state_changed(&mut self, new_state: &WatershedState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.sigma_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.sigma;
                cx.notify();
            });
        }
        if let Some(ref ctrl) = self.min_distance_control {
            ctrl.update(cx, |c, cx| {
                // This approximation is only used to display the integer backend state.
                let Some(display_value) = new_state.min_distance.to_f32() else {
                    eprintln!("Cannot display min_distance in the numeric control");
                    return;
                };
                c.value = display_value;
                cx.notify();
            });
        }
    }
}

impl Render for WatershedView {
    /// Build the watershed view and its interaction handlers.
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
                    .child("Error: Unable to read watershed state");
            }
        };

        div()
            .key_context("WatershedView")
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

impl ToolFrontend for WatershedView {
    /// Connect the view’s subscriptions and event handlers.
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            WatershedToggleActivity,
            Some("WatershedView"),
        )]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let sigma_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.2}", state.sigma),
                state.sigma,
                ValueControlConfig {
                    label: "Sigma".to_string(),
                    step: 0.1,
                    id: "sigma".to_string(),
                    min: 0.1,
                    max: 10.0,
                },
                cx,
            )
        });

        let Some(display_value) = state.min_distance.to_f32() else {
            eprintln!("Cannot display min_distance in the numeric control");
            return;
        };
        let min_distance_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.0}", state.min_distance),
                display_value,
                ValueControlConfig {
                    label: "Min Distance".to_string(),
                    step: 1.0,
                    id: "min_distance".to_string(),
                    min: 1.0,
                    max: 100.0,
                },
                cx,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this
                        .state
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let min_dist = current_state.min_distance;
                    drop(current_state);
                    this.adjust_sigma(event.value, min_dist);
                }
            },
        )
        .detach();

        cx.subscribe(
            &min_distance_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min_distance"
                    && event.value.is_finite()
                    && event.value >= 0.0
                    && let Some(value) = event.value.to_u32()
                {
                    let current_state = this
                        .state
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let sigma = current_state.sigma;
                    drop(current_state);
                    this.adjust_min_distance(sigma, value);
                }
            },
        )
        .detach();

        self.sigma_control = Some(sigma_control);
        self.min_distance_control = Some(min_distance_control);

        self.process_messages(window, cx);
    }
}
