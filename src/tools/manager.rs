// Implements UI for compositor tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, Bounds, Context, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use rfd::FileDialog;

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
    pub original_position: u16,
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
    pub tool_positions: Vec<(f32, u16)>,
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
            tool_positions: vec![],
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Compositor,
            ),
        }
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
                                        this.start_drag(tool_position, Point::new(event.position.x.into(), event.position.y.into()));
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
                    .child(format!("#{}", index)),
            )
    }

    // Tools list component
    fn render_tools(
        &mut self,
        sorted_tools: &mut Vec<ToolProxy>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        sorted_tools.sort_by_key(|tool| tool.position);

        // Estimate tool positions based on layout
        self.tool_positions.clear();
        let header_height = 40.0; // "Tools" header height
        let tool_height = 58.0; // p_3 (12px) + content + gap
        for (i, tool) in sorted_tools.iter().enumerate() {
            let estimated_y = header_height + (i as f32 * tool_height);
            self.tool_positions.push((estimated_y, tool.position));
        }

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

    fn start_drag(&mut self, tool_position: u16, position: Point<f32>) {
        self.drag_state = Some(DragState {
            dragging_tool_position: tool_position,
            drag_start_position: position,
            current_position: position,
            original_position: tool_position,
        });
        println!("Started dragging tool from position {}", tool_position);
    }

    fn update_drag(&mut self, position: Point<f32>) {
        if let Some(ref mut drag_state) = self.drag_state {
            drag_state.current_position = position;
        }
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
                eprintln!("Failed to read current tool index: {}", e);
                None
            }
        }
    }

    fn end_drag(&mut self, position: Point<f32>) {
        if let Some(drag_state) = self.drag_state.take() {
            let Some(current_tool_index) = self.get_current_tool_index() else {
                return;
            };

            // Calculate new position based on closest tool element position
            let (original_index, drop_index) = if let Ok(state) = self.state.read() {
                let mut sorted_tools = state.proxy_tools.clone();
                sorted_tools.sort_by_key(|tool| tool.position);

                // Find the original index in the enabled tools vector
                let original_index = self.tool_positions[sorted_tools
                    .iter()
                    .position(|tool| tool.position == drag_state.original_position)
                    .unwrap_or(0)]
                .1;

                // Get mouse Y position relative to the drop location
                let drop_y = position.y;
                let going_up = drop_y < drag_state.drag_start_position.y;

                // Find the closest tool element position
                let mut closest_index = original_index;
                let mut closest_distance = f32::INFINITY;

                for &(tool_y, pos_index) in self.tool_positions.iter() {
                    let distance = (drop_y - tool_y).abs();
                    if distance < closest_distance {
                        closest_distance = distance;
                        closest_index = pos_index;
                    }
                }

                // Apply the logic: if going up get min(index-1, 0), if going down get index+1
                let target_index = if going_up {
                    closest_index.saturating_sub(1)
                } else {
                    // Going down
                    closest_index + 1
                };

                (original_index, target_index)
            } else {
                eprintln!("Failed to read state!");
                (0, 0)
            };

            let manager_message =
                ManagerMessage::ReorderTool(original_index as u16, drop_index as u16);
            println!(
                "=== REORDER TOOL ===\nOriginal index: {}\nDrop index: {}\nPipeline position: #{} -> moving to position near index {}",
                original_index, drop_index, drag_state.original_position, drop_index
            );
            self.send_channels[current_tool_index as usize]
                .send(PipelineCommand::PipelineObject(
                    PipelineObjectMessage::ManagerMessage(manager_message),
                ))
                .unwrap();
        }
    }

    fn save_pipeline(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("toml", &["toml"])
                .set_directory("/")
                .save_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::ManagerMessage(ManagerMessage::SavePipeline(
                                file,
                            )),
                        )) {
                            eprintln!("Failed to send save pipeline command: {}", e);
                            return;
                        }
                    } else {
                        eprintln!("Invalid tool index: {}", current_tool_index);
                        return;
                    }
                }) {
                    eprintln!("Failed to save pipeline: {}", e);
                    return;
                }
            }
        })
        .detach();
    }

    fn load_pipeline(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("toml", &["toml"])
                .set_directory("/")
                .pick_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::ManagerMessage(ManagerMessage::LoadPipeline(
                                file,
                            )),
                        )) {
                            eprintln!("Failed to send load pipeline command: {}", e);
                            return;
                        }
                    } else {
                        eprintln!("Invalid tool index: {}", current_tool_index);
                        return;
                    }
                }) {
                    eprintln!("Failed to load pipeline: {}", e);
                    return;
                }
            }
        })
        .detach();
    }

    fn render_storage_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x006600))
                    .hover(|style| style.bg(rgb(0x007700)))
                    .rounded_md()
                    .text_sm()
                    .child("Save")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut ManagerView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<ManagerView>| {
                                _this.save_pipeline(_window, _cx);
                            },
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x0066cc))
                    .hover(|style| style.bg(rgb(0x0077dd)))
                    .rounded_md()
                    .text_sm()
                    .child("Load")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut ManagerView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<ManagerView>| {
                                _this.load_pipeline(_window, _cx);
                            },
                        ),
                    ),
            )
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
            .child({
                let mut tools_clone = state.proxy_tools.clone();
                drop(state); // Explicitly drop the guard
                self.render_tools(&mut tools_clone, cx)
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .mt_auto()
                    .child(self.render_storage_buttons(cx)),
            )
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

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Manager,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Manager");
        self.process_messages(window, cx);
    }
}
