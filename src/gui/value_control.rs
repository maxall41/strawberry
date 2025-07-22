// Reusable value control components for tool UIs

use gpui::{div, prelude::*, rgb, Context, IntoElement, MouseDownEvent, Window};

/// Configuration for button-based value controls (increment/decrement)
pub struct ButtonControlConfig {
    pub step: f32,
    pub decimal_places: usize,
}

impl Default for ButtonControlConfig {
    fn default() -> Self {
        Self {
            step: 0.01,
            decimal_places: 3,
        }
    }
}

/// Creates a button-based value control with increment/decrement buttons
pub fn render_button_value_control<T: 'static>(
    label: String,
    value: f32,
    config: ButtonControlConfig,
    adjust_callback: impl Fn(&mut T, f32, &mut Window, &mut Context<T>) + 'static + Clone,
    cx: &mut Context<T>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .child(label),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x666666))
                        .text_sm()
                        .child(format!("{:.precision$}", value, precision = config.decimal_places)),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(0x444444))
                                .text_sm()
                                .child("-")
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    {
                                        let callback = adjust_callback.clone();
                                        let step = config.step;
                                        cx.listener(move |this: &mut T, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<T>| {
                                            callback(this, -step, window, cx);
                                        })
                                    },
                                ),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(0x444444))
                                .text_sm()
                                .child("+")
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    {
                                        let callback = adjust_callback;
                                        let step = config.step;
                                        cx.listener(move |this: &mut T, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<T>| {
                                            callback(this, step, window, cx);
                                        })
                                    },
                                ),
                        ),
                ),
        )
}
