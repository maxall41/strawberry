// Implements UI for compositor tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::ToggleButton;
use strawberry_types::{
    BackendToolType, CompositorMessage, CompositorState, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct CompositorView {
    pub state: Arc<RwLock<CompositorState>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for CompositorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl CompositorView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(CompositorState::default())),
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Compositor,
            ),
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let channel_receiver_ref = self
            .base
            .receive_channel
            .clone()
            .expect("Should never fire: Created during init");

        // Spawn message processing thread
        cx.spawn_in(window, async move |this, cx| loop {
            if let Ok(command) = channel_receiver_ref.recv_async().await {
                if let PipelineCommand::FrontendMessage(message) = command {
                    match message {
                        FrontendMessage::ReturnState(tool_state) => {
                            if let ToolState::CompositorState(new_state) = tool_state {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| cx.notify()) {
                                            eprintln!(
                                                "Failed to update UI after state change: {e}"
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on compositor state");
                                }
                            }
                        }
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Compositor");
                            if let Err(e) = this.update(cx, |this, cx| {
                                if let Ok(mut current_tool_index) =
                                    this.base.current_tool_index.write()
                                {
                                    *current_tool_index = backend_ids.first().copied();
                                } else {
                                    eprintln!("Failed to acquire write lock on current tool index");
                                    return;
                                }
                                this.base.backend_tool_indicies = backend_ids;
                                this.base.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.base.send_channels.first() {
                                    if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                        eprintln!("Failed to request initial state: {e}");
                                    }
                                }
                            }) {
                                eprintln!("Failed to update backend tools: {e}");
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                break;
            }
        })
        .detach();
    }

    fn toggle_layer(
        &mut self,
        current_value: bool,
        layer_index: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::CompositorMessage(CompositorMessage::ToggleVisibility(
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
        _: &CompositorToggleActivity,
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
    fn render_status(&self, state: &CompositorState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut CompositorView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<CompositorView>| {
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
    ) -> impl IntoElement {
        ToggleButton::new(
            format!("Layer {}", index + 1),
            is_enabled,
            move |this: &mut CompositorView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<CompositorView>| {
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
        state: &CompositorState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                                Some(self.render_layer_item(index, is_enabled, cx))
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
                    .bg(rgb(0x505050))
                    .size_full()
                    .p_4()
                    .shadow_lg()
                    .text_color(rgb(0xffffff))
                    .child("Error: Unable to read compositor state");
            }
        };

        div()
            .track_focus(&self.base.focus_handle.clone().expect("initialized"))
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut CompositorView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<CompositorView>| {
                        this.focus_handle(cx).focus(window);
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
        let (sender, receiver) = flume::unbounded();
        self.base.receive_channel = Some(receiver);
        self.base.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self
            .base
            .command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::Compositor,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | Compositor");

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
