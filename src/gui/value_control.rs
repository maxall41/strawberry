// Reusable value control components for tool UIs

use std::{borrow::BorrowMut, time::Duration};

use gpui::{
    div, prelude::*, rgb, App, Context, CursorStyle, DragMoveEvent, ElementId, EntityId,
    EventEmitter, FocusHandle, Focusable, Keystroke, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, SharedString, Window,
};
use tokio::time::Instant;

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
    last_key: String,
    initialized: bool,
    focus_handle: Option<FocusHandle>,
    highlight: bool,
    last_focus: bool,
    mark_unfocus: bool,
    mouse_down: bool,
    drag_start_position: Option<Point<Pixels>>,
    drag_start_value: f32,
}

pub struct DragDummy {}

impl Render for DragDummy {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            last_key: String::new(),
            initialized: false,
            focus_handle: None,
            highlight: false,
            last_focus: false,
            mark_unfocus: false,
            mouse_down: false,
            drag_start_position: None,
            drag_start_value: 0.0,
            config: ValueControlConfig {
                label,
                step,
                min,
                max,
                id,
            },
        }
    }
    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle = Some(cx.focus_handle());

        cx.observe_keystrokes(|this, event, _window, cx| {
            let now = Instant::now();
            this.last_key_time = now;

            if !this.focus_handle.as_ref().unwrap().is_focused(_window) {
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
}

impl Focusable for ValueControl {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone().unwrap()
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
            println!("Focus changed {} {}", self.config.id, focused);
            self.last_focus = focused;
            if !focused {
                self.on_unfocus(cx);
            }
        }
        let id = self.config.id.clone();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
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
                            .rounded_md()
                            .bg(rgb(0x666666))
                            .border_2()
                            .hover(|style| {
                                if !focused {
                                    style.border_color(rgb(0x999999))
                                } else {
                                    style
                                }
                            })
                            .cursor(if self.mouse_down {
                                CursorStyle::ResizeLeftRight
                            } else {
                                CursorStyle::Arrow
                            })
                            .focus(|style| {
                                style.border_color(rgb(0x0F8BE7))
                            })
                            .track_focus(&self.focus_handle(cx))
                            .text_sm()
                            .child(if focused {
                                div()
                                    .child(self.text.clone())
                                    .bg(if self.highlight && focused {
                                        rgb(0x0C8CE9)
                                    } else {
                                        rgb(0x666666)
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
                                        this.focus_handle(cx).focus(window);
                                        this.text = format!("{:.precision$}", this.value, precision = 2);
                                        this.highlight = true;
                                        this.mouse_down = true;
                                        this.drag_start_position = Some(event.position);
                                        this.drag_start_value = this.value;
                                    },
                                )
                            })
                            .on_drag(DragDummy {
                            }, |this : &DragDummy, point, _, cx| {
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
                                                let delta_x = event.event.position.x.0 - start_pos.x.0;
                                                let delta_value = delta_x * scaling_factor;
                                                let new_value = (this.drag_start_value + delta_value).clamp(this.config.min, this.config.max);
                                                this.value = new_value;
                                                this.text = format!("{:.2}", new_value);
                                            }
                                    },
                                )
                            })
                            .on_drop({
                                cx.listener(
                                    move |this: &mut ValueControl,
                                          _event: &DragMoveEvent<DragDummy>,
                                          window: &mut Window,
                                          cx: &mut Context<ValueControl>| {
                                            this.mouse_down = false;
                                            this.drag_start_position = None;
                                    },
                                )
                            })
                        ,
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
                                    .hover(|style| style.bg(rgb(0x555555)))
                                    .text_sm()
                                    .child("-")
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        {
                                            cx.listener(move |this: &mut Self, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
                                                this.value -= this.config.step;
                                                cx.emit(ValueControlEvent::value_changed(
                                                    this.config.id.clone(),
                                                    this.value,
                                                ));
                                                cx.notify();
                                            })
                                        },
                                    )
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(0x444444))
                                    .hover(|style| style.bg(rgb(0x555555)))
                                    .text_sm()
                                    .child("+")
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        {
                                            cx.listener(move |this: &mut Self, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| {
                                                this.value += this.config.step;
                                                cx.emit(ValueControlEvent::value_changed(
                                                    this.config.id.clone(),
                                                    this.value,
                                                ));
                                                cx.notify();
                                            })
                                        },
                                    ),
                            ),
                    ),
            )
    }
}
