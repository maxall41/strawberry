// Implements UI for Decomposer tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, AppContext,
    Bounds, Context, MouseDownEvent, Point, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
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
                if let PipelineCommand::FrontendMessage(message) = command { match message {
                    FrontendMessage::ReturnState(tool_state) => if let ToolState::DecomposerState(new_state) = tool_state {
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
                    },
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
                } }
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
        .render(cx)
    }

    // Individual layer item component
    fn render_layer_item(
        &self,
        index: usize,
        is_enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(format!("Layer {index}:"))
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(if is_enabled {
                        rgb(0x00aa00)
                    } else {
                        rgb(0xaa0000)
                    })
                    .text_sm()
                    .child(if is_enabled { "On" } else { "Off" })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut DecomposerView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<DecomposerView>| {
                                this.toggle_layer(is_enabled, index, window, cx);
                            },
                        ),
                    ),
            )
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
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
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
}
