use crate::gui::actions::actions;
use crate::gui::theme;
// Implements UI for compositor tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Point, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    CompositorMessage, CompositorState, PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct CompositorView {
    pub state: Arc<RwLock<CompositorState>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for CompositorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl CompositorView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(CompositorState::default())),
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::Compositor,
                cx.focus_handle(),
            ),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_layer(
        &self,
        current_value: bool,
        layer_index: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Ok(layer_index) = u16::try_from(layer_index) else {
            eprintln!("Layer index exceeds the backend command range");
            return;
        };
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::CompositorMessage(CompositorMessage::ToggleVisibility(
                !current_value,
                layer_index,
            )),
        )) {
            eprintln!("Failed to send toggle layer command: {e}");
        }
    }

    fn toggle_enabled(&self, current_value: bool, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

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

    fn toggle_layer_by_index(
        &self,
        layer_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&current_value) = state.enabled_layers.get(layer_index) {
            drop(state);
            self.toggle_layer(current_value, layer_index, window, cx);
        }
    }

    fn toggle_layer_1_action(
        &mut self,
        _: &CompositorToggleLayer1,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(0, window, cx);
    }

    fn toggle_layer_2_action(
        &mut self,
        _: &CompositorToggleLayer2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(1, window, cx);
    }

    fn toggle_layer_3_action(
        &mut self,
        _: &CompositorToggleLayer3,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(2, window, cx);
    }

    fn toggle_layer_4_action(
        &mut self,
        _: &CompositorToggleLayer4,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(3, window, cx);
    }

    fn toggle_layer_5_action(
        &mut self,
        _: &CompositorToggleLayer5,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(4, window, cx);
    }

    fn toggle_layer_6_action(
        &mut self,
        _: &CompositorToggleLayer6,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(5, window, cx);
    }

    fn toggle_layer_7_action(
        &mut self,
        _: &CompositorToggleLayer7,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(6, window, cx);
    }

    fn toggle_layer_8_action(
        &mut self,
        _: &CompositorToggleLayer8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(7, window, cx);
    }

    fn toggle_layer_9_action(
        &mut self,
        _: &CompositorToggleLayer9,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_layer_by_index(8, window, cx);
    }

    // Status component showing enabled/disabled state
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

    // Individual layer item component
    fn render_layer_item(
        index: usize,
        is_enabled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        ToggleButton::new(
            format!("Layer {}", index.saturating_add(1)),
            is_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.toggle_layer(is_enabled, index, window, cx);
            },
        )
        .with_text("On".to_string(), "Off".to_string())
        .with_keyboard_shortcut(&format!("⌘{}", index.saturating_add(1)))
        .with_no_margin()
        .render(cx)
    }

    // Layers section component
    fn render_layers_section(
        state: &CompositorState,
        cx: &Context<Self>,
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
                        .filter_map(|(index, &is_enabled)| {
                            if index >= state.real_layers_len {
                                None
                            } else {
                                Some(Self::render_layer_item(index, is_enabled, cx))
                            }
                        }),
                ),
            )
    }
}

actions!([
    CompositorToggleActivity,
    CompositorToggleLayer1,
    CompositorToggleLayer2,
    CompositorToggleLayer3,
    CompositorToggleLayer4,
    CompositorToggleLayer5,
    CompositorToggleLayer6,
    CompositorToggleLayer7,
    CompositorToggleLayer8,
    CompositorToggleLayer9
]);

impl FrontendMessageHandler for CompositorView {
    type State = CompositorState;

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
            ToolState::CompositorState(s) => Some(s),
            _ => None,
        }
    }
}

impl Render for CompositorView {
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
                    .child("Error: Unable to read compositor state");
            }
        };

        div()
            .track_focus(&self.focus_handle(cx))
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
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
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
            .child(Self::render_status(&state, cx))
            .child(Self::render_layers_section(&state, cx))
    }
}

impl_tool_view_basics!(CompositorView, "Compositor");

impl ToolFrontend for CompositorView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Compositor")),
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
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([
            KeyBinding::new("cmd-t", CompositorToggleActivity, None),
            KeyBinding::new("cmd-1", CompositorToggleLayer1, None),
            KeyBinding::new("cmd-2", CompositorToggleLayer2, None),
            KeyBinding::new("cmd-3", CompositorToggleLayer3, None),
            KeyBinding::new("cmd-4", CompositorToggleLayer4, None),
            KeyBinding::new("cmd-5", CompositorToggleLayer5, None),
            KeyBinding::new("cmd-6", CompositorToggleLayer6, None),
            KeyBinding::new("cmd-7", CompositorToggleLayer7, None),
            KeyBinding::new("cmd-8", CompositorToggleLayer8, None),
            KeyBinding::new("cmd-9", CompositorToggleLayer9, None),
        ]);

        self.process_messages(window, cx);
    }
}
