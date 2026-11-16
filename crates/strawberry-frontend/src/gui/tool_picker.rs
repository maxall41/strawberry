use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Render, Window, div,
    prelude::*, px,
};
use strawberry_types::BackendToolType;

use super::{
    generic_picker::{GenericPicker, PickerAction, PickerPaste, picker_surface},
    theme,
};

#[derive(PartialEq, Eq)]
pub struct ToolChoice {
    pub kind: BackendToolType,
    pub label: &'static str,
}

pub enum ToolPickerEvent {
    Selected(BackendToolType),
    Dismissed,
}

pub struct ToolPicker {
    focus_handle: FocusHandle,
    tools: Vec<ToolChoice>,
    picker: GenericPicker,
}

impl EventEmitter<ToolPickerEvent> for ToolPicker {}

impl Focusable for ToolPicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ToolPicker {
    pub fn new(tools: Vec<ToolChoice>, cx: &Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            picker: GenericPicker::new(tools.iter().map(|tool| tool.label)),
            tools,
        }
    }

    fn confirm(&self, index: usize, cx: &mut Context<Self>) {
        if let Some(tool) = self.tools.get(index) {
            cx.emit(ToolPickerEvent::Selected(tool.kind.clone()));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.picker.handle_key(event, cx) {
            Some(PickerAction::Confirm(index)) => self.confirm(index, cx),
            Some(PickerAction::Dismiss) => cx.emit(ToolPickerEvent::Dismissed),
            None => {}
        }
    }

    fn render_row(&self, row: usize, cx: &Context<Self>) -> gpui::AnyElement {
        let Some((index, element)) = self.picker.render_row(row) else {
            return div().into_any_element();
        };
        element
            .on_click(cx.listener(move |this, _, _, cx| this.confirm(index, cx)))
            .into_any_element()
    }
}

impl Render for ToolPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let max_height = px((f32::from(window.viewport_size().height) - 240.0).clamp(36.0, 420.0));
        let empty_message = "No matching tools";

        picker_surface()
            .id("tool-picker")
            .key_context("ToolPicker SearchPicker")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &PickerPaste, _, cx| {
                this.picker.paste(cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("Add a tool")
                    .child(format!("{} available", self.picker.match_count())),
            )
            .child(
                self.picker
                    .render_results(max_height, empty_message, cx, Self::render_row),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(theme::BORDER)
                    .child(self.picker.render_query("Filter tools…", window)),
            )
    }
}
