// Implements UI for nerve tracer tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};
use rfd::FileDialog;

use crate::gui::ToggleButton;
use strawberry_types::{
    BackendToolType, FrontendMessage, NerveTracerMessage, NerveTracerState, PipelineCommand,
    PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct NerveTracerView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<NerveTracerState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for NerveTracerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl NerveTracerView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(NerveTracerState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                strawberry_types::BackendToolType::NerveTracer,
            ),
            focus_handle: None,
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
                if let PipelineCommand::FrontendMessage(message) = command {
                    match message {
                        FrontendMessage::ReturnState(tool_state) => {
                            if let ToolState::NerveTracerState(new_state) = tool_state {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |_this, cx| cx.notify()) {
                                            eprintln!(
                                                "Failed to update UI after state change: {e}"
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on nerve tracer state");
                                }
                            }
                        }
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend NerveTracer");
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

    fn save_traces(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("traces", &["traces", "xml"])
                .set_directory("~/")
                .save_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::NerveTracerMessage(
                                NerveTracerMessage::SaveTraces(file),
                            ),
                        )) {
                            eprintln!("Failed to send save traces command: {e}");
                        }
                    } else {
                        eprintln!("Invalid tool index: {current_tool_index}");
                    }
                }) {
                    eprintln!("Failed to save traces: {e}");
                }
            }
        })
        .detach();
    }

    fn load_traces(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("traces", &["xml", "traces"])
                .set_directory("~/")
                .pick_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::NerveTracerMessage(
                                NerveTracerMessage::LoadTraces(file),
                            ),
                        )) {
                            eprintln!("Failed to send load traces command: {e}");
                        }
                    } else {
                        eprintln!("Invalid tool index: {current_tool_index}");
                    }
                }) {
                    eprintln!("Failed to load traces: {e}");
                }
            }
        })
        .detach();
    }

    fn commit_segments(&mut self) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::NerveTracerMessage(
                    NerveTracerMessage::CommitCurrentPathSegments(),
                ),
            )) {
                eprintln!("Failed to send commit segments command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn delete_uncommitted_segment(&mut self, segment_id: usize) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::NerveTracerMessage(
                    NerveTracerMessage::DeleteUncomittedPathSegment(segment_id),
                ),
            )) {
                eprintln!("Failed to send delete uncommitted segment command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn delete_path(&mut self, path_id: usize) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::DeletePath(path_id)),
            )) {
                eprintln!("Failed to send delete path command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn delete_path_segment(&mut self, path_id: usize, segment_id: usize) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::NerveTracerMessage(
                    NerveTracerMessage::DeletePathSegmentInCommitedPath(path_id, segment_id),
                ),
            )) {
                eprintln!("Failed to send delete path segment command: {e}");
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
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
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

    fn set_selected_image(&mut self, image_index: usize) {
        let current_tool_index = match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::NerveTracerMessage(
                    NerveTracerMessage::SetSelectedImageIndex(image_index),
                ),
            )) {
                eprintln!("Failed to send set selected image command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn toggle_activity_action(
        &mut self,
        _: &NerveTracerToggleActivity,
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

    fn commit_segments_action(
        &mut self,
        _: &NerveTracerCommitSegments,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_segments();
        cx.notify();
    }

    fn save_traces_action(
        &mut self,
        _: &NerveTracerSaveTraces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_traces(window, cx);
    }

    fn load_traces_action(
        &mut self,
        _: &NerveTracerLoadTraces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_traces(window, cx);
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Nerve Tracer")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &NerveTracerState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut NerveTracerView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<NerveTracerView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Image selector component
    fn render_image_selector(
        &self,
        state: &NerveTracerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if state.total_images <= 1 {
            return div();
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Source Image:"),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .flex_wrap()
                    .children((0..state.total_images).map(|i| {
                        let is_selected = i == state.selected_image_index;
                        div()
                            .px_3()
                            .py_1()
                            .bg(if is_selected {
                                rgb(0x0066cc)
                            } else {
                                rgb(0x444444)
                            })
                            .hover(|style| {
                                style.bg(if is_selected {
                                    rgb(0x0077dd)
                                } else {
                                    rgb(0x555555)
                                })
                            })
                            .rounded_md()
                            .text_sm()
                            .text_center()
                            .child(format!("{}", i + 1))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    move |this: &mut NerveTracerView,
                                          _event: &MouseDownEvent,
                                          _window: &mut Window,
                                          _cx: &mut Context<NerveTracerView>| {
                                        this.set_selected_image(i);
                                    },
                                ),
                            )
                    })),
            )
    }

    // Segments section
    fn render_segments_section(
        &self,
        state: &NerveTracerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .mb_2()
                    .child("Segments:"),
            )
            .child(
                div()
                    .id("segments-list")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(rgb(0x333333))
                    .rounded_md()
                    .h(px(150.))
                    .overflow_scroll()
                    .when(state.current_path_segments.is_empty(), |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xaaaaaa))
                                .child("No segments yet. Click on the image to create segments."),
                        )
                    })
                    .when(!state.current_path_segments.is_empty(), |el| {
                        el.children(
                            state
                                .current_path_segments
                                .iter()
                                .enumerate()
                                .map(|(idx, segment)| {
                                    self.render_segment_item(idx, segment.len(), cx)
                                }),
                        )
                    }),
            )
    }

    // Individual segment item
    fn render_segment_item(
        &self,
        segment_id: usize,
        point_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .bg(rgb(0x444444))
            .hover(|style| style.bg(rgb(0x555555)))
            .rounded_md()
            .child(div().flex_1().child(format!(
                "Segment {:03} ({} points)",
                segment_id + 1,
                point_count
            )))
            .child(
                div()
                    .w_6()
                    .h_6()
                    .bg(rgb(0xaa0000))
                    .hover(|style| style.bg(rgb(0xbb1111)))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_lg()
                    .text_color(rgb(0xffffff))
                    .child("×")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut NerveTracerView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<NerveTracerView>| {
                                this.delete_uncommitted_segment(segment_id);
                            },
                        ),
                    ),
            )
    }

    // Add to path button
    fn render_add_to_path_button(
        &self,
        state: &NerveTracerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_segments = !state.current_path_segments.is_empty();

        if has_segments {
            div()
                .w_full()
                .px_4()
                .py_2()
                .my_2()
                .bg(rgb(0x0066cc))
                .hover(|style| style.bg(rgb(0x0077dd)))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .child("COMMIT")
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(
                        |this: &mut NerveTracerView,
                         _event: &MouseDownEvent,
                         _window: &mut Window,
                         _cx: &mut Context<NerveTracerView>| {
                            this.commit_segments();
                        },
                    ),
                )
        } else {
            div()
                .w_full()
                .px_4()
                .py_2()
                .my_2()
                .bg(rgb(0x555555))
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(rgb(0x888888))
                .child("COMMIT")
        }
    }

    // Paths section
    fn render_paths_section(
        &self,
        state: &NerveTracerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .mb_2()
                    .child("Paths:"),
            )
            .child(
                div()
                    .id("paths-list")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(rgb(0x333333))
                    .rounded_md()
                    .h(px(150.))
                    .overflow_scroll()
                    .when(state.paths.is_empty(), |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xaaaaaa))
                                .child("No paths yet. Create segments and add them to a path."),
                        )
                    })
                    .when(!state.paths.is_empty(), |el| {
                        el.children(
                            state
                                .paths
                                .iter()
                                .enumerate()
                                .map(|(path_idx, path)| self.render_path_item(path_idx, path, cx)),
                        )
                    }),
            )
    }

    // Individual path item (tree parent)
    fn render_path_item(
        &self,
        path_id: usize,
        path: &Vec<Vec<(u64, u64)>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .mb_2()
            .child(
                // Path header
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_2()
                    .bg(rgb(0x555555))
                    .hover(|style| style.bg(rgb(0x666666)))
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(format!("Path {:03}", path_id + 1)),
                    )
                    .child(
                        div()
                            .w_6()
                            .h_6()
                            .bg(rgb(0xaa0000))
                            .hover(|style| style.bg(rgb(0xbb1111)))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_lg()
                            .text_color(rgb(0xffffff))
                            .child("×")
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    move |this: &mut NerveTracerView,
                                          _event: &MouseDownEvent,
                                          _window: &mut Window,
                                          _cx: &mut Context<NerveTracerView>| {
                                        this.delete_path(path_id);
                                    },
                                ),
                            ),
                    ),
            )
            .child(
                // Path segments (children, indented)
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl_4()
                    .children(path.iter().enumerate().map(|(segment_idx, segment)| {
                        self.render_path_segment_item(path_id, segment_idx, segment.len(), cx)
                    })),
            )
    }

    // Individual segment within a path (tree child)
    fn render_path_segment_item(
        &self,
        path_id: usize,
        segment_id: usize,
        point_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .bg(rgb(0x444444))
            .hover(|style| style.bg(rgb(0x555555)))
            .rounded_md()
            .child(div().flex_1().child(format!(
                "Segment {:03} ({} points)",
                segment_id + 1,
                point_count
            )))
            .child(
                div()
                    .w_5()
                    .h_5()
                    .bg(rgb(0xaa0000))
                    .hover(|style| style.bg(rgb(0xbb1111)))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(0xffffff))
                    .child("×")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut NerveTracerView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<NerveTracerView>| {
                                this.delete_path_segment(path_id, segment_id);
                            },
                        ),
                    ),
            )
    }

    // Storage buttons (Save/Load)
    fn render_storage_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x006600))
                    .hover(|style| style.bg(rgb(0x007700)))
                    .rounded_md()
                    .text_sm()
                    .text_center()
                    .child("SAVE")
                    .id("save-button")
                    .tooltip(move |_window, cx| {
                        let tooltip_view = crate::gui::tooltip::TooltipView {
                            text: "⌘S".to_string(),
                        };
                        cx.new(|_cx| tooltip_view).into()
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |this: &mut NerveTracerView,
                             _event: &MouseDownEvent,
                             window: &mut Window,
                             cx: &mut Context<NerveTracerView>| {
                                this.save_traces(window, cx);
                            },
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x0066cc))
                    .hover(|style| style.bg(rgb(0x0077dd)))
                    .rounded_md()
                    .text_sm()
                    .text_center()
                    .child("LOAD")
                    .id("load-button")
                    .tooltip(move |_window, cx| {
                        let tooltip_view = crate::gui::tooltip::TooltipView {
                            text: "⌘L".to_string(),
                        };
                        cx.new(|_cx| tooltip_view).into()
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |this: &mut NerveTracerView,
                             _event: &MouseDownEvent,
                             window: &mut Window,
                             cx: &mut Context<NerveTracerView>| {
                                this.load_traces(window, cx);
                            },
                        ),
                    ),
            )
    }
}

actions!([
    NerveTracerToggleActivity,
    NerveTracerCommitSegments,
    NerveTracerSaveTraces,
    NerveTracerLoadTraces
]);

impl Render for NerveTracerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.process_messages(_window, cx);

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
                    .child("Error: Unable to read nerve tracer state");
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
                    move |this: &mut NerveTracerView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<NerveTracerView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .on_action(cx.listener(Self::commit_segments_action))
            .on_action(cx.listener(Self::save_traces_action))
            .on_action(cx.listener(Self::load_traces_action))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_image_selector(&state, cx))
            .child(self.render_segments_section(&state, cx))
            .child(self.render_add_to_path_button(&state, cx))
            .child(self.render_paths_section(&state, cx))
            .child(div().mt_auto().child(self.render_storage_buttons(cx)))
    }
}

impl ToolFrontend for NerveTracerView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Nerve Tracer")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(400.), px(700.)),
                origin: Point::new(px(1130.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver.clone());
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::NerveTracer,
            sender.clone(),
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | NerveTracer");

        cx.bind_keys([
            KeyBinding::new("cmd-t", NerveTracerToggleActivity, None),
            KeyBinding::new("cmd-c", NerveTracerCommitSegments, None),
            KeyBinding::new("cmd-s", NerveTracerSaveTraces, None),
            KeyBinding::new("cmd-l", NerveTracerLoadTraces, None),
        ]);

        self.process_messages(window, cx);
    }
}
