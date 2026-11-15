use std::{ops::Range, time::Duration};

use gpui::{
    Animation, AnimationExt, App, Context, FontWeight, HighlightStyle, KeyDownEvent, Pixels,
    ScrollStrategy, StyledText, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use super::{
    fuzzy::FuzzyMatcher,
    theme,
    tool_panel::{ListDirection, next_list_index},
};

super::actions::actions!([PickerPaste]);

const ROW_HEIGHT: f32 = 36.0;
const MAX_VISIBLE_ROWS: usize = 8;
const QUERY_FONT_SIZE: gpui::Rems = gpui::Rems(0.875);

struct Candidate {
    index: usize,
    runs: Vec<Range<usize>>,
}

pub enum PickerAction {
    Confirm(usize),
    Dismiss,
}

/// Shared search, editing, and selection state. Callers own the entries and their behavior.
pub struct GenericPicker {
    labels: Vec<&'static str>,
    matcher: FuzzyMatcher,
    query: String,
    cursor: usize,
    matches: Vec<Candidate>,
    selected: usize,
    list_scroll: UniformListScrollHandle,
}

impl GenericPicker {
    pub fn new(labels: impl IntoIterator<Item = &'static str>) -> Self {
        let mut picker = Self {
            labels: labels.into_iter().collect(),
            matcher: FuzzyMatcher::new(),
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            selected: 0,
            list_scroll: UniformListScrollHandle::new(),
        };
        picker.recompute();
        picker
    }

    pub fn set_labels(&mut self, labels: impl IntoIterator<Item = &'static str>) {
        self.labels = labels.into_iter().collect();
        self.recompute();
    }

    pub const fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn recompute(&mut self) {
        self.matcher.set_query(&self.query);

        let mut matches: Vec<Candidate> = self
            .labels
            .iter()
            .enumerate()
            .filter_map(|(index, label)| {
                self.matcher
                    .match_candidate(label)
                    .map(|matched| Candidate {
                        index,
                        runs: matched.runs,
                    })
            })
            .collect();

        matches.sort_by_key(|candidate| {
            self.labels
                .get(candidate.index)
                .map(|label| label.to_lowercase())
        });

        self.matches = matches;
        self.selected = 0;
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn move_selection(&mut self, delta: isize) {
        let direction = if delta < 0 {
            ListDirection::Up
        } else {
            ListDirection::Down
        };
        let Some(selected) = next_list_index(Some(self.selected), self.matches.len(), direction)
        else {
            return;
        };
        self.selected = selected;
        self.list_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
    }

    /// Byte offset of the char boundary before the cursor, or the cursor itself at the start.
    fn prev_boundary(&self) -> usize {
        self.query
            .get(..self.cursor)
            .unwrap_or_default()
            .chars()
            .next_back()
            .map_or(self.cursor, |c| self.cursor.saturating_sub(c.len_utf8()))
    }

    /// Byte offset of the char boundary after the cursor, or the cursor itself at the end.
    fn next_boundary(&self) -> usize {
        self.query
            .get(self.cursor..)
            .unwrap_or_default()
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor.saturating_add(c.len_utf8()))
    }

    fn insert(&mut self, text: &str) {
        self.query.insert_str(self.cursor, text);
        self.cursor = self.cursor.saturating_add(text.len());
        self.recompute();
    }

    /// Delete the char before the cursor, or everything before it when `to_start`.
    fn delete_backward(&mut self, to_start: bool) {
        let from = if to_start { 0 } else { self.prev_boundary() };
        if from == self.cursor {
            return;
        }
        self.query.replace_range(from..self.cursor, "");
        self.cursor = from;
        self.recompute();
    }

    fn delete_forward(&mut self) {
        let to = self.next_boundary();
        if to == self.cursor {
            return;
        }
        self.query.replace_range(self.cursor..to, "");
        self.recompute();
    }

    const fn move_cursor(&mut self, to: usize) {
        self.cursor = to;
    }

    pub fn paste(&mut self, cx: &App) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let line = text.lines().next().unwrap_or_default().to_string();
        self.insert(&line);
    }

    pub fn handle_key<T: 'static>(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<T>,
    ) -> Option<PickerAction> {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let action = match keystroke.key.as_str() {
            "escape" => Some(PickerAction::Dismiss),
            "enter" => self
                .matches
                .get(self.selected)
                .map(|item| PickerAction::Confirm(item.index)),
            "up" => {
                self.move_selection(-1);
                None
            }
            "down" => {
                self.move_selection(1);
                None
            }
            "tab" => {
                self.move_selection(if modifiers.shift { -1 } else { 1 });
                None
            }
            "left" => {
                self.move_cursor(if modifiers.platform {
                    0
                } else {
                    self.prev_boundary()
                });
                None
            }
            "right" => {
                self.move_cursor(if modifiers.platform {
                    self.query.len()
                } else {
                    self.next_boundary()
                });
                None
            }
            "home" => {
                self.move_cursor(0);
                None
            }
            "end" => {
                self.move_cursor(self.query.len());
                None
            }
            "backspace" => {
                self.delete_backward(modifiers.platform || modifiers.alt);
                None
            }
            "delete" => {
                self.delete_forward();
                None
            }
            _ if modifiers.platform || modifiers.control || modifiers.function => return None,
            _ => match keystroke.key_char.as_deref() {
                Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
                    self.insert(text);
                    None
                }
                _ => return None,
            },
        };
        cx.stop_propagation();
        cx.notify();
        action
    }

    pub fn render_query(
        &self,
        placeholder: &'static str,
        window: &Window,
    ) -> impl IntoElement + use<> {
        let style = window.text_style();
        let font_size = QUERY_FONT_SIZE.to_pixels(window.rem_size());
        let line_height = px(f32::from(font_size) * 1.3).round();
        let shaped = window.text_system().shape_line(
            self.query.clone().into(),
            font_size,
            &[gpui::TextRun {
                len: self.query.len(),
                font: style.font(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let caret = div()
            .absolute()
            .left(shaped.x_for_index(self.cursor))
            .top_0()
            .w(px(1.5))
            .h(line_height)
            .bg(theme::ACCENT)
            .with_animation(
                "search-picker-caret",
                Animation::new(Duration::from_millis(1100))
                    .repeat()
                    .with_easing(gpui::pulsating_between(0.1, 1.0)),
                gpui::Styled::opacity,
            );

        div()
            .m_2()
            .flex()
            .items_center()
            .gap_2()
            .h(px(34.))
            .px_3()
            .rounded_md()
            .bg(theme::SURFACE_DEEP)
            .border_1()
            .border_color(theme::ACCENT)
            .overflow_hidden()
            .text_size(QUERY_FONT_SIZE)
            .line_height(line_height)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(line_height)
                    .child(if self.query.is_empty() {
                        div().text_color(theme::TEXT_MUTED).child(placeholder)
                    } else {
                        div().child(self.query.clone())
                    })
                    .child(caret),
            )
    }

    pub fn render_row(&self, row: usize) -> Option<(usize, gpui::Stateful<gpui::Div>)> {
        let candidate = self.matches.get(row)?;
        let label = *self.labels.get(candidate.index)?;
        let selected = row == self.selected;

        let label = StyledText::new(label).with_highlights(candidate.runs.iter().map(|run| {
            (
                run.clone(),
                HighlightStyle {
                    color: Some(theme::ACCENT.into()),
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            )
        }));

        let element = div()
            .id(("search-picker-row", row))
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .text_sm()
            .cursor_pointer()
            .border_l_2()
            .border_color(if selected {
                theme::ACCENT
            } else {
                gpui::transparent_black().into()
            })
            .bg(if selected {
                theme::CONTROL_ACTIVE
            } else {
                theme::SURFACE
            })
            .when(!selected, |el| el.hover(|style| style.bg(theme::CONTROL)))
            .child(label);
        Some((candidate.index, element))
    }

    pub fn render_results<T: 'static>(
        &self,
        max_height: Pixels,
        empty_message: &'static str,
        cx: &Context<T>,
        render_row: fn(&T, usize, &Context<T>) -> gpui::AnyElement,
    ) -> gpui::AnyElement {
        if self.matches.is_empty() {
            return div()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .px_3()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child(empty_message)
                .into_any_element();
        }
        let visible = self.matches.len().min(MAX_VISIBLE_ROWS);
        let height = px(f32::from(Pixels::from(visible)) * ROW_HEIGHT).min(max_height);
        uniform_list(
            "search-picker-results",
            self.matches.len(),
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                range
                    .map(|row| render_row(this, row, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .h(height)
        .into_any_element()
    }
}

pub fn picker_surface() -> gpui::Div {
    div()
        .occlude()
        .max_w_full()
        .flex()
        .flex_col()
        .flex_none()
        .bg(theme::SURFACE)
        .text_color(theme::TEXT)
        .rounded_lg()
        .shadow_xl()
        .border_1()
        .border_color(theme::BORDER_MUTED)
        .overflow_hidden()
}
