use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, Context, InteractiveElement, IntoElement, MouseDownEvent, ParentElement, Render, Styled,
    Window,
};

use super::theme;

pub struct FeedbackModal {
    pub message: String,
    pub show_toast: bool,
    pub progress: f32,
    pub marked_for_deletion: bool,
}

impl FeedbackModal {
    pub fn new(message: String) -> Self {
        Self {
            message,
            show_toast: true,
            progress: 1.0,
            marked_for_deletion: false,
        }
    }

    pub fn enable_auto_close(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let total_duration = Duration::from_millis(3000);
            let update_interval = Duration::from_millis(16);
            let total_steps = total_duration.as_millis() / update_interval.as_millis();

            for step in 0..=total_steps {
                let progress = 1.0 - (step as f32 / total_steps as f32);

                this.update(cx, |this, cx| {
                    this.progress = progress;
                    cx.notify();
                })
                .ok();

                if step < total_steps {
                    cx.background_executor().timer(update_interval).await;
                }
            }

            this.update(cx, |this, cx| {
                this.show_toast = false;
                this.marked_for_deletion = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for FeedbackModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .bottom_4()
            .right_4()
            .w_80()
            .when(self.show_toast, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .bg(theme::SURFACE)
                        .text_color(theme::TEXT)
                        .rounded_lg()
                        .shadow_xl()
                        .border_1()
                        .border_color(theme::BORDER_MUTED)
                        .child(
                            // Progress bar
                            div()
                                .w_full()
                                .h(gpui::px(2.))
                                .bg(theme::SURFACE_DEEP)
                                .rounded_t_lg()
                                .child(
                                    div()
                                        .h_full()
                                        .bg(theme::ACCENT)
                                        .rounded_t_lg()
                                        .w(gpui::relative(self.progress))
                                )
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .p_4()
                                .child(
                                    // Content
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .child(self.message.clone())
                                        )
                                )
                                .child(
                                    // Close button
                                    div()
                                        .flex_shrink_0()
                                        .w_6()
                                        .h_6()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .text_lg()
                                        .text_color(theme::TEXT_MUTED)
                                        .cursor_pointer()
                                        .hover(|style| {
                                            style.text_color(theme::TEXT).bg(theme::CONTROL)
                                        })
                                        .child("×")
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener(
                                                |this: &mut FeedbackModal,
                                                 _event: &MouseDownEvent,
                                                 _window: &mut Window,
                                                 cx: &mut Context<FeedbackModal>| {
                                                    this.show_toast = false;
                                                    this.marked_for_deletion = true;
                                                    cx.notify();
                                                }
                                            )
                                        )
                                )
                        )
                )
            })
    }
}
