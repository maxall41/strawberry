use gpui::{Context, Entity, EventEmitter, Window, div, prelude::*, px};
use num_traits::ToPrimitive;

use super::{
    slider::{Slider, SliderEvent},
    theme,
    value_control::{ValueControl, ValueControlConfig, ValueControlEvent},
    workspace::STATUS_BAR_HEIGHT,
};

/// Emitted when the user selects a stack in the status bar.
pub struct StackSelected(pub usize);

impl EventEmitter<StackSelected> for StackBar {}

/// Status bar for moving between loaded stacks with a slider or a stack number.
pub struct StackBar {
    slider: Entity<Slider>,
    /// One-based stack number, matching how stacks are presented to the user.
    number: Entity<ValueControl>,
    stack_count: usize,
}

impl StackBar {
    /// Create the status bar and keep its slider and stack number in step.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let slider = cx.new(|_| Slider::new("stack-slider", 0, 0));
        let number = cx.new(|cx| {
            ValueControl::new_integer(
                1,
                ValueControlConfig {
                    min: 1.0,
                    max: 1.0,
                    label: String::new(),
                    step: 1.0,
                    id: "stack-number".into(),
                },
                cx,
            )
        });
        cx.subscribe(&slider, |this, _, event: &SliderEvent, cx| {
            this.select_stack(usize::try_from(event.0).unwrap_or_default(), cx);
        })
        .detach();
        cx.subscribe(&number, |this, _, event: &ValueControlEvent, cx| {
            if let Some(number) = event.integer_value.and_then(|number| number.checked_sub(1)) {
                this.select_stack(usize::try_from(number).unwrap_or_default(), cx);
            }
        })
        .detach();
        Self {
            slider,
            number,
            stack_count: 0,
        }
    }

    /// Show the loaded stack count and selected stack without reporting a selection.
    pub fn set_stacks(&mut self, selected: usize, stack_count: usize, cx: &mut Context<Self>) {
        self.stack_count = stack_count;
        let last = u32::try_from(stack_count.saturating_sub(1)).unwrap_or(u32::MAX);
        let index = u32::try_from(selected).unwrap_or(last).min(last);
        self.slider.update(cx, |slider, cx| {
            slider.set_range(index, last);
            cx.notify();
        });
        self.number.update(cx, |number, cx| {
            number.config.max = last.saturating_add(1).to_f32().unwrap_or(f32::MAX);
            number.set_integer_value(index.saturating_add(1));
            cx.notify();
        });
        cx.notify();
    }

    /// Mirror a stack chosen in one control in the other and report the selection.
    fn select_stack(&self, stack: usize, cx: &mut Context<Self>) {
        let index = u32::try_from(stack).unwrap_or(u32::MAX);
        self.slider.update(cx, |slider, cx| {
            slider.set_value(index);
            cx.notify();
        });
        self.number.update(cx, |number, cx| {
            number.set_integer_value(index.saturating_add(1));
            cx.notify();
        });
        cx.emit(StackSelected(stack));
    }
}

impl Render for StackBar {
    /// Build the stack slider, stack number, and stack count.
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .h(px(STATUS_BAR_HEIGHT))
            .px_3()
            .border_t_1()
            .border_color(theme::BORDER_MUTED)
            .bg(theme::WINDOW_BG)
            .child(div().text_xs().text_color(theme::TEXT_MUTED).child("Stack"))
            .child(div().flex_1().min_w_0().child(self.slider.clone()))
            .child(self.number.clone())
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("/ {}", self.stack_count)),
            )
    }
}
