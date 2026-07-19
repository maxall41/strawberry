use gpui::{div, prelude::*, Context, IntoElement};

use super::theme;

pub struct TooltipView {
    pub text: String,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(theme::SURFACE_DEEP)
            .border_1()
            .border_color(theme::BORDER)
            .text_color(theme::TEXT)
            .rounded_md()
            .shadow_md()
            .text_xs()
            .child(self.text.clone())
    }
}
