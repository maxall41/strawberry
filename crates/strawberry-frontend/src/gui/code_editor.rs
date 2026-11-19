use std::{cell::Cell, ops::Range, rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, Div, EventEmitter, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Rgba,
    ScrollHandle, ShapedLine, TextAlign, TextRun, Window, canvas, div, font, point, prelude::*,
    pulsating_between, px,
};
use num_traits::ToPrimitive;

use super::{text_buffer::TextBuffer, theme};

super::actions::actions!([
    EditorSelectAll,
    EditorCopy,
    EditorCut,
    EditorPaste,
    EditorUndo,
    EditorRedo,
]);

/// Key context that editor shortcuts are bound in.
pub const KEY_CONTEXT: &str = "CodeEditor";
const FONT_FAMILY: &str = "Menlo";
const FONT_SIZE: f32 = 12.0;
const LINE_HEIGHT: f32 = 18.0;
const GUTTER_WIDTH: f32 = 36.0;
const TEXT_PADDING: f32 = 8.0;
const INDENT: &str = "    ";
/// Enough history to back out of a long editing session without keeping every keystroke forever.
const UNDO_LIMIT: usize = 500;

/// Emitted after the editor's text changes.
pub struct CodeEdited;

impl EventEmitter<CodeEdited> for CodeEditor {}

/// Text and cursor restored by undo and redo.
struct Snapshot {
    text: String,
    cursor: usize,
}

/// Return whether a character belongs to a word for word-wise movement and selection.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// A multi-line monospace editor for scripts.
pub struct CodeEditor {
    focus_handle: FocusHandle,
    buffer: TextBuffer,
    /// Where the selection started; the selection spans from here to the cursor.
    anchor: Option<usize>,
    /// Horizontal position kept while moving between lines of different lengths.
    goal_x: Option<Pixels>,
    selecting: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Lines shaped by the last render, which place the caret and resolve clicks.
    lines: Vec<ShapedLine>,
    /// Window position of the first line's text, captured while painting.
    text_origin: Rc<Cell<Point<Pixels>>>,
    scroll: ScrollHandle,
}

impl CodeEditor {
    /// Create an editor holding `text`.
    pub fn new(text: String, cx: &Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            buffer: TextBuffer::new(text),
            anchor: None,
            goal_x: None,
            selecting: false,
            undo: Vec::new(),
            redo: Vec::new(),
            lines: Vec::new(),
            text_origin: Rc::default(),
            scroll: ScrollHandle::new(),
        }
    }

    /// Return the editor's text.
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replace the whole text as one undoable edit and move the cursor to the start.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.edit(0..self.buffer.text().len(), text, cx);
        self.buffer.move_cursor(0);
        self.reveal_cursor();
    }

    /// Return the selected byte range, if any text is selected.
    fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        let cursor = self.buffer.cursor();
        (anchor != cursor).then(|| anchor.min(cursor)..anchor.max(cursor))
    }

    /// Replace a range as one undoable edit and report the change.
    fn edit(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.record_undo();
        self.buffer.replace_range(range, text);
        self.anchor = None;
        self.goal_x = None;
        self.reveal_cursor();
        cx.emit(CodeEdited);
        cx.notify();
    }

    /// Replace the selection, or insert at the cursor, with `text`.
    fn replace_selection(&mut self, text: &str, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        let range = self.selection().unwrap_or(cursor..cursor);
        self.edit(range, text, cx);
    }

    /// Delete the selection, or the text between the cursor and `offset`.
    fn delete_toward(&mut self, offset: usize, cx: &mut Context<Self>) {
        let cursor = self.buffer.cursor();
        let range = self
            .selection()
            .unwrap_or_else(|| cursor.min(offset)..cursor.max(offset));
        if !range.is_empty() {
            self.edit(range, "", cx);
        }
    }

    /// Capture the current text and cursor.
    fn take_snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.buffer.text().to_owned(),
            cursor: self.buffer.cursor(),
        }
    }

    /// Remember the text before an edit so it can be undone.
    fn record_undo(&mut self) {
        if self.undo.len() >= UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(self.take_snapshot());
        self.redo.clear();
    }

    /// Replace the text and cursor with a snapshot, clearing the selection.
    fn restore_snapshot(&mut self, snapshot: Snapshot) {
        self.buffer = TextBuffer::new(snapshot.text);
        self.buffer.move_cursor(snapshot.cursor);
        self.anchor = None;
        self.goal_x = None;
        self.reveal_cursor();
    }

    /// Move the cursor, extending the selection when `extend` holds.
    fn move_to(&mut self, offset: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert_with(|| self.buffer.cursor());
        } else {
            self.anchor = None;
        }
        self.buffer.move_cursor(offset);
    }

    /// Return the offset where the line containing `offset` starts.
    fn find_line_start(&self, offset: usize) -> usize {
        self.buffer
            .text()
            .get(..offset)
            .and_then(|text| text.rfind('\n'))
            .map_or(0, |newline| newline.saturating_add(1))
    }

    /// Return the offset where the line containing `offset` ends, before its newline.
    fn find_line_end(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        text.get(offset..)
            .and_then(|rest| rest.find('\n'))
            .map_or(text.len(), |newline| offset.saturating_add(newline))
    }

    /// Return the zero-based line containing `offset`.
    fn find_line_index(&self, offset: usize) -> usize {
        self.buffer
            .text()
            .get(..offset)
            .map_or(0, |text| text.matches('\n').count())
    }

    /// Return the offset where a zero-based line starts, or the end of the text past the last line.
    fn find_offset_of_line(&self, line: usize) -> usize {
        let text = self.buffer.text();
        if line == 0 {
            return 0;
        }
        text.match_indices('\n')
            .nth(line.saturating_sub(1))
            .map_or(text.len(), |(newline, _)| newline.saturating_add(1))
    }

    /// Return the start of the word before `offset`, skipping the gap before it.
    fn find_word_start(&self, offset: usize) -> usize {
        let before = self.buffer.text().get(..offset).unwrap_or_default();
        before
            .trim_end_matches(|character| !is_word_character(character))
            .trim_end_matches(is_word_character)
            .len()
    }

    /// Return the end of the word after `offset`, skipping the gap before it.
    fn find_word_end(&self, offset: usize) -> usize {
        let text = self.buffer.text();
        let rest = text
            .get(offset..)
            .unwrap_or_default()
            .trim_start_matches(|character| !is_word_character(character))
            .trim_start_matches(is_word_character);
        text.len().saturating_sub(rest.len())
    }

    /// Return the range of the word touching `offset`, or an empty range between words.
    fn find_word_around(&self, offset: usize) -> Range<usize> {
        let text = self.buffer.text();
        let before = text.get(..offset).unwrap_or_default();
        let after = text.get(offset..).unwrap_or_default();
        let start = before.trim_end_matches(is_word_character).len();
        let end = text
            .len()
            .saturating_sub(after.trim_start_matches(is_word_character).len());
        start..end
    }

    /// Return the cursor's horizontal position within its line.
    fn find_cursor_x(&self) -> Pixels {
        let cursor = self.buffer.cursor();
        self.lines
            .get(self.find_line_index(cursor))
            .map_or(px(0.), |line| {
                line.x_for_index(cursor.saturating_sub(self.find_line_start(cursor)))
            })
    }

    /// Return the offset one line above or below the cursor, keeping its horizontal position.
    fn find_vertical_target(&mut self, down: bool) -> usize {
        let line = self.find_line_index(self.buffer.cursor());
        let x = match self.goal_x {
            Some(x) => x,
            None => *self.goal_x.insert(self.find_cursor_x()),
        };
        let target = if down {
            line.saturating_add(1)
        } else if let Some(previous) = line.checked_sub(1) {
            previous
        } else {
            return 0;
        };
        let Some(shaped) = self.lines.get(target) else {
            return self.buffer.text().len();
        };
        self.find_offset_of_line(target)
            .saturating_add(shaped.closest_index_for_x(x))
    }

    /// Return the offset under a window position.
    fn find_offset_at(&self, position: Point<Pixels>) -> usize {
        let origin = self.text_origin.get();
        let line = ((f32::from(position.y) - f32::from(origin.y)) / LINE_HEIGHT)
            .floor()
            .max(0.0)
            .to_usize()
            .unwrap_or_default()
            .min(self.lines.len().saturating_sub(1));
        let x = px(f32::from(position.x) - f32::from(origin.x));
        let column = self
            .lines
            .get(line)
            .map_or(0, |shaped| shaped.closest_index_for_x(x));
        self.find_offset_of_line(line).saturating_add(column)
    }

    /// Scroll so the cursor's line is visible.
    fn reveal_cursor(&self) {
        let line = self
            .find_line_index(self.buffer.cursor())
            .to_f32()
            .unwrap_or_default();
        let top = line.mul_add(LINE_HEIGHT, TEXT_PADDING);
        let bottom = top + LINE_HEIGHT + TEXT_PADDING;
        let viewport = f32::from(self.scroll.bounds().size.height);
        let offset = self.scroll.offset();
        let scrolled = -f32::from(offset.y);
        let y = if top - TEXT_PADDING < scrolled {
            top - TEXT_PADDING
        } else if bottom > scrolled + viewport && viewport > 0.0 {
            bottom - viewport
        } else {
            return;
        };
        self.scroll.set_offset(point(offset.x, px(-y)));
    }

    /// Apply editing and navigation keys; shortcuts with modifiers are left to key bindings.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let cursor = self.buffer.cursor();
        let key = keystroke.key.as_str();
        match key {
            "left" | "right" => {
                let left = key == "left";
                let target = match self.selection() {
                    Some(selection) if !modifiers.shift => {
                        if left {
                            selection.start
                        } else {
                            selection.end
                        }
                    }
                    _ if modifiers.platform => {
                        if left {
                            self.find_line_start(cursor)
                        } else {
                            self.find_line_end(cursor)
                        }
                    }
                    _ if modifiers.alt => {
                        if left {
                            self.find_word_start(cursor)
                        } else {
                            self.find_word_end(cursor)
                        }
                    }
                    _ if left => self.buffer.find_boundary_before(cursor),
                    _ => self.buffer.find_boundary_after(cursor),
                };
                self.goal_x = None;
                self.move_to(target, modifiers.shift);
            }
            "up" | "down" => {
                let down = key == "down";
                let target = if modifiers.platform {
                    self.goal_x = None;
                    if down { self.buffer.text().len() } else { 0 }
                } else {
                    self.find_vertical_target(down)
                };
                self.move_to(target, modifiers.shift);
            }
            "home" | "end" => {
                let target = if key == "home" {
                    self.find_line_start(cursor)
                } else {
                    self.find_line_end(cursor)
                };
                self.goal_x = None;
                self.move_to(target, modifiers.shift);
            }
            "backspace" => {
                let target = if modifiers.platform {
                    self.find_line_start(cursor)
                } else if modifiers.alt {
                    self.find_word_start(cursor)
                } else {
                    self.buffer.find_boundary_before(cursor)
                };
                self.delete_toward(target, cx);
            }
            "delete" => {
                let target = if modifiers.alt {
                    self.find_word_end(cursor)
                } else {
                    self.buffer.find_boundary_after(cursor)
                };
                self.delete_toward(target, cx);
            }
            "enter" => {
                let start = self.find_line_start(cursor);
                let indent: String = self
                    .buffer
                    .text()
                    .get(start..cursor)
                    .unwrap_or_default()
                    .chars()
                    .take_while(|character| *character == ' ')
                    .collect();
                self.replace_selection(&format!("\n{indent}"), cx);
            }
            "tab" => self.replace_selection(INDENT, cx),
            "escape" => return,
            _ if modifiers.platform || modifiers.control || modifiers.function => return,
            _ => match keystroke.key_char.as_deref() {
                Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
                    self.replace_selection(text, cx);
                }
                _ => return,
            },
        }
        self.reveal_cursor();
        cx.stop_propagation();
        cx.notify();
    }

    /// Place the caret at a click, select the clicked word on a double click, and start a
    /// drag selection.
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let offset = self.find_offset_at(event.position);
        if event.click_count >= 2 {
            let word = self.find_word_around(offset);
            self.anchor = Some(word.start);
            self.buffer.move_cursor(word.end);
        } else {
            self.move_to(offset, event.modifiers.shift);
        }
        self.goal_x = None;
        self.selecting = true;
        cx.notify();
    }

    /// Extend a drag selection to the pointer.
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            let offset = self.find_offset_at(event.position);
            self.move_to(offset, true);
            cx.notify();
        }
    }

    /// Select the whole text.
    fn select_all(&mut self, _: &EditorSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.anchor = Some(0);
        self.buffer.move_cursor(self.buffer.text().len());
        cx.notify();
    }

    /// Copy the selected text to the clipboard.
    fn copy(&mut self, _: &EditorCopy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self
            .selection()
            .and_then(|selection| self.buffer.text().get(selection))
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
    }

    /// Move the selected text to the clipboard.
    fn cut(&mut self, _: &EditorCut, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection().is_some() {
            self.copy(&EditorCopy, window, cx);
            self.replace_selection("", cx);
        }
    }

    /// Replace the selection with clipboard text, expanding tabs to the editor's indent.
    fn paste(&mut self, _: &EditorPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_selection(&text.replace("\r\n", "\n").replace('\t', INDENT), cx);
        }
    }

    /// Undo the last edit.
    fn undo(&mut self, _: &EditorUndo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.take_snapshot());
            self.restore_snapshot(snapshot);
            cx.emit(CodeEdited);
            cx.notify();
        }
    }

    /// Redo the last undone edit.
    fn redo(&mut self, _: &EditorRedo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.take_snapshot());
            self.restore_snapshot(snapshot);
            cx.emit(CodeEdited);
            cx.notify();
        }
    }

    /// Shape every line of the text in the editor font.
    fn shape_lines(&self, window: &Window) -> Vec<ShapedLine> {
        let run_font = font(FONT_FAMILY);
        self.buffer
            .text()
            .split('\n')
            .map(|line| {
                window.text_system().shape_line(
                    line.to_owned().into(),
                    px(FONT_SIZE),
                    &[TextRun {
                        len: line.len(),
                        font: run_font.clone(),
                        color: theme::TEXT.into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                )
            })
            .collect()
    }

    /// Build the highlight behind each selected line segment.
    fn render_selection(&self) -> Vec<Div> {
        let Some(selection) = self.selection() else {
            return Vec::new();
        };
        let first = self.find_line_index(selection.start);
        let last = self.find_line_index(selection.end);
        // Selected newlines are shown as a space-wide highlight at the end of the line.
        let newline_width = FONT_SIZE * 0.6;
        (first..=last)
            .filter_map(|line| {
                let shaped = self.lines.get(line)?;
                let start = self.find_offset_of_line(line);
                let from = selection.start.saturating_sub(start).min(shaped.len());
                let to = selection.end.saturating_sub(start).min(shaped.len());
                let left = f32::from(shaped.x_for_index(from));
                let mut right = f32::from(shaped.x_for_index(to));
                if line < last {
                    right += newline_width;
                }
                let top = line.to_f32()?.mul_add(LINE_HEIGHT, TEXT_PADDING);
                Some(
                    div()
                        .absolute()
                        .left(px(TEXT_PADDING + left))
                        .top(px(top))
                        .w(px(right - left))
                        .h(px(LINE_HEIGHT))
                        .bg(Rgba {
                            a: 0.35,
                            ..theme::ACCENT
                        }),
                )
            })
            .collect()
    }

    /// Build the blinking caret at the cursor.
    fn render_caret(&self) -> impl IntoElement + use<> {
        let caret_line = self
            .find_line_index(self.buffer.cursor())
            .to_f32()
            .unwrap_or_default();
        div()
            .absolute()
            .left(px(TEXT_PADDING + f32::from(self.find_cursor_x())))
            .top(px(caret_line.mul_add(LINE_HEIGHT, TEXT_PADDING)))
            .w(px(1.5))
            .h(px(LINE_HEIGHT))
            .bg(theme::ACCENT)
            .with_animation(
                "code-editor-caret",
                Animation::new(Duration::from_millis(1100))
                    .repeat()
                    .with_easing(pulsating_between(0.1, 1.0)),
                Styled::opacity,
            )
    }

    /// Build the canvas that paints the shaped lines and records where the text starts.
    fn render_text(&self) -> impl IntoElement + use<> {
        let text_origin = self.text_origin.clone();
        let lines = self.lines.clone();
        canvas(
            move |bounds: Bounds<Pixels>, _, _| {
                let origin = point(
                    px(f32::from(bounds.origin.x) + TEXT_PADDING),
                    px(f32::from(bounds.origin.y) + TEXT_PADDING),
                );
                text_origin.set(origin);
                origin
            },
            move |_, origin, window, cx| {
                for (index, line) in lines.iter().enumerate() {
                    let top = index.to_f32().unwrap_or_default() * LINE_HEIGHT;
                    let line_origin = point(origin.x, px(f32::from(origin.y) + top));
                    if let Err(error) = line.paint(
                        line_origin,
                        px(LINE_HEIGHT),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    ) {
                        eprintln!("Failed to paint script line: {error}");
                    }
                }
            },
        )
        .absolute()
        .size_full()
    }
}

impl Focusable for CodeEditor {
    /// Return the focus handle used for keyboard input.
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CodeEditor {
    /// Build the line numbers, painted text, selection, and caret inside a scrolling area.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        self.lines = self.shape_lines(window);
        let line_count = self.lines.len().to_f32().unwrap_or_default();
        let text_width = self
            .lines
            .iter()
            .map(|line| f32::from(line.width))
            .fold(0.0, f32::max);
        let text_height = line_count.mul_add(LINE_HEIGHT, 2.0 * TEXT_PADDING);
        let gutter = div()
            .flex_none()
            .w(px(GUTTER_WIDTH))
            .py(px(TEXT_PADDING))
            .pr_2()
            .text_right()
            .text_color(theme::TEXT_MUTED)
            .children((1..=self.lines.len()).map(|number| {
                div()
                    .h(px(LINE_HEIGHT))
                    .line_height(px(LINE_HEIGHT))
                    .child(number.to_string())
            }));
        let text_area = div()
            .relative()
            .flex_1()
            .min_w(px(2.0f32.mul_add(TEXT_PADDING, text_width)))
            .h(px(text_height))
            .children(self.render_selection())
            .child(self.render_text())
            .when(focused, |area| area.child(self.render_caret()));

        div()
            .id("code-editor")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting = false),
            )
            .size_full()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(if focused {
                theme::ACCENT
            } else {
                theme::BORDER_MUTED
            })
            .bg(theme::SURFACE_DEEP)
            .cursor_text()
            .font_family(FONT_FAMILY)
            .text_size(px(FONT_SIZE))
            .child(
                div()
                    .id("code-editor-scroll")
                    .size_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .child(div().flex().min_w_full().child(gutter).child(text_area)),
            )
    }
}
