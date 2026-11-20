use std::{ops::Range, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, Task, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use strawberry_backend::hyperstack::{HyperstackLayout, synthesize_parser_script};
use strawberry_types::{Dimension, ImageArrangement, Shape};

use super::{
    Button, ButtonVariant,
    code_editor::{CodeEdited, CodeEditor},
    generic_picker::picker_surface,
    theme,
};

/// Long enough to wait out a burst of typing before rerunning the script.
const RUN_DELAY: Duration = Duration::from_millis(250);
const ROW_HEIGHT: f32 = 24.0;
const DIMENSION_COLUMN_WIDTH: f32 = 36.0;

/// Emitted when the importer closes, carrying the arranged files unless it was cancelled.
pub enum ImporterClosed {
    Cancelled,
    Imported {
        files: Vec<PathBuf>,
        arrangement: ImageArrangement,
    },
}

impl EventEmitter<ImporterClosed> for HyperstackImporter {}

/// Overlay where a file-name parser script places images in a hyperstack, with a preview of
/// where each file lands.
pub struct HyperstackImporter {
    files: Vec<PathBuf>,
    file_names: Arc<[String]>,
    editor: Entity<CodeEditor>,
    /// The last script result: where each file lands, or why the script failed.
    result: Option<Result<HyperstackLayout, String>>,
    /// Script synthesis or a scheduled run; replacing it cancels the stale one.
    pending: Option<Task<()>>,
    list_scroll: UniformListScrollHandle,
}

impl HyperstackImporter {
    /// Create the importer for `files`, whose distinct names come from `collect_file_names`,
    /// and start generating a parser script for them.
    pub fn new(files: Vec<PathBuf>, file_names: Vec<String>, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| CodeEditor::new(String::new(), cx));
        cx.subscribe(&editor, |this, _, _: &CodeEdited, cx| this.schedule_run(cx))
            .detach();
        let mut importer = Self {
            files,
            file_names: file_names.into(),
            editor,
            result: None,
            pending: None,
            list_scroll: UniformListScrollHandle::new(),
        };
        importer.regenerate_script(cx);
        importer
    }

    /// Replace the script with one generated from the file names, as an undoable edit.
    fn regenerate_script(&mut self, cx: &mut Context<Self>) {
        let file_names = self.file_names.clone();
        let editor = self.editor.clone();
        self.pending = Some(cx.spawn(async move |_, cx| {
            let script = cx
                .background_spawn(async move { synthesize_parser_script(&file_names) })
                .await;
            editor.update(cx, |editor, cx| editor.set_text(&script, cx));
        }));
        cx.notify();
    }

    /// Schedule a script to run after some delay on code edit.
    fn schedule_run(&mut self, cx: &mut Context<Self>) {
        let script = self.editor.read(cx).text().to_owned();
        let file_names = self.file_names.clone();
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RUN_DELAY).await;
            let result = cx
                .background_spawn(async move {
                    HyperstackLayout::from_script(&script, &file_names)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.pending = None;
                this.result = Some(result);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Return the layout ready to import: that is, the latest result of the rhai script
    const fn get_ready_layout(&self) -> Option<&HyperstackLayout> {
        match &self.result {
            Some(Ok(layout)) if self.pending.is_none() => Some(layout),
            _ => None,
        }
    }

    /// Import the files the rhai script has categorized.
    fn send_import(&self, cx: &mut Context<Self>) {
        let Some(layout) = self.get_ready_layout() else {
            return;
        };
        let (files, arrangement) = layout.arrange(self.files.clone());
        cx.emit(ImporterClosed::Imported { files, arrangement });
    }

    /// Return the dimensions loaded into the previewr: id est, the dimensions that the preview has a column for.
    fn get_preview_dimensions(&self) -> Vec<Dimension> {
        let shape = match &self.result {
            Some(Ok(layout)) => layout.shape,
            _ => Shape::default(),
        };
        Dimension::ALL
            .into_iter()
            .filter(|&dimension| shape.extent(dimension) > 1)
            .collect()
    }

    /// Build one preview row from a file name and its cells, muted when the script skips it.
    fn render_preview_row(name: AnyElement, cells: Vec<String>, muted: bool) -> Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_3()
            .text_xs()
            .text_color(if muted {
                theme::TEXT_MUTED
            } else {
                theme::TEXT
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    // File names usually vary near their end, so keep both ends visible.
                    .text_ellipsis_middle()
                    .child(name),
            )
            .children(cells.into_iter().map(|cell| {
                div()
                    .flex_none()
                    .w(px(DIMENSION_COLUMN_WIDTH))
                    .text_right()
                    .child(cell)
            }))
    }

    /// Build the preview rows in `range`, numbering each file's place from one.
    fn render_preview_rows(&self, range: Range<usize>) -> Vec<Div> {
        let layout = match &self.result {
            Some(Ok(layout)) => Some(layout),
            _ => None,
        };
        let dimensions = self.get_preview_dimensions();
        range
            .filter_map(|row| {
                let name = self.file_names.get(row)?.clone();
                let coordinates =
                    layout.and_then(|layout| layout.coordinates.get(row).copied().flatten());
                let cells = dimensions
                    .iter()
                    .map(|&dimension| {
                        coordinates.map_or_else(String::new, |coordinates| {
                            coordinates.get(dimension).saturating_add(1).to_string()
                        })
                    })
                    .collect();
                let skipped = layout.is_some() && coordinates.is_none();
                Some(Self::render_preview_row(
                    name.into_any_element(),
                    cells,
                    skipped,
                ))
            })
            .collect()
    }

    /// Build the file preview: a header and a scrolling row per file.
    fn render_preview(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let header = Self::render_preview_row(
            "File".into_any_element(),
            self.get_preview_dimensions()
                .into_iter()
                .map(|dimension| dimension.label().to_owned())
                .collect(),
            true,
        )
        .border_b_1()
        .border_color(theme::BORDER_MUTED);
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(340.))
            .rounded_md()
            .border_1()
            .border_color(theme::BORDER_MUTED)
            .bg(theme::SURFACE_DEEP)
            .overflow_hidden()
            .child(header)
            .child(
                uniform_list(
                    "hyperstack-preview",
                    self.file_names.len(),
                    cx.processor(|this, range: Range<usize>, _window, _cx| {
                        this.render_preview_rows(range)
                    }),
                )
                .track_scroll(&self.list_scroll)
                .flex_1(),
            )
    }

    /// Build the status line: the hyperstack's shape and how many files it leaves out, or why
    /// the script failed.
    fn render_status(&self) -> impl IntoElement + use<> {
        let (text, color, left_out) = match &self.result {
            Some(Ok(layout)) => (
                layout.shape.to_string(),
                theme::TEXT_MUTED,
                layout
                    .coordinates
                    .len()
                    .saturating_sub(layout.count_placed_files()),
            ),
            Some(Err(error)) => (error.clone(), theme::DANGER, 0),
            None => (String::new(), theme::TEXT_MUTED, 0),
        };
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .text_xs()
            .whitespace_nowrap()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(color)
                    .child(text),
            )
            .when(left_out > 0, |status| {
                status.child(
                    div()
                        .flex_none()
                        .text_color(theme::WARNING)
                        .child(describe_left_out_files(left_out)),
                )
            })
    }
}

impl Focusable for HyperstackImporter {
    /// Return the script editor's focus handle, which receives typing while the importer is open.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl Render for HyperstackImporter {
    /// Build the script editor, file preview, status line, and buttons over the workspace.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let footer = div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.render_status())
            .child(
                Button::new("hyperstack-regenerate")
                    .label("Regenerate")
                    .on_click(cx.listener(|this, _, _, cx| this.regenerate_script(cx))),
            )
            .child(
                Button::new("hyperstack-cancel")
                    .label("Cancel")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ImporterClosed::Cancelled))),
            )
            .child(
                Button::new("hyperstack-import")
                    .variant(ButtonVariant::Primary)
                    .label("Import")
                    .disabled(self.get_ready_layout().is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.send_import(cx))),
            );
        let card = picker_surface()
            .id("hyperstack-importer-card")
            .key_context("HyperstackImporter")
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(ImporterClosed::Cancelled);
                    cx.stop_propagation();
                }
            }))
            .w(px(1000.))
            .h(px(620.))
            .max_h_full()
            .p_3()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .child(format!("Import {} Files as a Hyperstack", self.files.len())),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(self.editor.clone()))
                    .child(self.render_preview(cx)),
            )
            .child(footer);

        div()
            .id("hyperstack-importer")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(card)
    }
}

/// Describe how many files the script leaves out, such as "3 files left out".
fn describe_left_out_files(count: usize) -> String {
    if count == 1 {
        "1 file left out".to_owned()
    } else {
        format!("{count} files left out")
    }
}
