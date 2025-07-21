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
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::Compositor(vec![]),
            ))
            .unwrap();
        println!("Send discovery request to backend | Compositor");
        self.process_messages(window, cx);
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sender_ref = self.command_sender.clone();
        let mut receiver_ref = self.command_receiver.resubscribe();
        let state = self.state.clone();
        let current_tool_index_ref = self.current_tool_index.clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let mut recv = false;
                while !recv {
                    if let Ok(command) = receiver_ref.try_recv() {
                        match command {
                            PipelineCommand::FrontendMessage(message) => match message {
                                FrontendMessage::ReturnState(tool_state) => match tool_state {
                                    ToolState::CompositorState(new_state) => {
                                        let mut locked_state = state.write().unwrap();
                                        if *locked_state != new_state {
                                            *locked_state = new_state;
                                            this.update(cx, |this, cx| cx.notify()).unwrap();
                                        }
                                        recv = true;
                                    }
                                    _ => {}
                                },
                                FrontendMessage::ReturnBackendTools(backend_tool_type) => {
                                    match backend_tool_type {
                                        BackendToolType::Compositor(data) => {
                                            println!("AHFAHJ");
                                            this.update(cx, |this, cx| {
                                                let index = data[0];
                                                this.backend_tool_indicies = data;
                                                *this.current_tool_index.write().unwrap() =
                                                    Some(index);
                                                cx.notify()
                                            })
                                            .unwrap();
                                            recv = true; // Exit the loop when we get backend tools
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            },
                            _ => {}
                        }
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(10))
                        .await;
                }

                // Check if we have a current tool index before sending RequestState
                let current_tool_index = current_tool_index_ref.read().unwrap();
                if let Some(tool_index) = *current_tool_index {
                    // println!("Sending state request! | Compositor");
                    sender_ref
                        .send(PipelineCommand::RequestState(tool_index))
                        .unwrap();
                }
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await; // 30FPS
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
        let current_tool_index = self.current_tool_index.read().unwrap().unwrap();
        self.command_sender
            .send(PipelineCommand::PipelineObject(
                self.backend_tool_indicies[current_tool_index as usize],
                PipelineObjectMessage::CompositorMessage(CompositorMessage::ToggleVisibility(
                    !current_value,
                    layer_index as u16,
                )),
            ))
            .unwrap();
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
        let state = self.state.read().unwrap();

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
