// Implements UI for compositor tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, Bounds, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::pipeline::{
    manager::{ManagerState, ToolProxy},
    BackendToolType, FrontendMessage, ManagerMessage, PipelineCommand, PipelineObjectMessage,
    ToolState,
};

use super::ToolFrontend;

#[derive(Clone)]
pub struct DragState {
    pub dragging_tool_position: u16,
    pub drag_start_position: Point<f32>,
    pub current_position: Point<f32>,
    pub original_index: usize,
}

pub struct ManagerView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<ManagerState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub drag_state: Option<DragState>,
    _cleanup_guard: crate::tools::CleanupGuard,
}

impl ManagerView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(ManagerState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            drag_state: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Compositor,
            ),
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Manager,
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
                            ToolState::ManagerState(new_state) => {
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
            } else {
                break;
            }
        })
        .detach();
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Pipeline")
    }

    fn render_tool(
        &self,
        tool: &ToolProxy,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_dragging = self
            .drag_state
            .as_ref()
            .map(|drag| drag.dragging_tool_position == tool.position)
            .unwrap_or(false);

        let opacity = if is_dragging { 0.5 } else { 1.0 };
        let bg_color = if is_dragging {
            rgb(0x707070)
        } else {
            rgb(0x606060)
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .p_3()
            .bg(bg_color)
            .opacity(opacity)
            .rounded_md()
            .border_1()
            .border_color(rgb(0x707070))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xCCCCCC))
                            .cursor_pointer()
                            .child("⋮⋮")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener({
                                    let tool_position = tool.position;
                                    move |this: &mut ManagerView, event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<ManagerView>| {
                                        cx.stop_propagation();
                                        this.start_drag(tool_position, Point::new(event.position.x.into(), event.position.y.into()), index);
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(tool.name.clone()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0xCCCCCC))
                    .child(format!("#{}", tool.position)),
            )
    }

    // Tools list component
    fn render_tools(
        &self,
        state: &crate::pipeline::manager::ManagerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut sorted_tools = state.proxy_tools.clone();
        sorted_tools.sort_by_key(|tool| tool.position);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .mb_3()
                    .child("Tools"),
            )
            .children(
                sorted_tools
                    .iter()
                    .enumerate()
                    .map(|(index, tool)| self.render_tool(tool, index, cx)),
            )
    }

    fn start_drag(&mut self, tool_position: u16, position: Point<f32>, index: usize) {
        self.drag_state = Some(DragState {
            dragging_tool_position: tool_position,
            drag_start_position: position,
            current_position: position,
            original_index: index,
        });
        println!(
            "Started dragging tool {} from index {}",
            tool_position, index
        );
    }

    fn update_drag(&mut self, position: Point<f32>) {
        if let Some(ref mut drag_state) = self.drag_state {
            drag_state.current_position = position;
        }
    }

    fn end_drag(&mut self, position: Point<f32>) {
        if let Some(drag_state) = self.drag_state.take() {
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

            // Calculate new position based on mouse position
            let drop_index = if let Ok(state) = self.state.read() {
                let mut sorted_tools = state.proxy_tools.clone();
                sorted_tools.sort_by_key(|tool| tool.position);

                if position.y > drag_state.drag_start_position.y {
                    (drag_state.original_index + 1).min(sorted_tools.len() - 1)
                } else {
                    drag_state.original_index.saturating_sub(1)
                }
            } else {
                eprintln!("Failed to read state!");
                drag_state.original_index
            };

            let manager_message =
                ManagerMessage::ReorderTool(drag_state.original_index as u16, drop_index as u16);
            println!("Moving {} to {}", drag_state.original_index, drop_index);
            self.send_channels[current_tool_index as usize]
                .send(PipelineCommand::PipelineObject(
                    PipelineObjectMessage::ManagerMessage(manager_message),
                ))
                .unwrap();
        }
    }
}

impl Render for ManagerView {
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
                    .child("Error: Unable to read manager state");
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
            .child(self.render_tools(&state, cx))
            .on_mouse_move(cx.listener(
                |this: &mut ManagerView,
                 event: &MouseMoveEvent,
                 _window: &mut Window,
                 cx: &mut Context<ManagerView>| {
                    if this.drag_state.is_some() {
                        this.update_drag(Point::new(
                            event.position.x.into(),
                            event.position.y.into(),
                        ));
                        cx.notify();
                    }
                },
            ))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(
                    |this: &mut ManagerView,
                     event: &MouseUpEvent,
                     _window: &mut Window,
                     cx: &mut Context<ManagerView>| {
                        if this.drag_state.is_some() {
                            this.end_drag(Point::new(
                                event.position.x.into(),
                                event.position.y.into(),
                            ));
                            cx.notify();
                        }
                    },
                ),
            )
    }
}

impl ToolFrontend for ManagerView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Pipeline Manager")),
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
