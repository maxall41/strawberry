use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt, App, Context, Global, MouseMoveEvent, Pixels, Point, Render,
    Transformation, Window, canvas, div, point, prelude::*, px, radians, svg,
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
    origin: Rc<Cell<Point<Pixels>>>,
}

impl BusyOverlay {
    pub fn new(blocking: bool, cx: &mut Context<Self>) -> Self {
        cx.observe_global::<PipelineBusy>(|_, cx| cx.notify())
            .detach();
        Self {
            blocking,
            cursor_position: None,
            origin: Rc::new(Cell::new(Point::default())),
        }
    }

    fn render_spinner(position: Point<Pixels>) -> impl IntoElement {
        div()
            .absolute()
            .left(px(f32::from(position.x) + 12.))
            .top(px(f32::from(position.y) + 16.))
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
        let origin = self.origin.clone();
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
                    let origin = this.origin.get();
                    this.cursor_position = Some(point(
                        px(f32::from(event.position.x) - f32::from(origin.x)),
                        px(f32::from(event.position.y) - f32::from(origin.y)),
                    ));
                    cx.notify();
                }))
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if !hovered {
                        this.cursor_position = None;
                        cx.notify();
                    }
                }))
                .child(
                    canvas(
                        move |bounds, _, _| origin.set(bounds.origin),
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .when_some(self.cursor_position, |el, position| {
                    el.child(Self::render_spinner(position))
                })
        })
    }
}
