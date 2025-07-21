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
    compositor::CompositorState, CompositorMessage, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolState,
};

use super::ToolFrontend;

pub struct CompositorView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<CompositorState>>,
}

impl CompositorView {
    pub fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sender_ref = self.command_sender.clone();
        let mut receiver_ref = self.command_receiver.resubscribe();
        let state = self.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                sender_ref.send(PipelineCommand::RequestState(0)).unwrap();
                let mut recv = false;
                while !recv {
                    // TODO: DANGER!
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
                                },
                            },
                            _ => {}
                        }
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(10))
                        .await;
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
        self.command_sender
            .send(PipelineCommand::PipelineObject(
                0,
                PipelineObjectMessage::CompositorMessage(CompositorMessage::ToggleVisibility(
                    !current_value,
                    layer_index as u16,
                )),
            ))
            .unwrap();
    }
}
impl Render for CompositorView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::BOLD)
                    .mb_4()
                    .child("Compositor"),
            )
            .child(
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
                    ),
            )
            .child(
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
                                .map(|(index, &is_enabled)| {
                                    if index >= state.real_layers_len {
                                        return div();
                                    }
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
                                                    {
                                                        let index = index;
                                                        let is_enabled = is_enabled;
                                                        _cx.listener(move |this: &mut CompositorView, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<CompositorView>| {
                                                            this.toggle_layer(is_enabled, index, window, cx);
                                                        })
                                                    },
                                                ),
                                        )
                                }),
                        ),
                    ),
            )
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
