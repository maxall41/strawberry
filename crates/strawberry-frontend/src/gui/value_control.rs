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
    input_error: Option<String>,
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
    /// Build the value control view
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl ValueControl {
    /// Create `ValueControl` with its initial state and configuration.
    pub fn new(text: String, value: f32, config: ValueControlConfig, cx: &App) -> Self {
        Self {
            kind: ValueKind::Decimal,
            text,
            value,
            input_error: None,
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

    /// Clamp and store an exact integer value and refresh its text.
    pub fn set_integer_value(&mut self, value: u32) {
        let (min, max) = self.integer_bounds();
        let value = value.clamp(min, max);
        self.kind = ValueKind::Integer(value);
        self.value = value.to_f32().unwrap_or_default();
        self.text = self.formatted_value();
        self.input_error = None;
    }

    /// Return the inclusive whole-number limits for this control.
    fn integer_bounds(&self) -> (u32, u32) {
        (
            self.config.min.ceil().to_u32().unwrap_or_default(),
            self.config.max.floor().to_u32().unwrap_or(u32::MAX),
        )
    }

    /// Format the current value for display and editing.
    fn formatted_value(&self) -> String {
        match self.kind {
            ValueKind::Integer(value) => value.to_string(),
            ValueKind::Decimal => format!("{:.2}", self.value),
        }
    }

    /// Clamp and store a finite numeric value and refresh its text.
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
        self.input_error = None;
    }

    /// Publish the current numeric value to control subscribers.
    fn emit_change(&self, cx: &mut Context<Self>) {
        cx.emit(ValueControlEvent {
            id: self.config.id.clone(),
            value: self.value,
            integer_value: self.integer_value(),
        });
    }

    /// Return the exact integer value when this is an integer control.
    pub const fn integer_value(&self) -> Option<u32> {
        match self.kind {
            ValueKind::Integer(value) => Some(value),
            ValueKind::Decimal => None,
        }
    }

    /// Step the value in the requested direction and publish the change.
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

    /// Update the value from horizontal drag distance.
    fn drag_to(&mut self, delta: f32) {
        let range = f64::from(self.config.max) - f64::from(self.config.min);
        if matches!(self.kind, ValueKind::Integer(_)) {
            let step = f64::from(self.config.step).round().max(1.0);
            // Very wide count ranges retain fine adjustment instead of jumping millions per pixel.
            let increment = if range <= 16_777_216.0 {
                step.max(range / 200.0)
            } else {
                step
            };
            let value = f64::from(delta)
                .mul_add(increment, self.drag_start_value)
                .round()
                .clamp(0.0, f64::from(u32::MAX));
            self.set_integer_value(value.to_u32().unwrap_or_default());
        } else {
            let scaling = range / 200.0;
            let value = f64::from(delta).mul_add(scaling, self.drag_start_value);
            self.set_value(value.to_f32().unwrap_or_default());
        }
    }

    /// Connect the view’s subscriptions and event handlers.
    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.on_focus(&self.focus_handle, window, |this, _window, cx| {
            if !this.last_focus {
                this.begin_edit();
                cx.notify();
            }
        })
        .detach();
        // Commit immediately when focus leaves, including when the containing
        // tool is no longer rendered after selecting a different sidebar panel.
        cx.on_blur(&self.focus_handle, window, |this, _window, cx| {
            this.commit_input(cx);
            this.last_focus = false;
            cx.notify();
        })
        .detach();
        cx.observe_keystrokes(|this, event, window, cx| {
            if !this.focus_handle.is_focused(window) {
                return;
            }
            if !this.last_focus {
                this.begin_edit();
            }
            this.handle_keystroke(&event.keystroke, cx);
        })
        .detach();
        self.initialized = true;
    }
    /// Prepare the current value for selected text entry.
    fn begin_edit(&mut self) {
        if !self.last_focus && self.input_error.is_none() {
            self.text = self.formatted_value();
        }
        self.highlight = true;
        self.last_focus = true;
    }

    /// Validate entered text and publish a changed value.
    fn commit_input(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = match self.kind {
            ValueKind::Integer(previous) => {
                let (min, max) = self.integer_bounds();
                let Some(value) = self
                    .text
                    .parse::<u32>()
                    .ok()
                    .filter(|value| (min..=max).contains(value))
                else {
                    self.input_error = Some(format!("Enter a whole number ({min}–{max})."));
                    return false;
                };
                self.set_integer_value(value);
                value != previous
            }
            ValueKind::Decimal => {
                let Some(value) = self
                    .text
                    .parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite())
                else {
                    self.input_error = Some("Enter a valid number.".to_string());
                    return false;
                };
                let previous = self.value;
                self.set_value(value);
                self.value.to_bits() != previous.to_bits()
            }
        };
        self.highlight = false;
        if changed {
            self.emit_change(cx);
        }
        true
    }
    /// Apply numeric editing keys to the value field.
    fn handle_keystroke(&mut self, event: &Keystroke, cx: &mut Context<Self>) {
        if event.modifiers.modified() {
            return;
        }
        match event.key.as_str() {
            "escape" => {
                self.text = self.formatted_value();
                self.input_error = None;
                self.mark_unfocus = true;
                cx.notify();
            }
            "enter" => {
                self.mark_unfocus = self.commit_input(cx);
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
                self.input_error = None;
                cx.notify();
            }
            "backspace" => {
                if self.highlight {
                    self.text = String::new();
                }
                // Handle backspace input
                self.text.pop();
                self.input_error = None;
                cx.notify();
            }
            _ => return,
        }
        self.highlight = false;
    }

    /// Build a button that increments or decrements the value.
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
    /// Return the focus handle used for keyboard interaction.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ValueControl {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep declarative GPUI layout and its event bindings together"
    )]
    /// Build the value control view and its interaction handlers.
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
            if focused {
                self.begin_edit();
            } else {
                self.last_focus = false;
                self.commit_input(cx);
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
                            .border_color(if self.input_error.is_some() { theme::DANGER } else { theme::BORDER_MUTED })
                            .hover(|style| {
                                if focused || self.input_error.is_some() {
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
                                style.border_color(if self.input_error.is_some() { theme::DANGER } else { theme::ACCENT })
                            })
                            .track_focus(&self.focus_handle(cx))
                            .text_sm()
                            .child(if focused || self.input_error.is_some() {
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
                                        this.begin_edit();
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
            .when_some(self.input_error.clone(), |control, error| {
                control.child(div().text_xs().text_color(theme::DANGER).child(error))
            })
    }
}
