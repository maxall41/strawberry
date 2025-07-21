// Implements UI for compositor tool

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use gpui::{
    actions, div, img, prelude::*, px, rgb, size, App, AppContext, Application, AssetSource,
    Bounds, Context, FocusHandle, Focusable, ImageSource, KeyBinding, KeyDownEvent, KeyUpEvent,
    Menu, MenuItem, MouseDownEvent, Point, RenderImage, SharedString, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};

use crate::pipeline::{
    compositor::CompositorState, BackendToolType, CompositorMessage, FrontendMessage,
    PipelineCommand, PipelineObjectMessage, ToolState,
};

use super::ToolFrontend;

pub struct CompositorView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<CompositorState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
}

impl CompositorView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender,
            state: Arc::new(RwLock::new(CompositorState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Compositor,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Compositor");
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
                match command {
                    PipelineCommand::FrontendMessage(message) => match message {
                        FrontendMessage::ReturnState(tool_state) => match tool_state {
                            ToolState::CompositorState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| cx.notify()) {
                                            eprintln!(
                                                "Failed to update UI after state change: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on compositor state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Compositor");
                            if let Err(e) = this.update(cx, |this, cx| {
                                if let Ok(mut current_tool_index) = this.current_tool_index.write()
                                {
                                    *current_tool_index = backend_ids.get(0).copied();
                                } else {
                                    eprintln!("Failed to acquire write lock on current tool index");
                                    return;
                                }
                                this.backend_tool_indicies = backend_ids;
                                this.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.send_channels.get(0) {
                                    if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                        eprintln!("Failed to request initial state: {}", e);
                                    }
                                }
                            }) {
                                eprintln!("Failed to update backend tools: {}", e);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        })
        .detach();
    }

    fn toggle_layer(
        &mut self,
        current_value: bool,
        layer_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {}", e);
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CompositorMessage(CompositorMessage::ToggleVisibility(
                    !current_value,
                    layer_index as u16,
                )),
            )) {
                eprintln!("Failed to send toggle layer command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Compositor")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &CompositorState) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .mb_3()
            .child("Enabled:")
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(if state.is_enabled {
                        rgb(0x00aa00)
                    } else {
                        rgb(0xaa0000)
                    })
                    .child(if state.is_enabled { "Yes" } else { "No" }),
            )
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
            .child(format!("Layer {}:", index))
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
                            move |this: &mut CompositorView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<CompositorView>| {
                                this.toggle_layer(is_enabled, index, window, cx);
                            },
                        ),
                    ),
            )
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

impl Render for CompositorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(e) => {
                eprintln!("Failed to read state for rendering: {}", e);
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
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
            .child(self.render_header())
            .child(self.render_status(&state))
            .child(self.render_layers_section(&state, cx))
    }
}

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
}
