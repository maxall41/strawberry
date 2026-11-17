use crate::gui::actions::actions;
use crate::gui::theme;
use std::sync::{Arc, RwLock};
use strawberry_types::{CommandSender, ToolId};

use gpui::{
    App, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div, prelude::*,
};

use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, ToolBase, spawn_message_processor};
use strawberry_types::{GrayscaleState, ToolState};

use strawberry_types::ToolFrontend;

pub struct GrayscaleView {
    pub state: Arc<RwLock<GrayscaleState>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for GrayscaleView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl FrontendMessageHandler for GrayscaleView {
    type State = GrayscaleState;

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
    fn extract_state(ts: ToolState) -> Option<Self::State> {
        match ts {
            ToolState::GrayscaleState(s) => Some(s),
            _ => None,
        }
    }
}

impl GrayscaleView {
    /// Create the grayscale tool view and its initial controls.
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(GrayscaleState::default())),
            base: crate::tools::base::ToolBase::new(command_sender, id, cx.focus_handle()),
        }
    }

    /// Start receiving backend state updates for this tool view.
    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    /// Send the opposite enabled state to the backend.
    fn toggle_enabled(&self, current_value: bool, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.set_enabled(!current_value) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    /// Handle the keyboard action to toggle activity.
    fn toggle_activity_action(
        &mut self,
        _: &GrayscaleToggleActivity,
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

    /// Build the status for the grayscale view.
    fn render_status(state: &GrayscaleState, cx: &Context<Self>) -> impl IntoElement + use<> {
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
}

actions!([GrayscaleToggleActivity]);

// Use macro to implement common tool methods

impl Render for GrayscaleView {
    /// Build the grayscale view and its interaction handlers.
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
                    .w_full()
                    .text_sm()
                    .p_4()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read grayscale state");
            }
        };

        div()
            .key_context("GrayscaleView")
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
    }
}

impl ToolFrontend for GrayscaleView {
    /// Connect the view’s subscriptions and event handlers.
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            GrayscaleToggleActivity,
            Some("GrayscaleView"),
        )]);

        self.process_messages(window, cx);
    }
}
