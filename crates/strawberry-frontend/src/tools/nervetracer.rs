use crate::gui::actions::actions;
use crate::gui::{Button, ButtonVariant};
use crate::gui::{theme, tool_panel};
// Implements UI for nerve tracer tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div, prelude::*,
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
    segments_scroll: gpui::ScrollHandle,
    paths_scroll: gpui::ScrollHandle,
}

impl Focusable for NerveTracerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl NerveTracerView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(NerveTracerState::default())),
            segments_scroll: gpui::ScrollHandle::new(),
            paths_scroll: gpui::ScrollHandle::new(),
            last_directory: Arc::new(RwLock::new(None)),
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::NerveTracer,
                cx.focus_handle(),
            ),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn save_traces(&self, window: &Window, cx: &Context<Self>) {
        let last_directory = self.last_directory.clone();

        cx.spawn_in(window, async move |this, cx| {
            let default_dir = last_directory
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~/".to_string());

            let file = FileDialog::new()
                .add_filter("traces", &["traces", "xml"])
                .set_directory(&default_dir)
                .save_file();

            if let Some(file) = file
                && let Err(e) = this.update(cx, |this, _cx| {
                    if let Err(e) = this.base.send_to_backend(PipelineCommand::PipelineObject(
                        PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::SaveTraces(
                            file,
                        )),
                    )) {
                        eprintln!("Failed to send save traces command: {e}");
                    }
                })
            {
                eprintln!("Failed to save traces: {e}");
            }
        })
        .detach();
    }

    fn load_traces(&self, window: &Window, cx: &Context<Self>) {
        let last_directory = self.last_directory.clone();

        cx.spawn_in(window, async move |this, cx| {
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
                if let Some(parent) = file.parent()
                    && let Ok(mut dir) = last_directory.write()
                {
                    *dir = Some(parent.to_path_buf());
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

    fn commit_segments(&self) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::CommitCurrentPathSegments(),
            ),
        )) {
            eprintln!("Failed to send commit segments command: {e}");
        }
    }

    fn delete_uncommitted_segment(&self, segment_id: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeleteUncomittedPathSegment(segment_id),
            ),
        )) {
            eprintln!("Failed to send delete uncommitted segment command: {e}");
        }
    }

    fn delete_path(&self, path_id: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(NerveTracerMessage::DeletePath(path_id)),
        )) {
            eprintln!("Failed to send delete path command: {e}");
        }
    }

    fn delete_path_segment(&self, path_id: usize, segment_id: usize) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeletePathSegmentInCommitedPath(path_id, segment_id),
            ),
        )) {
            eprintln!("Failed to send delete path segment command: {e}");
        }
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    fn set_selected_image(&self, image_index: usize) {
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    fn render_status(state: &NerveTracerState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .with_no_margin()
        .render(cx)
    }

    fn render_image_selector(
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        tool_panel::section("Input layer").child(div().flex().gap_1().flex_wrap().children(
            (0..state.total_images).map(|i| {
                Button::new(("trace-image", i))
                    .compact()
                    .selected(i == state.selected_image_index)
                    .label(format!("{}", i.saturating_add(1)))
                    .on_click(cx.listener(move |this, _, _, _| this.set_selected_image(i)))
            }),
        ))
    }

    fn render_segments_section(
        &self,
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut list = tool_panel::list("segments-list", &self.segments_scroll);
        if state.current_path_segments.is_empty() {
            list = list.child(tool_panel::empty("No segments"));
        }
        for (i, _) in state.current_path_segments.iter().enumerate() {
            list =
                list.child(
                    tool_panel::row()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .child(format!("Segment {}", i.saturating_add(1))),
                        )
                        .child(
                            Button::new(("delete-segment", i))
                                .variant(ButtonVariant::Delete)
                                .aria_label("Delete segment")
                                .tooltip_text("Delete segment")
                                .on_click(cx.listener(move |this, _, _, _| {
                                    this.delete_uncommitted_segment(i)
                                })),
                        ),
                );
        }
        tool_panel::section("Current path").child(list).child(
            Button::new("commit-segments")
                .variant(ButtonVariant::Success)
                .large()
                .w_full()
                .label("Commit path")
                .disabled(state.current_path_segments.is_empty())
                .tooltip_text("Commit segments to a path (⌘C)")
                .on_click(cx.listener(|this, _, _, _| this.commit_segments())),
        )
    }

    fn render_paths_section(
        &self,
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut list = tool_panel::list("paths-list", &self.paths_scroll);
        if state.paths.is_empty() {
            list = list.child(tool_panel::empty("No paths"));
        }
        for (path_id, path) in state.paths.iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::BORDER_MUTED)
                    .bg(theme::SURFACE_ALT)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("Path {}", path_id.saturating_add(1))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{} segments", path.len())),
                    )
                    .child(
                        Button::new(("delete-path", path_id))
                            .variant(ButtonVariant::Delete)
                            .aria_label("Delete path")
                            .tooltip_text("Delete path")
                            .on_click(cx.listener(move |this, _, _, _| this.delete_path(path_id))),
                    ),
            );
            for (segment_id, _) in path.iter().enumerate() {
                list = list.child(
                    tool_panel::row()
                        .pl_4()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .child(format!("Segment {}", segment_id.saturating_add(1))),
                        )
                        .child(
                            Button::new(gpui::ElementId::Name(
                                format!("delete-path-{path_id}-segment-{segment_id}").into(),
                            ))
                            .variant(ButtonVariant::Delete)
                            .aria_label("Delete path segment")
                            .tooltip_text("Delete path segment")
                            .on_click(cx.listener(
                                move |this, _, _, _| this.delete_path_segment(path_id, segment_id),
                            )),
                        ),
                );
            }
        }
        tool_panel::section("Paths")
            .child(list)
            .child(Self::render_storage_buttons(cx))
    }

    fn render_storage_buttons(cx: &Context<Self>) -> impl IntoElement + use<> {
        tool_panel::file_actions()
            .child(
                Button::new("trace-load")
                    .compact()
                    .flex_1()
                    .min_w_0()
                    .label("Load traces")
                    .tooltip_text("⌘L")
                    .on_click(cx.listener(|this, _, window, cx| this.load_traces(window, cx))),
            )
            .child(
                Button::new("trace-save")
                    .compact()
                    .flex_1()
                    .min_w_0()
                    .label("Save traces")
                    .tooltip_text("⌘S")
                    .on_click(cx.listener(|this, _, window, cx| this.save_traces(window, cx))),
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
                    .w_full()
                    .text_sm()
                    .p_4()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read nerve tracer state");
            }
        };

        tool_panel::panel()
            .key_context("NerveTracerView")
            .track_focus(&self.focus_handle(cx))
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .on_action(cx.listener(Self::commit_segments_action))
            .on_action(cx.listener(Self::save_traces_action))
            .on_action(cx.listener(Self::load_traces_action))
            .child(Self::render_status(&state, cx))
            .when(state.total_images > 1, |el| {
                el.child(Self::render_image_selector(&state, cx))
            })
            .child(self.render_segments_section(&state, cx))
            .child(self.render_paths_section(&state, cx))
    }
}

impl ToolFrontend for NerveTracerView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([
            KeyBinding::new("cmd-t", NerveTracerToggleActivity, Some("NerveTracerView")),
            KeyBinding::new("cmd-c", NerveTracerCommitSegments, Some("NerveTracerView")),
            KeyBinding::new("cmd-s", NerveTracerSaveTraces, Some("NerveTracerView")),
            KeyBinding::new("cmd-l", NerveTracerLoadTraces, Some("NerveTracerView")),
        ]);

        self.process_messages(window, cx);
    }
}
