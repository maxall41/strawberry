// Reusable value control components for tool UIs

use std::time::Duration;

use gpui::{
    App, Context, CursorStyle, DragMoveEvent, ElementId, EventEmitter, FocusHandle, Focusable,
    Keystroke, MouseDownEvent, Pixels, Point, SharedString, Window, div, prelude::*, px,
};
use tokio::time::Instant;

use super::theme;

#[derive(Clone)]
pub struct ValueControlEvent {
    pub id: String,
    pub value: f32,
}

impl ValueControlEvent {
    pub fn value_changed(id: String, value: f32) -> Self {
        Self { id, value }
    }
}

impl EventEmitter<ValueControlEvent> for ValueControl {}

pub struct ValueControlConfig {
    pub min: f32,
    pub max: f32,
    pub label: String,
    pub step: f32,
    pub id: String,
}

pub struct ValueControl {
    pub config: ValueControlConfig,
    pub text: String,
    pub value: f32,
    last_key_time: Instant,
    initialized: bool,
    focus_handle: Option<FocusHandle>,
    highlight: bool,
    last_focus: bool,
    mark_unfocus: bool,
    mouse_down: bool,
    drag_start_position: Option<Point<Pixels>>,
    drag_start_value: f32,
    last_drag_emit: Option<Instant>,
}

pub struct DragDummy {}

impl Render for DragDummy {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl ValueControl {
    pub fn new(
        text: String,
        value: f32,
        label: String,
        step: f32,
        id: String,
        min: f32,
        max: f32,
    ) -> Self {
        ValueControl {
            text,
            value,
            last_key_time: Instant::now(),
            initialized: false,
            focus_handle: None,
            highlight: false,
            last_focus: false,
            mark_unfocus: false,
            mouse_down: false,
            drag_start_position: None,
            drag_start_value: 0.0,
            last_drag_emit: None,
            config: ValueControlConfig {
                label,
                step,
                min,
                max,
                id,
            },
        }
    }
    pub fn initialize(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle = Some(cx.focus_handle());

        cx.observe_keystrokes(|this, event, _window, cx| {
            let now = Instant::now();
            this.last_key_time = now;

            if !this
                .focus_handle
                .as_ref()
                .expect("focus_handle should be initialized before use")
                .is_focused(_window)
            {
                return;
            }

            // Schedule a check after debounce period
            let event_ref = event.keystroke.clone();
            let task = cx.spawn_in(_window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;

                this.update(cx, |this, cx| {
                    let time_since_last = Instant::now().duration_since(this.last_key_time);
                    if time_since_last >= Duration::from_millis(10) {
                        this.handle_keystroke(event_ref, cx);
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
        if let Ok(value) = self.text.parse() {
            self.value = value;
            cx.emit(ValueControlEvent::value_changed(
                self.config.id.clone(),
                self.value,
            ));
        } else {
            eprintln!("Failed to parse text into value: {}", self.text);
        }
    }
    fn handle_keystroke(&mut self, event: Keystroke, cx: &mut Context<Self>) {
        match event.key.as_str() {
            "escape" | "enter" => {
                self.mark_unfocus = true;
                cx.notify();
            }
            "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "." => {
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

    fn step_button(
        id: String,
        symbol: &'static str,
        direction: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id(ElementId::Name(SharedString::from(id)))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::CONTROL)
            .cursor_pointer()
            .hover(|style| style.bg(theme::CONTROL_HOVER))
            .active(|style| style.bg(theme::CONTROL_ACTIVE))
            .text_sm()
            .child(symbol)
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          cx: &mut Context<Self>| {
                        this.value = (this.value + direction * this.config.step)
                            .clamp(this.config.min, this.config.max);
                        this.text = format!("{:.2}", this.value);
                        cx.emit(ValueControlEvent::value_changed(
                            this.config.id.clone(),
                            this.value,
                        ));
                        cx.notify();
                    },
                )
            })
    }
}

impl Focusable for ValueControl {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl Render for ValueControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.initialized {
            self.initialize(_window, cx);
        }
        if self.mark_unfocus {
            _window.blur();
            self.mark_unfocus = false;
        }
        let focused = self.focus_handle(cx).is_focused(_window);
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
                                if !focused {
                                    style.border_color(theme::BORDER)
                                } else {
                                    style
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
                                    .child(format!("{:.precision$}", self.value, precision = 2))
                            })
                            .id(ElementId::Name(SharedString::from(id)))
                            .on_mouse_down(gpui::MouseButton::Left, {
                                cx.listener(
                                    move |this: &mut ValueControl,
                                          event: &MouseDownEvent,
                                          window: &mut Window,
                                          cx: &mut Context<ValueControl>| {
                                        cx.stop_propagation();
                                        this.focus_handle(cx).focus(window, cx);
                                        this.text = format!("{:.precision$}", this.value, precision = 2);
                                        this.highlight = true;
                                        this.mouse_down = true;
                                        this.drag_start_position = Some(event.position);
                                        this.drag_start_value = this.value;
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
                                    move |this: &mut ValueControl,
                                          event: &DragMoveEvent<DragDummy>,
                                          window: &mut Window,
                                          cx: &mut Context<ValueControl>| {
                                              if !this.focus_handle(cx).is_focused(window) {
                                                  return;
                                              }
                                              if let Some(start_pos) = this.drag_start_position {
                                                let value_range = this.config.max - this.config.min;
                                                let pixels_for_full_range = 200.0;
                                                let scaling_factor = value_range / pixels_for_full_range;
                                                let delta_x = event.event.position.x.to_f64() as f32 - start_pos.x.to_f64() as f32;
                                                let delta_value = delta_x * scaling_factor;
                                                let new_value = (this.drag_start_value + delta_value).clamp(this.config.min, this.config.max);
                                                this.value = new_value;
                                                this.text = format!("{new_value:.2}");
                                                // Live preview while dragging, throttled so the
                                                // backend isn't flooded with recomputes
                                                let now = Instant::now();
                                                let should_emit = this
                                                    .last_drag_emit
                                                    .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(80));
                                                if should_emit {
                                                    this.last_drag_emit = Some(now);
                                                    cx.emit(ValueControlEvent::value_changed(
                                                        this.config.id.clone(),
                                                        new_value,
                                                    ));
                                                }
                                            }
                                    },
                                )
                            })
                            .on_drop({
                                cx.listener(
                                    move |this: &mut ValueControl,
                                          _event: &DragMoveEvent<DragDummy>,
                                          _window: &mut Window,
                                          cx: &mut Context<ValueControl>| {
                                            this.mouse_down = false;
                                            if this.drag_start_position.take().is_some() {
                                                this.last_drag_emit = None;
                                                cx.emit(ValueControlEvent::value_changed(
                                                    this.config.id.clone(),
                                                    this.value,
                                                ));
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
                            .child(Self::step_button(dec_id, "−", -1.0, cx))
                            .child(Self::step_button(inc_id, "+", 1.0, cx)),
                    ),
            )
    }
}
