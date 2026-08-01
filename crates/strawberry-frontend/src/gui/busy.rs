use std::time::Duration;

use gpui::{
    div, prelude::*, px, radians, svg, Animation, AnimationExt, App, Context, Global,
    MouseMoveEvent, Pixels, Point, Render, Transformation, Window,
};

use super::{icon, theme};

pub struct PipelineBusy(pub bool);

impl Global for PipelineBusy {}

pub fn pipeline_busy(cx: &App) -> bool {
    cx.try_global::<PipelineBusy>().is_some_and(|busy| busy.0)
}

pub fn set_pipeline_busy(cx: &mut App, busy: bool) {
    if pipeline_busy(cx) != busy {
        cx.set_global(PipelineBusy(busy));
    }
}

pub struct BusyOverlay {
    blocking: bool,
    cursor_position: Option<Point<Pixels>>,
}

impl BusyOverlay {
    pub fn new(blocking: bool, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<PipelineBusy>(|_, cx| cx.notify())
            .detach();
        Self {
            blocking,
            cursor_position: None,
        }
    }

    fn render_spinner(position: Point<Pixels>) -> impl IntoElement {
        div()
            .absolute()
            .left(position.x + px(12.))
            .top(position.y + px(16.))
            .child(
                svg()
                    .path(icon::asset_path("spinner.svg"))
                    .size(px(16.))
                    .text_color(theme::ACCENT)
                    .with_animation(
                        "busy-cursor-spinner",
                        Animation::new(Duration::from_millis(800)).repeat(),
                        |el, delta| {
                            el.with_transformation(Transformation::rotate(radians(
                                delta * std::f32::consts::TAU,
                            )))
                        },
                    ),
            )
    }
}

impl Render for BusyOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = pipeline_busy(cx);
        if !busy {
            self.cursor_position = None;
        }
        div().id("busy-overlay").when(busy, |el| {
            el.absolute()
                .inset_0()
                .when(self.blocking, |el| {
                    el.occlude().bg(gpui::Rgba {
                        a: 0.35,
                        ..theme::WINDOW_BG
                    })
                })
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                    this.cursor_position = Some(event.position);
                    cx.notify();
                }))
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if !hovered {
                        this.cursor_position = None;
                        cx.notify();
                    }
                }))
                .when_some(self.cursor_position, |el, position| {
                    el.child(Self::render_spinner(position))
                })
        })
    }
}
