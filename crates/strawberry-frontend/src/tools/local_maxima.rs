use crate::gui::Button;
use crate::gui::actions::actions;
use num_traits::ToPrimitive;

use crate::gui::theme;
// Implements UI for local maxima tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_processing::local_maxima::ThresholdType;
use strawberry_types::{
    LocalMaximaMessage, LocalMaximaState, PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct LocalMaximaView {
    pub state: Arc<RwLock<LocalMaximaState>>,
    pub min_distance_control: Option<Entity<ValueControl>>,
    pub threshold_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
    last_absolute_value: f32,
    last_relative_value: f32,
}

impl Focusable for LocalMaximaView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl LocalMaximaView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(LocalMaximaState::default())),
            min_distance_control: None,
            threshold_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::LocalMaxima,
                cx.focus_handle(),
            ),
            last_absolute_value: 0.1,
            last_relative_value: 0.01,
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    fn update_parameters(&self, min_distance: u32, threshold: ThresholdType, exclude_border: bool) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::LocalMaximaMessage(LocalMaximaMessage::SetParameters(
                min_distance,
                threshold,
                exclude_border,
            )),
        )) {
            eprintln!("Failed to send parameter update: {e}");
        }
    }

    fn adjust_min_distance_value(&self, new_value: u32) {
        let current_state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let threshold = current_state.threshold.clone();
        let exclude_border = current_state.exclude_border;
        drop(current_state);
        self.update_parameters(new_value, threshold, exclude_border);
    }

    fn adjust_threshold_value(&self, new_value: f32) {
        let current_state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let new_threshold = match current_state.threshold {
            ThresholdType::Relative(_) => ThresholdType::Relative(new_value),
            ThresholdType::Absolute(_) | ThresholdType::None => ThresholdType::Absolute(new_value),
        };
        let min_distance = current_state.min_distance;
        let exclude_border = current_state.exclude_border;
        drop(current_state);
        self.update_parameters(min_distance, new_threshold, exclude_border);
    }

    fn toggle_exclude_border(
        &self,
        current_value: bool,
        _window: &Window,
        _cx: &mut Context<Self>,
    ) {
        let current_state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for exclude border toggle: {e}");
                return;
            }
        };
        self.update_parameters(
            current_state.min_distance,
            current_state.threshold,
            !current_value,
        );
    }

    fn toggle_threshold_mode(&mut self, _window: &Window, _cx: &mut Context<Self>) {
        let current_state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for threshold mode toggle: {e}");
                return;
            }
        };
        let new_threshold = match current_state.threshold {
            ThresholdType::Absolute(current_value) => {
                self.last_absolute_value = current_value;
                ThresholdType::Relative(self.last_relative_value)
            }
            ThresholdType::Relative(current_value) => {
                self.last_relative_value = current_value;
                ThresholdType::Absolute(self.last_absolute_value)
            }
            ThresholdType::None => ThresholdType::Relative(self.last_relative_value),
        };

        self.update_parameters(
            current_state.min_distance,
            new_threshold,
            current_state.exclude_border,
        );
    }

    fn toggle_activity_action(
        &mut self,
        _: &LocalMaximaToggleActivity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_enabled = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;

        self.toggle_enabled(current_enabled, window, cx);
    }

    // Status component showing enabled/disabled state
    fn render_status(state: &LocalMaximaState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Min distance control component
    fn render_min_distance_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.min_distance_control.clone().map_or_else(
            || gpui::div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    // Threshold control component with mode toggle
    fn render_threshold_control(
        &self,
        cx: &Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mode_label, is_enabled) = match &state.threshold {
            ThresholdType::Absolute(_) => ("Absolute", true),
            ThresholdType::Relative(_) => ("Relative", true),
            ThresholdType::None => ("None", false),
        };
        drop(state);

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Threshold:")
                    )
                    .child(
                        Button::new("local_maxima-threshold_control")
                            .label(mode_label)
                            .on_click({
                                cx.listener(move |this: &mut Self, _event: &gpui::ClickEvent, window: &mut Window, cx: &mut Context<Self>| {
                                    this.toggle_threshold_mode(window, cx);
                                })
                            }),
                    )
            )
            .when(is_enabled, |div| {
                let tc = self
                    .threshold_control
                    .clone()
                    .map_or_else(|| gpui::div().into_any_element(), gpui::IntoElement::into_any_element);
                div.child(tc)
            })
    }

    // Exclude border toggle component
    fn render_exclude_border_control(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let exclude_border = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exclude_border;
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Exclude Border:"),
            )
            .child(
                Button::new("local_maxima-exclude_border_control")
                    .selected(exclude_border)
                    .label(if exclude_border { "Yes" } else { "No" })
                    .on_click({
                        cx.listener(
                            move |this: &mut Self,
                                  _event: &gpui::ClickEvent,
                                  window: &mut Window,
                                  cx: &mut Context<Self>| {
                                this.toggle_exclude_border(exclude_border, window, cx);
                            },
                        )
                    }),
            )
    }

    // Preset button component
    fn render_preset_button(
        label: String,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        Button::new(gpui::ElementId::Name(
            format!("render_preset_button-{label}").into(),
        ))
        .large()
        .label(label)
        .on_click(cx.listener(
            move |this: &mut Self,
                  _event: &gpui::ClickEvent,
                  _window: &mut Window,
                  _cx: &mut Context<Self>| {
                this.update_parameters(min_distance, threshold.clone(), exclude_border);
            },
        ))
    }

    // Presets section component
    fn render_presets_section(cx: &Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().font_weight(gpui::FontWeight::BOLD).child("Presets:"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Self::render_preset_button(
                        "Strict".to_string(),
                        10,
                        ThresholdType::Absolute(0.2),
                        true,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Default".to_string(),
                        5,
                        ThresholdType::Relative(0.01),
                        true,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Relaxed".to_string(),
                        3,
                        ThresholdType::Relative(0.005),
                        false,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Very Relaxed".to_string(),
                        1,
                        ThresholdType::Relative(0.001),
                        false,
                        cx,
                    )),
            )
    }

    // Controls section component
    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_min_distance_control(cx, window))
            .child(self.render_threshold_control(cx, window))
            .child(self.render_exclude_border_control(cx))
    }
}

actions!([LocalMaximaToggleActivity]);

impl FrontendMessageHandler for LocalMaximaView {
    type State = LocalMaximaState;

    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    fn base(&self) -> &crate::tools::ToolBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut crate::tools::ToolBase {
        &mut self.base
    }

    fn extract_state(tool_state: ToolState) -> Option<Self::State> {
        match tool_state {
            ToolState::LocalMaximaState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &LocalMaximaState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.min_distance_control {
            ctrl.update(cx, |c, cx| {
                // This approximation is only used to display the integer backend state.
                let Some(display_value) = new_state.min_distance.to_f32() else {
                    eprintln!("Cannot display min_distance in the numeric control");
                    return;
                };
                c.value = display_value;
                cx.notify();
            });
        }
        if let Some(ref ctrl) = self.threshold_control {
            let threshold_value = match &new_state.threshold {
                ThresholdType::Absolute(val) | ThresholdType::Relative(val) => *val,
                ThresholdType::None => 0.0,
            };
            ctrl.update(cx, |c, cx| {
                c.value = threshold_value;
                cx.notify();
            });
        }
    }
}

impl Render for LocalMaximaView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for rendering: {e}");
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(theme::WINDOW_BG)
                    .w_full()
                    .text_sm()
                    .p_4()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read local maxima state");
            }
        };

        div()
            .key_context("LocalMaximaView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .w_full()
            .text_sm()
            .p_4()
            .text_color(theme::TEXT)
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(Self::render_status(&state, cx))
            .child(self.render_controls_section(cx, window))
            .child(Self::render_presets_section(cx))
    }
}

impl ToolFrontend for LocalMaximaView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            LocalMaximaToggleActivity,
            Some("LocalMaximaView"),
        )]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(display_value) = state.min_distance.to_f32() else {
            eprintln!("Cannot display min_distance in the numeric control");
            return;
        };
        let min_distance_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.0}", state.min_distance),
                display_value,
                ValueControlConfig {
                    label: "Min Distance".to_string(),
                    step: 1.0,
                    id: "min_distance".to_string(),
                    min: 1.0,
                    max: 100.0,
                },
                cx,
            )
        });

        let threshold_value = match &state.threshold {
            ThresholdType::Absolute(val) | ThresholdType::Relative(val) => *val,
            ThresholdType::None => 0.0,
        };

        let threshold_control = cx.new(|cx| {
            ValueControl::new(
                format!("{threshold_value:.3}"),
                threshold_value,
                ValueControlConfig {
                    label: "Threshold".to_string(),
                    step: 0.01,
                    id: "threshold".to_string(),
                    min: 0.0,
                    max: 1.0,
                },
                cx,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &min_distance_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min_distance"
                    && event.value.is_finite()
                    && event.value >= 0.0
                    && let Some(value) = event.value.to_u32()
                {
                    this.adjust_min_distance_value(value);
                }
            },
        )
        .detach();

        cx.subscribe(
            &threshold_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "threshold" {
                    this.adjust_threshold_value(event.value);
                }
            },
        )
        .detach();

        self.min_distance_control = Some(min_distance_control);
        self.threshold_control = Some(threshold_control);

        self.process_messages(window, cx);
    }
}
