// Implements UI for Decomposer tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::ToggleButton;
use crate::pipeline::{
    decomposer::DecomposerState, BackendToolType, DecomposerMessage, FrontendMessage,
    PipelineCommand, PipelineObjectMessage, ToolState,
};

use super::ToolFrontend;

pub struct DecomposerView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<DecomposerState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for DecomposerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl DecomposerView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(DecomposerState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Decomposer,
            ),
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Decomposer,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | Decomposer");
        self.process_messages(window, cx);
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let channel_receiver_ref = self
            .receive_channel
            .clone()
            .expect("Should never fire: Created during init");

        // Spawn message processing thread
        cx.spawn_in(window, async move |this, cx| loop {
            if let Ok(command) = channel_receiver_ref.recv_async().await {
                if let PipelineCommand::FrontendMessage(message) = command {
                    match message {
                        FrontendMessage::ReturnState(tool_state) => {
                            if let ToolState::DecomposerState(new_state) = tool_state {
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
                                    eprintln!("Failed to acquire write lock on Decomposer state");
                                }
                            }
                        }
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Decomposer");
                            if let Err(e) = this.update(cx, |this, cx| {
                                if let Ok(mut current_tool_index) = this.current_tool_index.write()
                                {
                                    *current_tool_index = backend_ids.first().copied();
                                } else {
                                    eprintln!("Failed to acquire write lock on current tool index");
                                    return;
                                }
                                this.backend_tool_indicies = backend_ids;
                                this.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.send_channels.first() {
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

    fn get_current_tool_index(&self) -> Option<u16> {
        match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => Some(index),
                None => {
                    eprintln!("No current tool index available");
                    None
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                None
            }
        }
    }

    fn toggle_layer(
        &mut self,
        current_value: bool,
        layer_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::DecomposerMessage(DecomposerMessage::ToggleVisibility(
                    !current_value,
                    layer_index as u16,
                )),
            )) {
                eprintln!("Failed to send toggle layer command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ToggleActive(!current_value),
            )) {
                eprintln!("Failed to send toggle enabled command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
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

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Decomposer")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &DecomposerState, cx: &mut Context<Self>) -> impl IntoElement {
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
    ) -> impl IntoElement {
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
                    .bg(rgb(0x505050))
                    .size_full()
                    .p_4()
                    .shadow_lg()
                    .text_color(rgb(0xffffff))
                    .child("Error: Unable to read Decomposer state");
            }
        };

        div()
            .track_focus(&self.focus_handle(cx))
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
                    move |this: &mut DecomposerView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<DecomposerView>| {
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
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Decomposer,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | Decomposer");

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
