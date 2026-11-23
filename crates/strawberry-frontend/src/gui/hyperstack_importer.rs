use std::{ops::Range, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use gpui::{
    App, AppContext, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, Pixels, Rgba, SharedString, Task, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};
use num_traits::ToPrimitive;
use strawberry_backend::hyperstack::{
    HyperstackLayout, ParserTemplate, TemplateField, TemplatePart, synthesize_parser_template,
};
use strawberry_types::{Dimension, ImageArrangement, Shape};

use super::{
    Button, ButtonVariant,
    code_editor::{CodeEdited, CodeEditor, MONOSPACE_FONT},
    dropdown::Dropdown,
    generic_picker::picker_surface,
    icon::Icon,
    theme,
};

/// Long enough to wait out a burst of typing before rerunning the script.
const RUN_DELAY: Duration = Duration::from_millis(250);
const ROW_HEIGHT: f32 = 24.0;
const FIELD_ROW_HEIGHT: f32 = 44.0;
const CELL_GAP: f32 = 8.0;
const COORDINATE_COLUMN_WIDTH: f32 = 36.0;
const PART_COLUMN_WIDTH: f32 = 80.0;
const DIMENSION_COLUMN_WIDTH: f32 = 176.0;
const PREVIEW_WIDTH: f32 = 380.0;
// Tight line heights keep field chips snug around their text, like inline code spans.
const PATTERN_LINE_HEIGHT: f32 = 18.0;
const PART_CHIP_LINE_HEIGHT: f32 = 14.0;
const CHIP_FILL: Rgba = Rgba {
    a: 0.16,
    ..theme::ACCENT
};
const CHIP_BORDER: Rgba = Rgba {
    a: 0.5,
    ..theme::ACCENT
};
const TAG_FILL: Rgba = Rgba {
    a: 0.3,
    ..theme::ACCENT
};

/// Emitted when the importer closes, carrying the arranged files unless it was cancelled.
pub enum ImporterClosed {
    Cancelled,
    Imported {
        files: Vec<PathBuf>,
        arrangement: ImageArrangement,
    },
}

impl EventEmitter<ImporterClosed> for HyperstackImporter {}

/// The parser template synthesized from the file names, and the user's changes to it.
struct Templates {
    /// The template as synthesized, which Regenerate restores.
    detected: ParserTemplate,
    /// The detected template with the dimensions the user chose for its fields.
    current: ParserTemplate,
    /// `current` rendered as a script; editor text that differs from it was edited by hand.
    script: String,
}

/// Overlay that places images in a hyperstack by the parts of their file names, chosen in a
/// guided field list or written as a parser script, with a preview of where each file lands.
pub struct HyperstackImporter {
    files: Vec<PathBuf>,
    file_names: Arc<[String]>,
    focus_handle: FocusHandle,
    editor: Entity<CodeEditor>,
    /// `None` until synthesis finishes.
    templates: Option<Templates>,
    /// Whether the script editor replaces the guided fields.
    show_script: bool,
    /// The template part whose dimension menu is open.
    open_dropdown: Option<usize>,
    /// The last script result: where each file lands, or why the script failed.
    result: Option<Result<HyperstackLayout, String>>,
    /// Template synthesis or a scheduled run; replacing it cancels the stale one.
    pending: Option<Task<()>>,
    list_scroll: UniformListScrollHandle,
}

impl HyperstackImporter {
    /// Create the importer for `files`, whose distinct names come from `collect_file_names`,
    /// and start synthesizing a parser template for them.
    pub fn new(files: Vec<PathBuf>, file_names: Vec<String>, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| CodeEditor::new(String::new(), cx));
        cx.subscribe(&editor, |this, _, _: &CodeEdited, cx| this.schedule_run(cx))
            .detach();
        let mut importer = Self {
            files,
            file_names: file_names.into(),
            focus_handle: cx.focus_handle(),
            editor,
            templates: None,
            show_script: false,
            open_dropdown: None,
            result: None,
            pending: None,
            list_scroll: UniformListScrollHandle::new(),
        };
        importer.start_template_synthesis(cx);
        importer
    }

    /// Synthesize a parser template from the file names in the background, then write its
    /// script into the editor.
    fn start_template_synthesis(&mut self, cx: &Context<Self>) {
        let file_names = self.file_names.clone();
        self.pending = Some(cx.spawn(async move |this, cx| {
            let detected = cx
                .background_spawn(async move { synthesize_parser_template(&file_names) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.templates = Some(Templates {
                    current: detected.clone(),
                    detected,
                    script: String::new(),
                });
                this.write_template_script(cx);
            });
        }));
    }

    /// Render the current template into the editor as an undoable edit, which reruns the
    /// layout.
    fn write_template_script(&mut self, cx: &mut Context<Self>) {
        let Some(templates) = &mut self.templates else {
            return;
        };
        templates.script = templates.current.render_script();
        self.editor
            .update(cx, |editor, cx| editor.set_text(&templates.script, cx));
        cx.notify();
    }

    /// Place files along `dimension` by the template field at `part`, or ignore the field
    /// when `dimension` is `None`.
    fn set_field_dimension(
        &mut self,
        part: usize,
        dimension: Option<Dimension>,
        cx: &mut Context<Self>,
    ) {
        if let Some(templates) = &mut self.templates {
            templates.current.set_field_dimension(part, dimension);
            self.write_template_script(cx);
        }
    }

    /// Undo the user's dimension choices and script edits by restoring the detected template.
    fn restore_detected_template(&mut self, cx: &mut Context<Self>) {
        if let Some(templates) = &mut self.templates {
            templates.current = templates.detected.clone();
            self.write_template_script(cx);
        }
    }

    /// Return whether the editor's text was edited by hand, so the guided fields no longer
    /// describe it.
    fn is_script_edited(&self, cx: &App) -> bool {
        self.templates
            .as_ref()
            .is_some_and(|templates| self.editor.read(cx).text() != templates.script)
    }

    /// Return whether the placement differs from the detected template, through either
    /// dimension choices or script edits.
    fn is_template_changed(&self, cx: &App) -> bool {
        self.templates
            .as_ref()
            .is_some_and(|templates| templates.current != templates.detected)
            || self.is_script_edited(cx)
    }

    /// Show the script editor in place of the guided fields, or return to the fields, moving
    /// focus so typing reaches the editor and Escape still reaches the importer.
    fn set_script_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.show_script = visible;
        self.open_dropdown = None;
        let focus = if visible {
            self.editor.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        };
        window.focus(&focus, cx);
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

    /// Return the latest script result's layout, even while a newer run is pending.
    fn get_layout(&self) -> Option<&HyperstackLayout> {
        self.result.as_ref().and_then(|result| result.as_ref().ok())
    }

    /// Return the layout ready to import: the latest script result, once no run is pending.
    fn get_ready_layout(&self) -> Option<&HyperstackLayout> {
        self.get_layout().filter(|_| self.pending.is_none())
    }

    /// Import the files the rhai script has categorized.
    fn send_import(&self, cx: &mut Context<Self>) {
        let Some(layout) = self.get_ready_layout() else {
            return;
        };
        let (files, arrangement) = layout.arrange(self.files.clone());
        cx.emit(ImporterClosed::Imported { files, arrangement });
    }

    /// Return the dimensions the preview has a column for: those the layout spans.
    fn get_preview_dimensions(&self) -> Vec<Dimension> {
        let shape = self
            .get_layout()
            .map_or_else(Shape::default, |layout| layout.shape);
        Dimension::ALL
            .into_iter()
            .filter(|&dimension| shape.get_extent(dimension) > 1)
            .collect()
    }

    /// Build the preview rows in `range`, numbering each file's place from one and marking
    /// the files the script leaves out.
    fn render_preview_rows(&self, range: Range<usize>) -> Vec<Div> {
        let layout = self.get_layout();
        let dimensions = self.get_preview_dimensions();
        range
            .filter_map(|row| {
                let name = build_fill_cell()
                    // File names usually vary near their end, so keep both ends visible.
                    .text_ellipsis_middle()
                    .child(self.file_names.get(row)?.clone());
                let element = build_table_row(ROW_HEIGHT).child(name);
                Some(
                    match layout.and_then(|layout| layout.coordinates.get(row).copied()) {
                        Some(Some(coordinates)) => {
                            element.children(dimensions.iter().map(|&dimension| {
                                build_coordinate_cell(
                                    coordinates.get(dimension).saturating_add(1).to_string(),
                                )
                            }))
                        }
                        Some(None) => element.text_color(theme::TEXT_MUTED).child(
                            div()
                                .flex_none()
                                .min_w(get_coordinate_columns_width(dimensions.len()))
                                .text_right()
                                .text_color(theme::WARNING)
                                .child("Left out"),
                        ),
                        None => element,
                    },
                )
            })
            .collect()
    }

    /// Build the file preview: a header and a scrolling row per file.
    fn render_preview(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let header = build_table_header()
            .child(build_fill_cell().child("File"))
            .children(
                self.get_preview_dimensions()
                    .into_iter()
                    .map(|dimension| build_coordinate_cell(dimension.get_label())),
            );
        build_well()
            .flex_none()
            .w(px(PREVIEW_WIDTH))
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

    /// Build the guided panel: the example file name with its fields marked, and a row per
    /// field choosing its dimension. Shows a notice instead when the script was edited by hand.
    fn render_guided_panel(&self, cx: &Context<Self>) -> Div {
        let panel = build_well().flex_1().min_w_0();
        let Some(templates) = &self.templates else {
            return panel;
        };
        if self.is_script_edited(cx) {
            return panel
                .items_center()
                .justify_center()
                .gap_3()
                .child(Icon::code().size(28.).color(theme::TEXT_MUTED))
                .child(div().text_sm().child("Placed by an edited script"))
                .child(
                    Button::new("hyperstack-open-script")
                        .compact()
                        .label("Open Script")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_script_visible(true, window, cx);
                        })),
                );
        }
        let options = get_dimension_options();
        let header = build_table_header()
            .child(build_cell(PART_COLUMN_WIDTH).child("Part"))
            .child(build_fill_cell().child("Values"))
            .child(build_cell(DIMENSION_COLUMN_WIDTH).child("Dimension"));
        let rows =
            templates.current.get_parts().iter().enumerate().filter_map(
                |(part, kind)| match kind {
                    TemplatePart::Field(field) => {
                        Some(self.render_field_row(part, field, options.clone(), cx))
                    }
                    TemplatePart::Literal(_) => None,
                },
            );
        panel
            .child(render_name_pattern(templates.current.get_parts()))
            .child(header)
            .child(
                div()
                    .id("hyperstack-fields")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }

    /// Build the row for the template field at `part`: its sample text, a summary of its
    /// values, and the menu choosing its dimension.
    fn render_field_row(
        &self,
        part: usize,
        field: &TemplateField,
        options: Vec<SharedString>,
        cx: &Context<Self>,
    ) -> Div {
        let selected = field
            .dimension
            .and_then(|dimension| Dimension::ALL.iter().position(|&other| other == dimension))
            .unwrap_or(Dimension::ALL.len());
        let chip = build_field_chip(field.dimension.is_some())
            // A flex chip would not narrow its text, which could then never be ellipsized.
            .block()
            .min_w_0()
            .px(px(6.))
            .py(px(2.))
            .font_family(MONOSPACE_FONT)
            .line_height(px(PART_CHIP_LINE_HEIGHT))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(field.sample.clone());
        let dropdown = Dropdown::new(("hyperstack-field-dimension", part), options, selected)
            .flex_none()
            .w(px(DIMENSION_COLUMN_WIDTH))
            .aria_label(format!("Dimension for {}", field.sample))
            .open(self.open_dropdown == Some(part))
            .on_toggle(cx.listener(move |this, open: &bool, _, cx| {
                this.open_dropdown = open.then_some(part);
                cx.notify();
            }))
            .on_select(cx.listener(move |this, option: &usize, _, cx| {
                this.set_field_dimension(part, Dimension::ALL.get(*option).copied(), cx);
            }));
        build_table_row(FIELD_ROW_HEIGHT)
            .border_b_1()
            .border_color(theme::BORDER_MUTED)
            .child(build_cell(PART_COLUMN_WIDTH).flex().child(chip))
            .child(
                build_fill_cell()
                    .text_ellipsis()
                    .when(field.dimension.is_none(), |cell| {
                        cell.text_color(theme::TEXT_MUTED)
                    })
                    .child(summarize_values(&field.values)),
            )
            .child(dropdown)
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
            Some(Err(error)) => (error.clone(), theme::DANGER_TEXT, 0),
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
    /// Return the importer's focus handle, which receives Escape while the guided fields show.
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HyperstackImporter {
    /// Build the guided fields or script editor, file preview, status line, and buttons over
    /// the workspace.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .child(format!("Import {} Files as a Hyperstack", self.files.len())),
            )
            .child(
                Button::new("hyperstack-script")
                    .compact()
                    .variant(ButtonVariant::Ghost)
                    .selected(self.show_script)
                    .gap_1p5()
                    .child(Icon::code())
                    .label("Script")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_script_visible(!this.show_script, window, cx);
                    })),
            );
        let fields = if self.show_script {
            div().flex_1().min_w_0().child(self.editor.clone())
        } else {
            self.render_guided_panel(cx)
        };
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(self.render_status())
            .when(self.is_template_changed(cx), |footer| {
                footer.child(
                    Button::new("hyperstack-regenerate")
                        .label("Regenerate")
                        .on_click(cx.listener(|this, _, _, cx| this.restore_detected_template(cx))),
                )
            })
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
            .track_focus(&self.focus_handle)
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
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .child(fields)
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

/// Build the inset panel that holds the guided fields or the file preview.
fn build_well() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(theme::BORDER_MUTED)
        .bg(theme::SURFACE_DEEP)
        .overflow_hidden()
}

/// Build a table row whose cells are spaced to line up with the table's header.
fn build_table_row(height: f32) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(CELL_GAP))
        .w_full()
        .h(px(height))
        .px_3()
        .text_xs()
}

/// Build a table header: column names in muted text above a divider.
fn build_table_header() -> Div {
    build_table_row(ROW_HEIGHT)
        .text_color(theme::TEXT_MUTED)
        .border_b_1()
        .border_color(theme::BORDER_MUTED)
}

/// Build a table cell of fixed width.
fn build_cell(width: f32) -> Div {
    div().flex_none().w(px(width))
}

/// Build a table cell that takes the row's remaining width, clipping its text to one line.
fn build_fill_cell() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
}

/// Build a preview cell holding one coordinate or a dimension's letter.
fn build_coordinate_cell(text: impl IntoElement) -> Div {
    build_cell(COORDINATE_COLUMN_WIDTH).text_right().child(text)
}

/// Return the width `count` coordinate columns span together, including the gaps between them.
fn get_coordinate_columns_width(count: usize) -> Pixels {
    let columns = count.to_f32().unwrap_or_default();
    let gaps = count.saturating_sub(1).to_f32().unwrap_or_default();
    px(columns.mul_add(COORDINATE_COLUMN_WIDTH, gaps * CELL_GAP))
}

/// Build the outline of a field chip: tinted when the field places files along a dimension,
/// and dashed and muted when it is ignored.
fn build_field_chip(assigned: bool) -> Div {
    let chip = div().flex().items_center().rounded(px(4.)).border_1();
    if assigned {
        chip.bg(CHIP_FILL)
            .border_color(CHIP_BORDER)
            .text_color(theme::TEXT)
    } else {
        chip.border_dashed()
            .border_color(theme::BORDER)
            .text_color(theme::TEXT_MUTED)
    }
}

/// Build the example file name with each field shown as a chip tagged with its dimension.
fn render_name_pattern(parts: &[TemplatePart]) -> impl IntoElement + use<> {
    let tag_text = theme::lighten(theme::ACCENT, 0.45);
    div()
        .flex()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(px(2.))
        .px(px(16.))
        .py(px(20.))
        .border_b_1()
        .border_color(theme::BORDER_MUTED)
        .text_size(px(15.))
        .line_height(px(PATTERN_LINE_HEIGHT))
        .children(parts.iter().map(|part| {
            match part {
                TemplatePart::Literal(text) => div()
                    .font_family(MONOSPACE_FONT)
                    .text_color(theme::TEXT_MUTED)
                    .child(text.clone()),
                TemplatePart::Field(field) => {
                    let chip = build_field_chip(field.dimension.is_some())
                        .gap_1p5()
                        .py(px(3.))
                        .pr(px(8.));
                    let sample = div()
                        .font_family(MONOSPACE_FONT)
                        .child(field.sample.clone());
                    match field.dimension {
                        Some(dimension) => chip.pl(px(3.)).child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .size(px(18.))
                                .rounded(px(3.))
                                .bg(TAG_FILL)
                                .text_color(tag_text)
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(dimension.get_label()),
                        ),
                        None => chip.pl(px(8.)),
                    }
                    .child(sample)
                }
            }
        }))
}

/// Return the dimension menu's options: each dimension, such as "C · Channel", then Ignore.
fn get_dimension_options() -> Vec<SharedString> {
    Dimension::ALL
        .into_iter()
        .map(|dimension| format!("{} · {}", dimension.get_label(), dimension.get_name()).into())
        .chain(["Ignore".into()])
        .collect()
}

/// Summarize a field's values: the range of numbers, such as "001 – 010", or else every value.
fn summarize_values(values: &[String]) -> String {
    let numeric = values
        .iter()
        .all(|value| value.bytes().all(|byte| byte.is_ascii_digit()));
    match values {
        [first, .., last] if numeric => format!("{first} – {last}"),
        _ => values.join(" · "),
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
