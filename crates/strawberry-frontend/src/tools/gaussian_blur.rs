use crate::gui::Button;
use crate::gui::actions::actions;
use crate::gui::theme;
use strawberry_types::{CommandSender, ToolId};
// Implements UI for gaussian blur tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{GaussianBlurMessage, GaussianBlurState, PipelineObjectMessage, ToolState};

use strawberry_types::ToolFrontend;

pub struct GaussianBlurView {
    pub state: Arc<RwLock<GaussianBlurState>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for GaussianBlurView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl GaussianBlurView {
    /// Create the gaussian blur tool view and its initial controls.
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(GaussianBlurState::default())),
            sigma_control: None,
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

    /// Send the updated sigma to the backend.
    fn update_sigma(&self, sigma: f32) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::GaussianBlurMessage(
                GaussianBlurMessage::SetSigma(sigma),
            ))
        {
            eprintln!("Failed to send radius/sigma update: {e}");
        }
    }

    /// Handle the keyboard action to toggle activity.
    fn toggle_activity_action(
        &mut self,
        _: &GaussianBlurToggleActivity,
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
    /// Build the status for the gaussian blur view.
    fn render_status(state: &GaussianBlurState, cx: &Context<Self>) -> impl IntoElement + use<> {
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

    // Value control component for radius and sigma
    /// Build the sigma control for the gaussian blur view.
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

    /// Build the preset button for the gaussian blur view.
    fn render_preset_button(
        label: String,
        sigma: f32,
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
                this.update_sigma(sigma);
            },
        ))
    }

    /// Build the presets section for the gaussian blur view.
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
                    .child(Self::render_preset_button("Light".to_string(), 2.0, cx))
                    .child(Self::render_preset_button("Medium".to_string(), 5.0, cx))
                    .child(Self::render_preset_button("Heavy".to_string(), 10.0, cx))
                    .child(Self::render_preset_button("Extreme".to_string(), 15.0, cx)),
            )
    }

    /// Build the controls section for the gaussian blur view.
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
    }
}

actions!([GaussianBlurToggleActivity]);

impl FrontendMessageHandler for GaussianBlurView {
    type State = GaussianBlurState;

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
            ToolState::GaussianBlurState(s) => Some(s),
            _ => None,
        }
    }

    /// Synchronize local controls with the latest backend settings.
    fn on_state_changed(&mut self, new_state: &GaussianBlurState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.sigma_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.sigma;
                cx.notify();
            });
        }
    }
}

impl Render for GaussianBlurView {
    /// Build the gaussian blur view and its interaction handlers.
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
                    .child("Error: Unable to read gaussian blur state");
            }
        };

        div()
            .key_context("GaussianBlurView")
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

impl ToolFrontend for GaussianBlurView {
    /// Connect the view’s subscriptions and event handlers.
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            GaussianBlurToggleActivity,
            Some("GaussianBlurView"),
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

        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this
                        .state
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    drop(current_state);
                    this.update_sigma(event.value);
                }
            },
        )
        .detach();

        self.sigma_control = Some(sigma_control);

        cx.notify();

        println!("Initialized");

        self.process_messages(window, cx);
    }
}
