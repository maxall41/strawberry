use crate::gui::theme;
// Implements UI for nerve tracer tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Point, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};
use rfd::FileDialog;

use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    NerveTracerMessage, NerveTracerState, PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct NerveTracerView {
    pub state: Arc<RwLock<NerveTracerState>>,
    pub last_directory: Arc<RwLock<Option<PathBuf>>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for NerveTracerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl NerveTracerView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(NerveTracerState::default())),
            last_directory: Arc::new(RwLock::new(None)),
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::NerveTracer,
            ),
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn save_traces(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let last_directory = self.last_directory.clone();

        cx.spawn_in(_window, async move |this, cx| {
            let default_dir = last_directory
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~/".to_string());

            let file = FileDialog::new()
                .add_filter("traces", &["traces", "xml"])
                .set_directory(&default_dir)
                .save_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, _cx| {
                    if let Err(e) = this.base.send_to_backend(PipelineCommand::PipelineObject(
                        PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::SaveTraces(
                            file,
                        )),
                    )) {
                        eprintln!("Failed to send save traces command: {e}");
                    }
                }) {
                    eprintln!("Failed to save traces: {e}");
                }
            }
        })
        .detach();
    }

    fn load_traces(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let last_directory = self.last_directory.clone();

        cx.spawn_in(_window, async move |this, cx| {
            let default_dir = last_directory
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~/".to_string());

            let file = FileDialog::new()
                .add_filter("traces", &["xml", "traces"])
                .set_directory(&default_dir)
                .pick_file();

            if let Some(file) = file {
                // Store the directory for future save/load operations
                if let Some(parent) = file.parent() {
                    if let Ok(mut dir) = last_directory.write() {
                        *dir = Some(parent.to_path_buf());
                    }
                }

                if let Err(e) = this.update(cx, |this, _cx| {
                    if let Err(e) = this.base.send_to_backend(PipelineCommand::PipelineObject(
                        PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::LoadTraces(
                            file,
                        )),
                    )) {
                        eprintln!("Failed to send load traces command: {e}");
                    }
                }) {
                    eprintln!("Failed to load traces: {e}");
                }
            }
        })
        .detach();
    }

    fn commit_segments(&mut self) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::CommitCurrentPathSegments(),
            ),
        )) {
            eprintln!("Failed to send commit segments command: {e}");
        }
    }

    fn delete_uncommitted_segment(&mut self, segment_id: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeleteUncomittedPathSegment(segment_id),
            ),
        )) {
            eprintln!("Failed to send delete uncommitted segment command: {e}");
        }
    }

    fn delete_path(&mut self, path_id: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::DeletePath(path_id)),
        )) {
            eprintln!("Failed to send delete path command: {e}");
        }
    }

    fn delete_path_segment(&mut self, path_id: usize, segment_id: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeletePathSegmentInCommitedPath(path_id, segment_id),
            ),
        )) {
            eprintln!("Failed to send delete path segment command: {e}");
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

    fn set_selected_image(&mut self, image_index: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::SetSelectedImageIndex(
                image_index,
            )),
        )) {
            eprintln!("Failed to send set selected image command: {e}");
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
    // Status component showing enabled/disabled state
    fn render_status(
        &self,
        state: &NerveTracerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
    ) -> impl IntoElement + use<> {
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
                                theme::ACCENT
                            } else {
                                theme::CONTROL
                            })
                            .cursor_pointer()
                            .hover(move |style| {
                                style.bg(if is_selected {
                                    theme::ACCENT_HOVER
                                } else {
                                    theme::CONTROL_HOVER
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
    ) -> impl IntoElement + use<> {
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
                    .bg(theme::SURFACE)
                    .rounded_md()
                    .h(px(150.))
                    .overflow_scroll()
                    .when(state.current_path_segments.is_empty(), |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(theme::TEXT_MUTED)
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
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .bg(theme::CONTROL)
            .cursor_pointer()
            .hover(|style| style.bg(theme::CONTROL_HOVER))
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
                    .bg(theme::DANGER)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::DANGER_HOVER))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_lg()
                    .text_color(theme::TEXT)
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
    ) -> impl IntoElement + use<> {
        let has_segments = !state.current_path_segments.is_empty();

        if has_segments {
            div()
                .w_full()
                .px_4()
                .py_2()
                .my_2()
                .bg(theme::ACCENT)
                .cursor_pointer()
                .hover(|style| style.bg(theme::ACCENT_HOVER))
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
                .bg(theme::CONTROL)
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child("COMMIT")
        }
    }

    // Paths section
    fn render_paths_section(
        &self,
        state: &NerveTracerState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                    .bg(theme::SURFACE)
                    .rounded_md()
                    .h(px(150.))
                    .overflow_scroll()
                    .when(state.paths.is_empty(), |el| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(theme::TEXT_MUTED)
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
    ) -> impl IntoElement + use<> {
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
                    .bg(theme::CONTROL)
                    .cursor_pointer().hover(|style| style.bg(theme::CONTROL_HOVER))
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
                            .bg(theme::DANGER)
                            .cursor_pointer().hover(|style| style.bg(theme::DANGER_HOVER))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_lg()
                            .text_color(theme::TEXT)
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
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .bg(theme::CONTROL)
            .cursor_pointer()
            .hover(|style| style.bg(theme::CONTROL_HOVER))
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
                    .bg(theme::DANGER)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::DANGER_HOVER))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(theme::TEXT)
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
    fn render_storage_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .px_4()
                    .py_2()
                    .bg(theme::SUCCESS)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::SUCCESS_HOVER))
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
                    .bg(theme::ACCENT)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::ACCENT_HOVER))
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

impl FrontendMessageHandler for NerveTracerView {
    type State = NerveTracerState;

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
            ToolState::NerveTracerState(s) => Some(s),
            _ => None,
        }
    }
}

impl Render for NerveTracerView {
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
                    .child("Error: Unable to read nerve tracer state");
            }
        };

        div()
            .track_focus(&self.base.focus_handle.clone().expect("initialized"))
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
                    move |this: &mut NerveTracerView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<NerveTracerView>| {
                        this.focus_handle(cx).focus(window, cx);
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

impl_tool_view_basics!(NerveTracerView, "Nerve Tracer");

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
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([
            KeyBinding::new("cmd-t", NerveTracerToggleActivity, None),
            KeyBinding::new("cmd-c", NerveTracerCommitSegments, None),
            KeyBinding::new("cmd-s", NerveTracerSaveTraces, None),
            KeyBinding::new("cmd-l", NerveTracerLoadTraces, None),
        ]);

        self.process_messages(window, cx);
    }
}
