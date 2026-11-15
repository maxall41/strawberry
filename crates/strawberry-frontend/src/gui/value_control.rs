use crate::gui::Button;
// Reusable value control components for tool UIs

use num_traits::ToPrimitive;
use std::time::Duration;

use gpui::{
    App, Context, CursorStyle, DragMoveEvent, ElementId, EventEmitter, FocusHandle, Focusable,
    Keystroke, MouseDownEvent, Pixels, Point, SharedString, Window, div, prelude::*, px,
};
use tokio::time::Instant;

use super::{icon::Icon, theme};

#[derive(Clone)]
pub struct ValueControlEvent {
    pub id: String,
    pub value: f32,
    /// Exact pixel counts and other integers that may not fit in an f32.
    pub integer_value: Option<u32>,
}

impl EventEmitter<ValueControlEvent> for ValueControl {}

pub struct ValueControlConfig {
    pub min: f32,
    pub max: f32,
    pub label: String,
    pub step: f32,
    pub id: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Decimal,
    Integer(u32),
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "Focus, text selection, and dragging are independent UI state flags"
)]
pub struct ValueControl {
    kind: ValueKind,
    pub config: ValueControlConfig,
    pub text: String,
    pub value: f32,
    last_key_time: Instant,
    initialized: bool,
    focus_handle: FocusHandle,
    highlight: bool,
    last_focus: bool,
    mark_unfocus: bool,
    mouse_down: bool,
    drag_start_position: Option<Point<Pixels>>,
    drag_start_value: f64,
    last_drag_emit: Option<Instant>,
}

pub struct DragDummy {}

impl Render for DragDummy {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl ValueControl {
    pub fn new(text: String, value: f32, config: ValueControlConfig, cx: &App) -> Self {
        Self {
            kind: ValueKind::Decimal,
            text,
            value,
            last_key_time: Instant::now(),
            initialized: false,
            focus_handle: cx.focus_handle(),
            highlight: false,
            last_focus: false,
            mark_unfocus: false,
            mouse_down: false,
            drag_start_position: None,
            drag_start_value: 0.0,
            last_drag_emit: None,
            config,
        }
    }
    /// Integer controls reject fractional input and quantize every interaction.
    pub fn new_integer(value: u32, config: ValueControlConfig, cx: &App) -> Self {
        let mut control = Self::new(String::new(), 0.0, config, cx);
        control.set_integer_value(value);
        control
    }

    pub fn set_integer_value(&mut self, value: u32) {
        let min = self.config.min.ceil().to_u32().unwrap_or_default();
        let max = self.config.max.floor().to_u32().unwrap_or(u32::MAX);
        let value = value.clamp(min, max);
        self.kind = ValueKind::Integer(value);
        self.value = value.to_f32().unwrap_or_default();
        self.text = self.formatted_value();
    }

    fn formatted_value(&self) -> String {
        match self.kind {
            ValueKind::Integer(value) => value.to_string(),
            ValueKind::Decimal => format!("{:.2}", self.value),
        }
    }

    pub fn set_value(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        if matches!(self.kind, ValueKind::Integer(_)) {
            self.set_integer_value(value.round().max(0.0).to_u32().unwrap_or(u32::MAX));
            return;
        }
        self.value = value.clamp(self.config.min, self.config.max);
        self.text = self.formatted_value();
    }

    fn emit_change(&self, cx: &mut Context<Self>) {
        cx.emit(ValueControlEvent {
            id: self.config.id.clone(),
            value: self.value,
            integer_value: self.integer_value(),
        });
    }

    pub const fn integer_value(&self) -> Option<u32> {
        match self.kind {
            ValueKind::Integer(value) => Some(value),
            ValueKind::Decimal => None,
        }
    }

    fn adjust(&mut self, direction: f32, cx: &mut Context<Self>) {
        if let ValueKind::Integer(value) = self.kind {
            let step = self.config.step.round().to_u32().unwrap_or(1).max(1);
            self.set_integer_value(if direction > 0.0 {
                value.saturating_add(step)
            } else {
                value.saturating_sub(step)
            });
        } else {
            self.set_value(direction.mul_add(self.config.step, self.value));
        }
        self.emit_change(cx);
        cx.notify();
    }

    fn drag_to(&mut self, delta: f32) {
        if matches!(self.kind, ValueKind::Integer(_)) {
            let step = f64::from(self.config.step.round().max(1.0));
            let value = f64::from(delta)
                .mul_add(step, self.drag_start_value)
                .round()
                .clamp(0.0, f64::from(u32::MAX));
            self.set_integer_value(value.to_u32().unwrap_or_default());
        } else {
            let scaling = f64::from(self.config.max - self.config.min) / 200.0;
            let value = f64::from(delta).mul_add(scaling, self.drag_start_value);
            self.set_value(value.to_f32().unwrap_or_default());
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Commit immediately when focus leaves, including when the containing
        // tool is no longer rendered after selecting a different sidebar panel.
        cx.on_blur(&self.focus_handle, window, |this, _window, cx| {
            this.on_unfocus(cx);
            this.last_focus = false;
            cx.notify();
        })
        .detach();
        cx.observe_keystrokes(|this, event, window, cx| {
            let now = Instant::now();
            this.last_key_time = now;

            if !this.focus_handle.is_focused(window) {
                return;
            }

            // Schedule a check after debounce period
            let event_ref = event.keystroke.clone();
            let task = cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;

                this.update(cx, |this, cx| {
                    let time_since_last = Instant::now().duration_since(this.last_key_time);
                    if time_since_last >= Duration::from_millis(10) {
                        this.handle_keystroke(&event_ref, cx);
                    }
                })
                .ok();
            });
            task.detach();
        })
        .detach();
        self.initialized = true;
    }
    fn on_unfocus(&mut self, cx: &mut Context<Self>) {
        match self.kind {
            ValueKind::Integer(_) => {
                if let Ok(value) = self.text.parse::<u32>() {
                    self.set_integer_value(value);
                    self.emit_change(cx);
                }
            }
            ValueKind::Decimal => {
                if let Ok(value) = self.text.parse::<f32>()
                    && value.is_finite()
                {
                    self.set_value(value);
                    self.emit_change(cx);
                }
            }
        }
        self.text = self.formatted_value();
    }
    fn handle_keystroke(&mut self, event: &Keystroke, cx: &mut Context<Self>) {
        match event.key.as_str() {
            "escape" | "enter" => {
                self.mark_unfocus = true;
                cx.notify();
            }
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "." => {
                if matches!(self.kind, ValueKind::Integer(_)) && event.key == "." {
                    return;
                }
                if self.highlight {
                    self.text = String::new();
                }
                // Handle number input
                self.text = format!("{}{}", self.text, event.key);
                cx.notify();
            }
            "backspace" => {
                if self.highlight {
                    self.text = String::new();
                }
                // Handle backspace input
                self.text.pop();
                cx.notify();
            }
            _ => {}
        }
        self.highlight = false;
    }

    fn step_button(id: String, direction: f32, cx: &Context<Self>) -> impl IntoElement + use<> {
        Button::new(ElementId::Name(SharedString::from(id)))
            .child(if direction > 0.0 {
                Icon::plus()
            } else {
                Icon::minus()
            })
            .on_click({
                cx.listener(
                    move |this: &mut Self,
                          _event: &gpui::ClickEvent,
                          _window: &mut Window,
                          cx: &mut Context<Self>| {
                        this.adjust(direction, cx);
                    },
                )
            })
    }
}

impl Focusable for ValueControl {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ValueControl {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep declarative GPUI layout and its event bindings together"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.initialized {
            self.initialize(window, cx);
        }
        if self.mark_unfocus {
            window.blur();
            self.mark_unfocus = false;
        }
        let focused = self.focus_handle(cx).is_focused(window);
        if focused != self.last_focus {
            self.last_focus = focused;
            if !focused {
                self.on_unfocus(cx);
            }
        }
        let id = self.config.id.clone();
        let dec_id = format!("{id}-dec");
        let inc_id = format!("{id}-inc");

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme::TEXT_MUTED)
                    .child(self.config.label.clone()),
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
                            .min_w(px(64.))
                            .rounded_md()
                            .bg(theme::SURFACE_DEEP)
                            .border_1()
                            .border_color(theme::BORDER_MUTED)
                            .hover(|style| {
                                if focused {
                                    style
                                } else {
                                    style.border_color(theme::BORDER)
                                }
                            })
                            // Resize cursor hints that the field is drag-to-adjust
                            .cursor(if focused {
                                CursorStyle::IBeam
                            } else {
                                CursorStyle::ResizeLeftRight
                            })
                            .focus(|style| {
                                style.border_color(theme::ACCENT)
                            })
                            .track_focus(&self.focus_handle(cx))
                            .text_sm()
                            .child(if focused {
                                div()
                                    .child(self.text.clone())
                                    .bg(if self.highlight && focused {
                                        theme::ACCENT
                                    } else {
                                        theme::SURFACE_DEEP
                                    })
                            } else {
                                div()
                                    .child(self.formatted_value())
                            })
                            .id(ElementId::Name(SharedString::from(id)))
                            .on_scroll_wheel(cx.listener(
                                |this, event: &gpui::ScrollWheelEvent, window, cx| {
                                    if matches!(this.kind, ValueKind::Integer(_))
                                        && this.focus_handle(cx).is_focused(window)
                                    {
                                        let delta = f32::from(event.delta.pixel_delta(px(16.)).y);
                                        if delta != 0.0 {
                                            this.adjust(delta.signum(), cx);
                                        }
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .on_mouse_down(gpui::MouseButton::Left, {
                                cx.listener(
                                    move |this: &mut Self,
                                          event: &MouseDownEvent,
                                          window: &mut Window,
                                          cx: &mut Context<Self>| {
                                        cx.stop_propagation();
                                        this.focus_handle(cx).focus(window, cx);
                                        this.text = this.formatted_value();
                                        this.highlight = true;
                                        this.mouse_down = true;
                                        this.drag_start_position = Some(event.position);
                                        this.drag_start_value = match this.kind {
                                            ValueKind::Integer(value) => f64::from(value),
                                            ValueKind::Decimal => f64::from(this.value),
                                        };
                                    },
                                )
                            })
                            .on_drag(DragDummy {
                            }, |_this : &DragDummy, _point, _, cx| {
                                  cx.new(|_| DragDummy {
                                  })
                            })
                            .on_drag_move({
                                cx.listener(
                                    move |this: &mut Self,
                                          event: &DragMoveEvent<DragDummy>,
                                          window: &mut Window,
                                          cx: &mut Context<Self>| {
                                              if !this.focus_handle(cx).is_focused(window) {
                                                  return;
                                              }
                                              if let Some(start_pos) = this.drag_start_position {
                                                let delta_x = f32::from(event.event.position.x) - f32::from(start_pos.x);
                                                this.drag_to(delta_x);
                                                // Live preview while dragging, throttled so the
                                                // backend isn't flooded with recomputes
                                                let now = Instant::now();
                                                let should_emit = this
                                                    .last_drag_emit
                                                    .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(80));
                                                if should_emit {
                                                    this.last_drag_emit = Some(now);
                                                    this.emit_change(cx);
                                                }
                                            }
                                    },
                                )
                            })
                            .on_drop({
                                cx.listener(
                                    move |this: &mut Self,
                                          _event: &DragMoveEvent<DragDummy>,
                                          _window: &mut Window,
                                          cx: &mut Context<Self>| {
                                            this.mouse_down = false;
                                            if this.drag_start_position.take().is_some() {
                                                this.last_drag_emit = None;
                                                this.emit_change(cx);
                                            }
                                    },
                                )
                            })
                        ,
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(Self::step_button(dec_id, -1.0, cx))
                            .child(Self::step_button(inc_id, 1.0, cx)),
                    ),
            )
    }
}
