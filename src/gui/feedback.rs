use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, rgb, Context, InteractiveElement, IntoElement, MouseDownEvent, ParentElement, Render,
    Styled, Window,
};

#[derive(Clone, Debug)]
pub enum ToastType {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastType {
    fn background_color(&self) -> gpui::Rgba {
        match self {
            ToastType::Info => rgb(0x3b82f6),
            ToastType::Success => rgb(0x10b981),
            ToastType::Warning => rgb(0xf59e0b),
            ToastType::Error => rgb(0xef4444),
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            ToastType::Info => "ℹ",
            ToastType::Success => "✓",
            ToastType::Warning => "⚠",
            ToastType::Error => "✕",
        }
    }
}

pub struct FeedbackModal {
    pub message: String,
    pub show_toast: bool,
    pub progress: f32,
}

impl FeedbackModal {
    pub fn new(message: String) -> Self {
        Self {
            message,
            show_toast: true,
            progress: 1.0,
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
                        .bg(rgb(0x444444))
                        .text_color(rgb(0xffffff))
                        .rounded_lg()
                        .shadow_xl()
                        .border_1()
                        .border_color(rgb(0x00000020))
                        .child(
                            // Progress bar
                            div()
                                .w_full()
                                .h_1()
                                .bg(rgb(0x666666))
                                .rounded_t_lg()
                                .child(
                                    div()
                                        .h_full()
                                        .bg(rgb(0xffffff))
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
                                        .text_xl()
                                        .opacity(0.7)
                                        .hover(|style| style.opacity(1.0))
                                        .child("×")
                                        .on_mouse_down(
                                            gpui::MouseButton::Left,
                                            cx.listener(
                                                |this: &mut FeedbackModal,
                                                 _event: &MouseDownEvent,
                                                 _window: &mut Window,
                                                 cx: &mut Context<FeedbackModal>| {
                                                    this.show_toast = false;
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
