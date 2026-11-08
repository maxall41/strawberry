use crate::gui::actions::actions;
use crate::gui::theme;
use crate::gui::{Button, ButtonVariant};
// Implements UI for nerve tracer tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div, prelude::*, px,
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
        .render(cx)
    }

    // Image selector component
    fn render_image_selector(
        state: &NerveTracerState,
        cx: &Context<Self>,
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
                        Button::new(("trace-image", i))
                            .selected(is_selected)
                            .label(format!("{}", i.saturating_add(1)))
                            .on_click(cx.listener(
                                move |this: &mut Self,
                                      _event: &gpui::ClickEvent,
                                      _window: &mut Window,
                                      _cx: &mut Context<Self>| {
                                    this.set_selected_image(i);
                                },
                            ))
                    })),
            )
    }

    // Segments section
    fn render_segments_section(
        state: &NerveTracerState,
        cx: &Context<Self>,
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
                        el.children(state.current_path_segments.iter().enumerate().map(
                            |(idx, segment)| Self::render_segment_item(idx, segment.len(), cx),
                        ))
                    }),
            )
    }

    // Individual segment item
    fn render_segment_item(
        segment_id: usize,
        point_count: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .bg(theme::CONTROL)
            .rounded_md()
            .child(div().flex_1().child(format!(
                "Segment {:03} ({} points)",
                segment_id.saturating_add(1),
                point_count
            )))
            .child(
                Button::new(("delete-segment", segment_id))
                    .variant(ButtonVariant::Delete)
                    .aria_label("Delete segment")
                    .on_click(cx.listener(
                        move |this: &mut Self,
                              _event: &gpui::ClickEvent,
                              _window: &mut Window,
                              _cx: &mut Context<Self>| {
                            this.delete_uncommitted_segment(segment_id);
                        },
                    )),
            )
    }

    // Add to path button
    fn render_add_to_path_button(
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        Button::new("commit-segments")
            .variant(ButtonVariant::Primary)
            .large()
            .w_full()
            .my_2()
            .label("Commit")
            .disabled(state.current_path_segments.is_empty())
            .tooltip_text("Commit segments to a path (⌘C)")
            .on_click(cx.listener(|this, _, _, _| this.commit_segments()))
    }

    // Paths section
    fn render_paths_section(
        state: &NerveTracerState,
        cx: &Context<Self>,
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
                                .map(|(path_idx, path)| Self::render_path_item(path_idx, path, cx)),
                        )
                    }),
            )
    }

    // Individual path item (tree parent)
    fn render_path_item(
        path_id: usize,
        path: &[Vec<(u64, u64)>],
        cx: &Context<Self>,
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
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(format!("Path {:03}", path_id.saturating_add(1))),
                    )
                    .child(
                        Button::new(("delete-path", path_id))
                            .variant(ButtonVariant::Delete)
                            .aria_label("Delete path")
                            .on_click(cx.listener(
                                move |this: &mut Self,
                                      _event: &gpui::ClickEvent,
                                      _window: &mut Window,
                                      _cx: &mut Context<Self>| {
                                    this.delete_path(path_id);
                                },
                            )),
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
                        Self::render_path_segment_item(path_id, segment_idx, segment.len(), cx)
                    })),
            )
    }

    // Individual segment within a path (tree child)
    fn render_path_segment_item(
        path_id: usize,
        segment_id: usize,
        point_count: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .items_center()
            .justify_between()
            .p_2()
            .bg(theme::CONTROL)
            .rounded_md()
            .child(div().flex_1().child(format!(
                "Segment {:03} ({} points)",
                segment_id.saturating_add(1),
                point_count
            )))
            .child(
                Button::new(gpui::ElementId::Name(
                    format!("delete-path-{path_id}-segment-{segment_id}").into(),
                ))
                .variant(ButtonVariant::Delete)
                .aria_label("Delete path segment")
                .on_click(cx.listener(
                    move |this: &mut Self,
                          _event: &gpui::ClickEvent,
                          _window: &mut Window,
                          _cx: &mut Context<Self>| {
                        this.delete_path_segment(path_id, segment_id);
                    },
                )),
            )
    }

    // Storage buttons (Save/Load)
    fn render_storage_buttons(cx: &Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .gap_2()
            .child(
                Button::new("save-button")
                    .variant(ButtonVariant::Success)
                    .large()
                    .flex_1()
                    .label("Save")
                    .tooltip_text("⌘S")
                    .on_click(cx.listener(
                        |this: &mut Self,
                         _event: &gpui::ClickEvent,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
                            this.save_traces(window, cx);
                        },
                    )),
            )
            .child(
                Button::new("load-button")
                    .variant(ButtonVariant::Primary)
                    .large()
                    .flex_1()
                    .label("Load")
                    .tooltip_text("⌘L")
                    .on_click(cx.listener(
                        |this: &mut Self,
                         _event: &gpui::ClickEvent,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
                            this.load_traces(window, cx);
                        },
                    )),
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

        div()
            .key_context("NerveTracerView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .w_full()
            .text_sm()
            .p_4()
            .text_color(theme::TEXT)
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
            .child(Self::render_image_selector(&state, cx))
            .child(Self::render_segments_section(&state, cx))
            .child(Self::render_add_to_path_button(&state, cx))
            .child(Self::render_paths_section(&state, cx))
            .child(div().mt_auto().child(Self::render_storage_buttons(cx)))
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
