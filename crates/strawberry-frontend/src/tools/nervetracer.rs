use crate::gui::actions::actions;
use crate::gui::{Button, ButtonVariant};
use crate::gui::{theme, tool_panel};
use strawberry_types::{CommandSender, ToolId};
// Implements UI for nerve tracer tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div, prelude::*,
};
use rfd::FileDialog;
use strawberry_backend::nervetracer::io::read_tracings_file;

use crate::gui::{ToggleButton, prompts::ask_annotation_placement};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{NerveTracerMessage, NerveTracerState, PipelineObjectMessage, ToolState};

use strawberry_types::ToolFrontend;

pub struct NerveTracerView {
    pub state: Arc<RwLock<NerveTracerState>>,
    pub last_directory: Arc<RwLock<Option<PathBuf>>>,
    pub base: crate::tools::base::ToolBase,
    segments_scroll: gpui::ScrollHandle,
    paths_scroll: gpui::ScrollHandle,
}

impl Focusable for NerveTracerView {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl NerveTracerView {
    /// Create the nervetracer tool view and its initial controls.
    pub fn new(id: ToolId, command_sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(NerveTracerState::default())),
            segments_scroll: gpui::ScrollHandle::new(),
            paths_scroll: gpui::ScrollHandle::new(),
            last_directory: Arc::new(RwLock::new(None)),
            base: crate::tools::base::ToolBase::new(command_sender, id, cx.focus_handle()),
        }
    }

    /// Start receiving backend state updates for this tool view.
    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    /// Prompt for a path and request trace export.
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
                    if let Err(e) =
                        this.base
                            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                                NerveTracerMessage::SaveTraces(file),
                            ))
                    {
                        eprintln!("Failed to send save traces command: {e}");
                    }
                })
            {
                eprintln!("Failed to save traces: {e}");
            }
        })
        .detach();
    }

    /// Prompt for a traces file and request its import.
    fn load_traces(&self, window: &Window, cx: &Context<Self>) {
        let last_directory = self.last_directory.clone();
        let state = self.state.clone();

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

                let Ok((selected, shape)) = state
                    .read()
                    .map(|state| (state.traces.selected_position(), state.shape))
                else {
                    return;
                };
                // Unreadable files load as usual so the backend reports why they failed.
                let saved = read_tracings_file(&file)
                    .ok()
                    .and_then(|tracings| tracings.get_saved_positions(shape).ok())
                    .unwrap_or_default();
                let Some(placement) =
                    ask_annotation_placement(cx, "traces", &saved, selected, shape).await
                else {
                    return;
                };

                if let Err(e) = this.update(cx, |this, _cx| {
                    if let Err(e) =
                        this.base
                            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                                NerveTracerMessage::LoadTraces(file, placement),
                            ))
                    {
                        eprintln!("Failed to send load traces command: {e}");
                    }
                }) {
                    eprintln!("Failed to load traces: {e}");
                }
            }
        })
        .detach();
    }

    /// Commit the pending trace segments to a path.
    fn commit_segments(&self) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::CommitCurrentPathSegments(),
            ))
        {
            eprintln!("Failed to send commit segments command: {e}");
        }
    }

    /// Request removal of a pending trace segment.
    fn delete_uncommitted_segment(&self, segment_id: usize) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeleteUncomittedPathSegment(segment_id),
            ))
        {
            eprintln!("Failed to send delete uncommitted segment command: {e}");
        }
    }

    /// Request removal of a committed trace path.
    fn delete_path(&self, path_id: usize) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeletePath(path_id),
            ))
        {
            eprintln!("Failed to send delete path command: {e}");
        }
    }

    /// Request removal of a segment from a committed trace path.
    fn delete_path_segment(&self, path_id: usize, segment_id: usize) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::DeletePathSegmentInCommitedPath(path_id, segment_id),
            ))
        {
            eprintln!("Failed to send delete path segment command: {e}");
        }
    }

    /// Send the opposite enabled state to the backend.
    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.set_enabled(!current_value) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    /// Select the layer used for tracing.
    fn select_layer(&self, layer: usize) {
        if let Err(e) = self
            .base
            .send_to_backend(PipelineObjectMessage::NerveTracerMessage(
                NerveTracerMessage::SelectLayer(layer),
            ))
        {
            eprintln!("Failed to send select layer command: {e}");
        }
    }

    /// Handle the keyboard action to toggle activity.
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

    /// Handle the keyboard action to commit segments.
    fn commit_segments_action(
        &mut self,
        _: &NerveTracerCommitSegments,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_segments();
        cx.notify();
    }

    /// Handle the keyboard action to save traces.
    fn save_traces_action(
        &mut self,
        _: &NerveTracerSaveTraces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_traces(window, cx);
    }

    /// Handle the keyboard action to load traces.
    fn load_traces_action(
        &mut self,
        _: &NerveTracerLoadTraces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_traces(window, cx);
    }
    // Status component showing enabled/disabled state
    /// Build the status for the nervetracer view.
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

    /// Build the layer selector for the nervetracer view.
    fn render_layer_selector(
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        tool_panel::section("Input layer").child(div().flex().gap_1().flex_wrap().children(
            (0..state.shape.c).map(|i| {
                Button::new(("trace-layer", i))
                    .compact()
                    .selected(i == state.selected_layer)
                    .label(format!("{}", i.saturating_add(1)))
                    .on_click(cx.listener(move |this, _, _, _| this.select_layer(i)))
            }),
        ))
    }

    /// Build the segments section for the nervetracer view.
    fn render_segments_section(
        &self,
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut list = tool_panel::list_for_tool(
            "segments-list",
            &self.segments_scroll,
            &self.base.focus_handle,
        );
        if state.current_path_segments().is_empty() {
            list = list.child(tool_panel::empty("No segments"));
        }
        for (i, _) in state.current_path_segments().iter().enumerate() {
            list = list.child(
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
                            .tooltip_text("Delete segment")
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.delete_uncommitted_segment(i);
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
                .disabled(state.current_path_segments().is_empty())
                .tooltip_text("Commit segments to a path (⌘C)")
                .on_click(cx.listener(|this, _, _, _| this.commit_segments())),
        )
    }

    /// Build the paths section for the nervetracer view.
    fn render_paths_section(
        &self,
        state: &NerveTracerState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut list =
            tool_panel::list_for_tool("paths-list", &self.paths_scroll, &self.base.focus_handle);
        if state.paths().is_empty() {
            list = list.child(tool_panel::empty("No paths"));
        }
        for (path_id, path) in state.paths().iter().enumerate() {
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

    /// Build the storage buttons for the nervetracer view.
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

    /// Return the shared snapshot of this tool’s backend state.
    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    /// Return the shared tool connection and instance metadata.
    fn base(&self) -> &crate::tools::ToolBase {
        &self.base
    }

    /// Return mutable access to the tool connection and subscription state.
    fn base_mut(&mut self) -> &mut crate::tools::ToolBase {
        &mut self.base
    }

    /// Extract this tool’s settings from a backend state update.
    fn extract_state(tool_state: ToolState) -> Option<Self::State> {
        match tool_state {
            ToolState::NerveTracerState(s) => Some(s),
            _ => None,
        }
    }
}

impl Render for NerveTracerView {
    /// Build the nervetracer view and its interaction handlers.
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
                        this.base.select();
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .on_action(cx.listener(Self::commit_segments_action))
            .on_action(cx.listener(Self::save_traces_action))
            .on_action(cx.listener(Self::load_traces_action))
            .child(Self::render_status(&state, cx))
            .when(state.shape.c > 1, |el| {
                el.child(Self::render_layer_selector(&state, cx))
            })
            .child(self.render_segments_section(&state, cx))
            .child(self.render_paths_section(&state, cx))
    }
}

impl ToolFrontend for NerveTracerView {
    /// Connect the view’s subscriptions and event handlers.
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
