use crate::gui::theme;
// Implements UI for Decomposer tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Point, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};

use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    DecomposerMessage, DecomposerState, PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct DecomposerView {
    pub state: Arc<RwLock<DecomposerState>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for DecomposerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl DecomposerView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(DecomposerState::default())),
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Decomposer,
            ),
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_layer(
        &mut self,
        current_value: bool,
        layer_index: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::DecomposerMessage(DecomposerMessage::ToggleVisibility(
                !current_value,
                layer_index as u16,
            )),
        )) {
            eprintln!("Failed to send toggle layer command: {e}");
        }
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
        _: &DecomposerToggleActivity,
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

    fn toggle_layer_by_index(
        &mut self,
        layer_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        if layer_index < state.enabled_layers.len() {
            let current_value = state.enabled_layers[layer_index];
            drop(state);
            self.toggle_layer(current_value, layer_index, window, cx);
        }
    }

    fn toggle_layer_1_action(
        &mut self,
        _: &DecomposerToggleLayer1,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(0, window, cx);
    }

    fn toggle_layer_2_action(
        &mut self,
        _: &DecomposerToggleLayer2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(1, window, cx);
    }

    fn toggle_layer_3_action(
        &mut self,
        _: &DecomposerToggleLayer3,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(2, window, cx);
    }

    fn toggle_layer_4_action(
        &mut self,
        _: &DecomposerToggleLayer4,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(3, window, cx);
    }

    fn toggle_layer_5_action(
        &mut self,
        _: &DecomposerToggleLayer5,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(4, window, cx);
    }

    fn toggle_layer_6_action(
        &mut self,
        _: &DecomposerToggleLayer6,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(5, window, cx);
    }

    fn toggle_layer_7_action(
        &mut self,
        _: &DecomposerToggleLayer7,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(6, window, cx);
    }

    fn toggle_layer_8_action(
        &mut self,
        _: &DecomposerToggleLayer8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(7, window, cx);
    }

    fn toggle_layer_9_action(
        &mut self,
        _: &DecomposerToggleLayer9,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(8, window, cx);
    }

    // Status component showing enabled/disabled state
    fn render_status(
        &self,
        state: &DecomposerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut DecomposerView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<DecomposerView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Individual layer item component
    fn render_layer_item(
        &self,
        index: usize,
        is_enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        ToggleButton::new(
            format!("Layer {}", index + 1),
            is_enabled,
            move |this: &mut DecomposerView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<DecomposerView>| {
                this.toggle_layer(is_enabled, index, window, cx);
            },
        )
        .with_text("On".to_string(), "Off".to_string())
        .with_keyboard_shortcut(&format!("⌘{}", index + 1))
        .with_no_margin()
        .render(cx)
    }

    // Layers section component
    fn render_layers_section(
        &self,
        state: &DecomposerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(gpui::FontWeight::BOLD).child("Layers:"))
            .child(
                div().flex().flex_col().gap_1().children(
                    state
                        .enabled_layers
                        .iter()
                        .enumerate()
                        .map(|(index, &is_enabled)| self.render_layer_item(index, is_enabled, cx)),
                ),
            )
    }
}

actions!([
    DecomposerToggleActivity,
    DecomposerToggleLayer1,
    DecomposerToggleLayer2,
    DecomposerToggleLayer3,
    DecomposerToggleLayer4,
    DecomposerToggleLayer5,
    DecomposerToggleLayer6,
    DecomposerToggleLayer7,
    DecomposerToggleLayer8,
    DecomposerToggleLayer9
]);

impl FrontendMessageHandler for DecomposerView {
    type State = DecomposerState;

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
            ToolState::DecomposerState(s) => Some(s),
            _ => None,
        }
    }
}

impl Render for DecomposerView {
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
                    .size_full()
                    .text_sm()
                    .p_4()
                    .shadow_lg()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read Decomposer state");
            }
        };

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
                    move |this: &mut DecomposerView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<DecomposerView>| {
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .on_action(cx.listener(Self::toggle_layer_1_action))
            .on_action(cx.listener(Self::toggle_layer_2_action))
            .on_action(cx.listener(Self::toggle_layer_3_action))
            .on_action(cx.listener(Self::toggle_layer_4_action))
            .on_action(cx.listener(Self::toggle_layer_5_action))
            .on_action(cx.listener(Self::toggle_layer_6_action))
            .on_action(cx.listener(Self::toggle_layer_7_action))
            .on_action(cx.listener(Self::toggle_layer_8_action))
            .on_action(cx.listener(Self::toggle_layer_9_action))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_layers_section(&state, cx))
    }
}

impl_tool_view_basics!(DecomposerView, "Decomposer");

impl ToolFrontend for DecomposerView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Decomposer")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(250.), px(350.)),
                origin: Point::new(px(200.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([
            KeyBinding::new("cmd-t", DecomposerToggleActivity, None),
            KeyBinding::new("cmd-1", DecomposerToggleLayer1, None),
            KeyBinding::new("cmd-2", DecomposerToggleLayer2, None),
            KeyBinding::new("cmd-3", DecomposerToggleLayer3, None),
            KeyBinding::new("cmd-4", DecomposerToggleLayer4, None),
            KeyBinding::new("cmd-5", DecomposerToggleLayer5, None),
            KeyBinding::new("cmd-6", DecomposerToggleLayer6, None),
            KeyBinding::new("cmd-7", DecomposerToggleLayer7, None),
            KeyBinding::new("cmd-8", DecomposerToggleLayer8, None),
            KeyBinding::new("cmd-9", DecomposerToggleLayer9, None),
        ]);

        self.process_messages(window, cx);
    }
}
