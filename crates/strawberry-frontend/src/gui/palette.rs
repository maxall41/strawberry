use std::rc::Rc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseDownEvent, Render,
    Window, div, prelude::*, px,
};

use super::{
    generic_picker::{GenericPicker, PickerAction, PickerPaste, picker_surface},
    theme,
};
use crate::commands::PalleteCommand;

pub struct PaletteDismissed;

pub struct CommandPalette {
    focus_handle: FocusHandle,
    commands: Rc<[PalleteCommand]>,
    picker: GenericPicker,
}

impl EventEmitter<PaletteDismissed> for CommandPalette {}

impl Focusable for CommandPalette {
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPalette {
    /// Create `CommandPalette` with its initial state and configuration.
    pub fn new(commands: Rc<[PalleteCommand]>, cx: &Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            picker: GenericPicker::new(commands.iter().map(|command| command.label)),
            commands,
        }
    }

    /// Activate the selected picker entry.
    fn confirm(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = self.commands.get(index) else {
            return;
        };
        window.dispatch_action(command.action.boxed_clone(), cx);
        cx.emit(PaletteDismissed);
    }

    /// Handle keyboard shortcuts and navigation key presses.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match self.picker.handle_key(event, cx) {
            Some(PickerAction::Confirm(index)) => self.confirm(index, window, cx),
            Some(PickerAction::Dismiss) => cx.emit(PaletteDismissed),
            None => {}
        }
    }

    /// Build the row for the palette view.
    fn render_row(&self, row: usize, cx: &Context<Self>) -> gpui::AnyElement {
        let Some((index, element)) = self.picker.render_row(row) else {
            return div().into_any_element();
        };
        let Some(command) = self.commands.get(index) else {
            return div().into_any_element();
        };
        element
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(command.hint.label()),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.confirm(index, window, cx)))
            .into_any_element()
    }
}

impl Render for CommandPalette {
    /// Build the palette view and its interaction handlers.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let card = picker_surface()
            .id("command-palette-card")
            .key_context("CommandPalette SearchPicker")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &PickerPaste, _, cx| {
                this.picker.paste(cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::on_key_down))
            .w(px(520.))
            .on_mouse_down_out(cx.listener(|_, _: &MouseDownEvent, _, cx| {
                cx.emit(PaletteDismissed);
            }))
            .child(self.picker.render_query("Type a command…", window))
            .child(self.picker.render_results(
                px(288.),
                "No matching commands",
                cx,
                Self::render_row,
            ));

        div()
            .id("command-palette")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(card)
    }
}
