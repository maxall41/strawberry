use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, App, Context, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, KeyDownEvent, MouseDownEvent, Render, ScrollStrategy, StyledText,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use super::fuzzy::CommandMatcher;
use super::theme;
use crate::commands::{PalettePaste, PalleteCommand};

const ROW_HEIGHT: f32 = 36.0;
const MAX_VISIBLE_ROWS: usize = 8;
const QUERY_FONT_SIZE: gpui::Rems = gpui::Rems(0.875);

/// A command that survived filtering
struct Candidate {
    index: usize,
    runs: Vec<Range<usize>>,
}

/// Emitted when the palette should be torn down.
pub struct PaletteDismissed;

pub struct CommandPalette {
    focus_handle: FocusHandle,
    commands: Rc<[PalleteCommand]>,
    matcher: CommandMatcher,
    query: String,
    cursor: usize,
    /// Sorted best match first.
    matches: Vec<Candidate>,
    selected: usize,
    list_scroll: UniformListScrollHandle,
}

impl EventEmitter<PaletteDismissed> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPalette {
    pub fn new(commands: Rc<[PalleteCommand]>, cx: &Context<Self>) -> Self {
        let mut palette = Self {
            focus_handle: cx.focus_handle(),
            commands,
            matcher: CommandMatcher::new(),
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            selected: 0,
            list_scroll: UniformListScrollHandle::new(),
        };
        palette.recompute();
        palette
    }

    fn recompute(&mut self) {
        self.matcher.set_query(&self.query);

        let mut scored: Vec<(Candidate, u32)> = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| {
                self.matcher.match_candidate(command.label).map(|m| {
                    (
                        Candidate {
                            index,
                            runs: m.runs,
                        },
                        m.score,
                    )
                })
            })
            .collect();

        scored.sort_by_key(|(candidate, _)| {
            self.commands
                .get(candidate.index)
                .map(|command| command.label.to_lowercase())
        });

        self.matches = scored.into_iter().map(|(candidate, _)| candidate).collect();
        self.selected = 0;
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let Some(count) = isize::try_from(self.matches.len()).ok() else {
            return;
        };
        let Some(selected) = isize::try_from(self.selected).ok() else {
            return;
        };
        self.selected =
            usize::try_from(selected.saturating_add(delta).rem_euclid(count)).unwrap_or_default();
        self.list_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn confirm(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(candidate) = self.matches.get(self.selected) else {
            return;
        };
        let Some(command) = self.commands.get(candidate.index) else {
            return;
        };
        let action = command.action.boxed_clone();
        window.dispatch_action(action, cx);
        cx.emit(PaletteDismissed);
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

    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        self.query.insert_str(self.cursor, text);
        self.cursor = self.cursor.saturating_add(text.len());
        self.recompute();
        cx.notify();
    }

    /// Delete the char before the cursor, or everything before it when `to_start`.
    fn delete_backward(&mut self, to_start: bool, cx: &mut Context<Self>) {
        let from = if to_start { 0 } else { self.prev_boundary() };
        if from == self.cursor {
            return;
        }
        self.query.replace_range(from..self.cursor, "");
        self.cursor = from;
        self.recompute();
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let to = self.next_boundary();
        if to == self.cursor {
            return;
        }
        self.query.replace_range(self.cursor..to, "");
        self.recompute();
        cx.notify();
    }

    fn move_cursor(&mut self, to: usize, cx: &mut Context<Self>) {
        if self.cursor != to {
            self.cursor = to;
            cx.notify();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let line = text.lines().next().unwrap_or_default().to_string();
        self.insert(&line, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;

        let handled = match keystroke.key.as_str() {
            "escape" => {
                cx.emit(PaletteDismissed);
                true
            }
            "enter" => {
                self.confirm(window, cx);
                true
            }
            "up" => {
                self.move_selection(-1, cx);
                true
            }
            "down" => {
                self.move_selection(1, cx);
                true
            }
            "tab" => {
                self.move_selection(if modifiers.shift { -1 } else { 1 }, cx);
                true
            }
            "left" => {
                let to = if modifiers.platform {
                    0
                } else {
                    self.prev_boundary()
                };
                self.move_cursor(to, cx);
                true
            }
            "right" => {
                let to = if modifiers.platform {
                    self.query.len()
                } else {
                    self.next_boundary()
                };
                self.move_cursor(to, cx);
                true
            }
            "home" => {
                self.move_cursor(0, cx);
                true
            }
            "end" => {
                self.move_cursor(self.query.len(), cx);
                true
            }
            "backspace" => {
                self.delete_backward(modifiers.platform || modifiers.alt, cx);
                true
            }
            "delete" => {
                self.delete_forward(cx);
                true
            }
            _ if modifiers.platform || modifiers.control || modifiers.function => false,
            _ => match keystroke.key_char.as_deref() {
                Some(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
                    self.insert(text, cx);
                    true
                }
                _ => false,
            },
        };

        if handled {
            cx.stop_propagation();
        }
    }

    fn render_query(&self, window: &Window) -> impl IntoElement + use<> {
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
                "palette-caret",
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
            .child(div().flex_none().text_color(theme::TEXT_MUTED).child("›"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h(line_height)
                    .child(if self.query.is_empty() {
                        div().text_color(theme::TEXT_MUTED).child("Type a command…")
                    } else {
                        div().child(self.query.clone())
                    })
                    .child(caret),
            )
    }

    fn render_row(&self, row: usize, cx: &Context<Self>) -> gpui::AnyElement {
        let Some(candidate) = self.matches.get(row) else {
            return div().into_any_element();
        };
        let Some(command) = self.commands.get(candidate.index) else {
            return div().into_any_element();
        };
        let selected = row == self.selected;

        let label =
            StyledText::new(command.label).with_highlights(candidate.runs.iter().map(|run| {
                (
                    run.clone(),
                    HighlightStyle {
                        color: Some(theme::ACCENT.into()),
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                )
            }));

        div()
            .id(("palette-row", row))
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
            .child(label)
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(command.hint.label()),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = row;
                this.confirm(window, cx);
            }))
            .into_any_element()
    }

    fn render_results(&self, cx: &Context<Self>) -> gpui::AnyElement {
        if self.matches.is_empty() {
            return div()
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .px_3()
                .text_sm()
                .text_color(theme::TEXT_MUTED)
                .child("No matching commands")
                .into_any_element();
        }

        let visible = self.matches.len().min(MAX_VISIBLE_ROWS);

        uniform_list(
            "palette-results",
            self.matches.len(),
            cx.processor(|this, range: Range<usize>, _window, cx| {
                range
                    .map(|row| this.render_row(row, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .h(px(f32::from(gpui::Pixels::from(visible)) * ROW_HEIGHT))
        .into_any_element()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let card = div()
            .id("command-palette-card")
            .w(px(520.))
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
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _window, cx| {
                cx.emit(PaletteDismissed);
            }))
            .child(self.render_query(window))
            .child(self.render_results(cx));

        div()
            .id("command-palette")
            .absolute()
            .inset_0()
            .occlude()
            .key_context("CommandPalette")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &PalettePaste, _window, cx| this.paste(cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(card)
    }
}
