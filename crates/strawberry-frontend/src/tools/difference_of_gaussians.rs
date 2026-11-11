use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};
use strawberry_types::{
    BackendToolType, DifferenceOfGaussiansMessage, DifferenceOfGaussiansState, PipelineCommand,
    PipelineObjectMessage, ToolFrontend, ToolState,
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
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl DifferenceOfGaussiansView {
    pub fn new(
        _receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(DifferenceOfGaussiansState::default())),
            base: ToolBase::new(
                sender,
                BackendToolType::DifferenceOfGaussians,
                cx.focus_handle(),
            ),
            small_sigma_control: None,
            large_sigma_control: None,
        }
    }

    fn send(&self, message: PipelineObjectMessage) {
        if let Err(error) = self
            .base
            .send_to_backend(PipelineCommand::PipelineObject(message))
        {
            eprintln!("Failed to update Difference of Gaussians: {error}");
        }
    }

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
        self.send(PipelineObjectMessage::ToggleActive(!enabled));
    }
}

impl FrontendMessageHandler for DifferenceOfGaussiansView {
    type State = DifferenceOfGaussiansState;

    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    fn base(&self) -> &ToolBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ToolBase {
        &mut self.base
    }

    fn extract_state(state: ToolState) -> Option<Self::State> {
        match state {
            ToolState::DifferenceOfGaussiansState(state) => Some(state),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, state: &Self::State, cx: &mut Context<Self>) {
        if let Some(control) = &self.small_sigma_control {
            control.update(cx, |control, cx| {
                control.config.max = (state.large_sigma - 0.1).max(0.1);
                control.set_value(state.small_sigma);
                cx.notify();
            });
        }
        if let Some(control) = &self.large_sigma_control {
            control.update(cx, |control, cx| {
                control.config.min = (state.small_sigma + 0.1).min(64.0);
                control.set_value(state.large_sigma);
                cx.notify();
            });
        }
    }
}

impl Render for DifferenceOfGaussiansView {
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
                    this.focus_handle(cx).focus(window, cx);
                }),
            )
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(
                ToggleButton::new(
                    "Enabled".to_string(),
                    enabled,
                    move |this: &mut Self, _, _, _| {
                        this.send(PipelineObjectMessage::ToggleActive(!enabled));
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
                    max: state.large_sigma - 0.1,
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
                    min: state.small_sigma + 0.1,
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
