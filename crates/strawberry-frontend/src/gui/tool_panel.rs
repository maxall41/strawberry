//! Shared inspector layout for interactive annotation tools.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use super::theme;
use gpui::{App, Div, ElementId, FocusHandle, ScrollHandle, Stateful, Window, div, prelude::*, px};

pub struct ListFocus<K> {
    handles: RefCell<HashMap<K, FocusHandle>>,
}

impl<K: Copy + Eq + Hash> ListFocus<K> {
    pub fn new() -> Self {
        Self {
            handles: RefCell::new(HashMap::new()),
        }
    }

    pub fn sync(&self, ids: &[K], cx: &App) {
        let current_ids: HashSet<_> = ids.iter().copied().collect();
        let mut handles = self.handles.borrow_mut();
        handles.retain(|id, _| current_ids.contains(id));
        for &id in ids {
            handles.entry(id).or_insert_with(|| cx.focus_handle());
        }
    }

    pub fn handle(&self, id: K, cx: &App) -> FocusHandle {
        self.handles
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub fn focus(&self, id: K, window: &mut Window, cx: &mut App) {
        let focus = self.handle(id, cx);
        focus.focus(window, cx);
    }

    pub fn focused_index(&self, ids: &[K], window: &Window) -> Option<usize> {
        let handles = self.handles.borrow();
        ids.iter().position(|id| {
            handles
                .get(id)
                .is_some_and(|focus| focus.is_focused(window))
        })
    }
}

#[derive(Clone, Copy)]
pub enum ListDirection {
    Up,
    Down,
}

impl ListDirection {
    pub fn from_key(event: &gpui::KeyDownEvent) -> Option<Self> {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() {
            return None;
        }
        match keystroke.key.as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

pub fn next_list_index(
    current: Option<usize>,
    len: usize,
    direction: ListDirection,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = current.filter(|&index| index < len);
    Some(match (current, direction) {
        (None, ListDirection::Up) => len.saturating_sub(1),
        (None, ListDirection::Down) => 0,
        (Some(index), ListDirection::Up) => index
            .checked_sub(1)
            .unwrap_or_else(|| len.saturating_sub(1)),
        (Some(index), ListDirection::Down) => {
            if index == len.saturating_sub(1) {
                0
            } else {
                index.saturating_add(1)
            }
        }
    })
}

pub fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .gap_4()
        .p_4()
        .text_sm()
        .text_color(theme::TEXT)
        .bg(theme::WINDOW_BG)
}

pub fn section(title: &'static str) -> Div {
    section_with_header(
        div()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child(title),
    )
}

pub fn section_with_header(header: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap_3()
        .w_full()
        .min_w_0()
        .p_3()
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::BORDER_MUTED)
        .rounded_lg()
        .child(header)
}

pub fn segmented() -> Div {
    div()
        .flex()
        .gap_1()
        .p_1()
        .rounded_md()
        .bg(theme::SURFACE_DEEP)
}

pub fn list(id: impl Into<ElementId>, scroll: &ScrollHandle) -> Stateful<Div> {
    let scroll = scroll.clone();
    div()
        .id(id)
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .min_w_0()
        .max_h(px(240.0))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .on_scroll_wheel(move |_, _, cx| {
            if scroll.max_offset().y > px(0.0) {
                cx.stop_propagation();
            }
        })
        .border_1()
        .border_color(theme::BORDER)
        .rounded_md()
        .bg(theme::SURFACE_DEEP)
}

/// Scope list interactions to their owning tool while preserving a child's own focus.
pub fn list_for_tool(
    id: impl Into<ElementId>,
    scroll: &ScrollHandle,
    owner: &FocusHandle,
) -> Stateful<Div> {
    let owner = owner.clone();
    list(id, scroll).on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
        if !window.default_prevented() {
            owner.focus(window, cx);
        }
        cx.stop_propagation();
    })
}

pub fn row() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .min_w_0()
        .min_h(px(36.0))
        .border_b_1()
        .border_color(theme::BORDER_MUTED)
        .bg(theme::SURFACE_DEEP)
}

pub fn empty(label: &'static str) -> Div {
    div()
        .flex_none()
        .p_3()
        .text_xs()
        .text_color(theme::TEXT_MUTED)
        .child(label)
}

pub fn file_actions() -> Div {
    div().flex().gap_2().min_w_0()
}

/// Selection marker shared by annotation class and count-type rows.
pub fn radio_indicator(selected: bool, color: gpui::Rgba) -> Div {
    div()
        .flex_none()
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(color)
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |el| {
            el.child(div().size(px(6.)).rounded_full().bg(color))
        })
}
