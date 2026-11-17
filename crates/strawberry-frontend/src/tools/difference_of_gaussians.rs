use std::sync::{Arc, RwLock};
use strawberry_types::{CommandSender, ToolId};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};
use strawberry_types::{
    DifferenceOfGaussiansMessage, DifferenceOfGaussiansState, PipelineObjectMessage, ToolFrontend,
    ToolState,
};

use crate::{
    gui::{
        ToggleButton,
        actions::actions,
        theme,
        value_control::{ValueControl, ValueControlConfig, ValueControlEvent},
    },
    tools::{FrontendMessageHandler, ToolBase, spawn_message_processor},
};

pub struct DifferenceOfGaussiansView {
    state: Arc<RwLock<DifferenceOfGaussiansState>>,
    base: ToolBase,
    small_sigma_control: Option<Entity<ValueControl>>,
    large_sigma_control: Option<Entity<ValueControl>>,
}

actions!([DifferenceOfGaussiansToggleActivity]);

impl Focusable for DifferenceOfGaussiansView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl DifferenceOfGaussiansView {
    /// Create the difference of gaussians tool view and its initial controls.
    pub fn new(id: ToolId, sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(DifferenceOfGaussiansState::default())),
            base: ToolBase::new(sender, id, cx.focus_handle()),
            small_sigma_control: None,
            large_sigma_control: None,
        }
    }

    /// Send a settings message to this tool’s backend instance.
    fn send(&self, message: PipelineObjectMessage) {
        if let Err(error) = self.base.send_to_backend(message) {
            eprintln!("Failed to update Difference of Gaussians: {error}");
        }
    }

    /// Handle the keyboard action to toggle activity.
    fn toggle_activity_action(
        &mut self,
        _: &DifferenceOfGaussiansToggleActivity,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let enabled = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;
        let _ = self.base.set_enabled(!enabled);
    }
}

impl FrontendMessageHandler for DifferenceOfGaussiansView {
    type State = DifferenceOfGaussiansState;

    /// Return the shared snapshot of this tool’s backend state.
    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    /// Return the shared tool connection and instance metadata.
    fn base(&self) -> &ToolBase {
        &self.base
    }

    /// Return mutable access to the tool connection and subscription state.
    fn base_mut(&mut self) -> &mut ToolBase {
        &mut self.base
    }

    /// Extract this tool’s settings from a backend state update.
    fn extract_state(state: ToolState) -> Option<Self::State> {
        match state {
            ToolState::DifferenceOfGaussiansState(state) => Some(state),
            _ => None,
        }
    }

    /// Synchronize local controls with the latest backend settings.
    fn on_state_changed(&mut self, state: &Self::State, cx: &mut Context<Self>) {
        if let Some(control) = &self.small_sigma_control {
            control.update(cx, |control, cx| {
                control.config.max = state.large_sigma.next_down();
                control.set_value(state.small_sigma);
                cx.notify();
            });
        }
        if let Some(control) = &self.large_sigma_control {
            control.update(cx, |control, cx| {
                control.config.min = state.small_sigma.next_up();
                control.set_value(state.large_sigma);
                cx.notify();
            });
        }
    }
}

impl Render for DifferenceOfGaussiansView {
    /// Build the difference of gaussians view and its interaction handlers.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;
        div()
            .key_context("DifferenceOfGaussiansView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .w_full()
            .text_sm()
            .p_4()
            .text_color(theme::TEXT)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.base.select();
                    this.focus_handle(cx).focus(window, cx);
                }),
            )
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(
                ToggleButton::new(
                    "Enabled".to_string(),
                    enabled,
                    move |this: &mut Self, _, _, _| {
                        let _ = this.base.set_enabled(!enabled);
                    },
                )
                .with_keyboard_shortcut("⌘T")
                .render(cx),
            )
            .children(self.small_sigma_control.clone())
            .children(self.large_sigma_control.clone())
    }
}

impl ToolFrontend for DifferenceOfGaussiansView {
    /// Connect the view’s subscriptions and event handlers.
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }
        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            DifferenceOfGaussiansToggleActivity,
            Some("DifferenceOfGaussiansView"),
        )]);
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let small = cx.new(|cx| {
            ValueControl::new(
                format!("{:.2}", state.small_sigma),
                state.small_sigma,
                ValueControlConfig {
                    label: "Small sigma (px)".to_string(),
                    id: "dog-small-sigma".to_string(),
                    step: 0.1,
                    min: 0.1,
                    max: state.large_sigma.next_down(),
                },
                cx,
            )
        });
        let large = cx.new(|cx| {
            ValueControl::new(
                format!("{:.2}", state.large_sigma),
                state.large_sigma,
                ValueControlConfig {
                    label: "Large sigma (px)".to_string(),
                    id: "dog-large-sigma".to_string(),
                    step: 0.1,
                    min: state.small_sigma.next_up(),
                    max: 64.0,
                },
                cx,
            )
        });
        cx.subscribe(&small, |this, _, event: &ValueControlEvent, _| {
            this.send(PipelineObjectMessage::DifferenceOfGaussiansMessage(
                DifferenceOfGaussiansMessage::SetSmallSigma(event.value),
            ));
        })
        .detach();
        cx.subscribe(&large, |this, _, event: &ValueControlEvent, _| {
            this.send(PipelineObjectMessage::DifferenceOfGaussiansMessage(
                DifferenceOfGaussiansMessage::SetLargeSigma(event.value),
            ));
        })
        .detach();
        self.small_sigma_control = Some(small);
        self.large_sigma_control = Some(large);
        spawn_message_processor(self, window, cx);
        cx.notify();
    }
}
