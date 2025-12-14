use gpui::{div, prelude::*, rgb, AnyView, Context, IntoElement, MouseDownEvent, RenderOnce};

pub struct TooltipView {
    pub text: String,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .p_1()
            .bg(rgb(0x333333))
            .border_2()
            .border_color(rgb(0x979797))
            .text_color(rgb(0xffffff))
            .rounded_md()
            .text_xs()
            .child(self.text.clone())
    }
}
