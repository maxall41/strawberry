use crate::gui::actions::{ToggleChannel, actions, create_channel_shortcuts};
use crate::gui::theme;
use strawberry_types::{CommandSender, ToolId};
// Implements UI for compositor tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div, prelude::*,
};

use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{CompositorMessage, CompositorState, PipelineObjectMessage, ToolState};

use strawberry_types::ToolFrontend;

pub struct CompositorView {
    pub state: Arc<RwLock<CompositorState>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for CompositorView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl CompositorView {
    /// Create the compositor tool view and its initial controls.
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(CompositorState::default())),
            base: crate::tools::base::ToolBase::new(command_sender, id, cx.focus_handle()),
        }
    }

    /// Start receiving backend state updates for this tool view.
    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    /// Send a channel visibility change to the backend.
    fn toggle_channel(
        &self,
        current_value: bool,
        channel_index: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Ok(channel_index) = u16::try_from(channel_index) else {
            eprintln!("Channel index exceeds the backend command range");
            return;
        };
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::CompositorMessage(
                CompositorMessage::ToggleVisibility(!current_value, channel_index),
            ))
        {
            eprintln!("Failed to send toggle channel command: {e}");
        }
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
        _: &CompositorToggleActivity,
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

    /// Handle the keyboard action toggling a channel by its position, ignoring channels the
    /// current image lacks.
    fn toggle_channel_action(
        &mut self,
        action: &ToggleChannel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if action.index < state.channel_count
            && let Some(&current_value) = state.enabled_channels.get(action.index)
        {
            drop(state);
            self.toggle_channel(current_value, action.index, window, cx);
        }
    }

    // Status component showing enabled/disabled state
    /// Build the status for the compositor view.
    fn render_status(state: &CompositorState, cx: &Context<Self>) -> impl IntoElement + use<> {
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

    // Individual channel item component
    /// Build the channel item for the compositor view.
    fn render_channel_item(
        index: usize,
        is_enabled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        ToggleButton::new(
            format!("Channel {}", index.saturating_add(1)),
            is_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.toggle_channel(is_enabled, index, window, cx);
            },
        )
        .with_text("On".to_string(), "Off".to_string())
        .with_keyboard_shortcut(&format!("⌘{}", index.saturating_add(1)))
        .with_no_margin()
        .render(cx)
    }

    // Channels section component
    /// Build the channels section for the compositor view.
    fn render_channels_section(
        state: &CompositorState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(gpui::FontWeight::BOLD).child("Channels:"))
            .child(
                div().flex().flex_col().gap_1().children(
                    state
                        .enabled_channels
                        .iter()
                        .enumerate()
                        .filter_map(|(index, &is_enabled)| {
                            if index >= state.channel_count {
                                None
                            } else {
                                Some(Self::render_channel_item(index, is_enabled, cx))
                            }
                        }),
                ),
            )
    }
}

actions!([CompositorToggleActivity,]);

impl FrontendMessageHandler for CompositorView {
    type State = CompositorState;

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
            ToolState::CompositorState(s) => Some(s),
            _ => None,
        }
    }
}

impl Render for CompositorView {
    /// Build the compositor view and its interaction handlers.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child("Error: Unable to read compositor state");
            }
        };

        div()
            .key_context("CompositorView")
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
            .on_action(cx.listener(Self::toggle_channel_action))
            .child(Self::render_status(&state, cx))
            .child(Self::render_channels_section(&state, cx))
    }
}

impl ToolFrontend for CompositorView {
    /// Connect the view’s subscriptions and event handlers.
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            CompositorToggleActivity,
            Some("CompositorView"),
        )]);
        cx.bind_keys(create_channel_shortcuts(9, "CompositorView"));

        self.process_messages(window, cx);
    }
}
