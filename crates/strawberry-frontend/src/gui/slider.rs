use std::{cell::Cell, rc::Rc};

use gpui::{
    Bounds, Context, DragMoveEvent, ElementId, EntityId, EventEmitter, MouseDownEvent, Pixels,
    Window, canvas, div, prelude::*, px, relative,
};
use num_traits::ToPrimitive;

use super::{theme, value_control::DragDummy};

const THUMB_SIZE: f32 = 12.0;
const TRACK_HEIGHT: f32 = 4.0;

/// Emitted when the user moves a slider to a new value.
pub struct SliderEvent(pub u32);

impl EventEmitter<SliderEvent> for Slider {}

/// Drag payload naming the slider being dragged; drag-move listeners see every drag of a type.
struct SliderDrag(EntityId);

/// A horizontal slider choosing a whole number from zero to a maximum.
pub struct Slider {
    id: ElementId,
    value: u32,
    max: u32,
    track_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl Slider {
    /// Create a slider at `value` within `0..=max`.
    pub fn new(id: impl Into<ElementId>, value: u32, max: u32) -> Self {
        Self {
            id: id.into(),
            value: value.min(max),
            max,
            track_bounds: Rc::default(),
        }
    }

    /// Change the range and value without notifying subscribers.
    pub fn set_range(&mut self, value: u32, max: u32) {
        self.max = max;
        self.set_value(value);
    }

    /// Change the value, clamped to the range, without notifying subscribers.
    pub fn set_value(&mut self, value: u32) {
        self.value = value.min(self.max);
    }

    /// Return the portion of the track that lies before the current value.
    fn fraction(&self) -> f32 {
        if self.max == 0 {
            return 0.0;
        }
        (self.value.to_f32().unwrap_or_default() / self.max.to_f32().unwrap_or(1.0)).clamp(0.0, 1.0)
    }

    /// Move to the value nearest a horizontal window position and report a change.
    fn move_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.track_bounds.get();
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return;
        }
        let fraction = ((f32::from(x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        let value = (fraction * self.max.to_f32().unwrap_or_default())
            .round()
            .to_u32()
            .unwrap_or_default()
            .min(self.max);
        if value != self.value {
            self.value = value;
            cx.emit(SliderEvent(value));
            cx.notify();
        }
    }
}

impl Render for Slider {
    /// Build the slider track, filled portion, and thumb with their pointer handlers.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fraction = self.fraction();
        let track_bounds = self.track_bounds.clone();
        let entity = cx.entity_id();

        div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .w_full()
            .h(px(THUMB_SIZE + 8.0))
            // Inset the track so the thumb stays inside the control at either end.
            .px(px(THUMB_SIZE / 2.0))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.move_to(event.position.x, cx);
                }),
            )
            .on_drag(SliderDrag(entity), |_, _, _, cx| cx.new(|_| DragDummy {}))
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<SliderDrag>, _, cx| {
                    if event.drag(cx).0 == entity {
                        this.move_to(event.event.position.x, cx);
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(TRACK_HEIGHT))
                    .rounded_full()
                    .bg(theme::SURFACE_DEEP)
                    .child(
                        canvas(
                            move |bounds, _, _| track_bounds.set(bounds),
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .w(relative(fraction))
                            .rounded_full()
                            .bg(theme::ACCENT),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(fraction))
                            .top(px((TRACK_HEIGHT - THUMB_SIZE) / 2.0))
                            .ml(px(-THUMB_SIZE / 2.0))
                            .size(px(THUMB_SIZE))
                            .rounded_full()
                            .bg(theme::TEXT)
                            .border_2()
                            .border_color(theme::ACCENT)
                            .hover(|style| style.bg(theme::lighten(theme::TEXT, 0.5))),
                    ),
            )
    }
}
