use crate::gui::theme;
// Implements UI for local maxima tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_processing::local_maxima::ThresholdType;
use strawberry_types::{
    BackendToolType, FrontendMessage, LocalMaximaMessage, LocalMaximaState, PipelineCommand,
    PipelineObjectMessage, ToolState,
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
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl LocalMaximaView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(LocalMaximaState::default())),
            min_distance_control: None,
            threshold_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::LocalMaxima,
            ),
            last_absolute_value: 0.1,
            last_relative_value: 0.01,
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    fn update_parameters(
        &mut self,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
    ) {
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

    fn adjust_min_distance_value(&mut self, new_value: u32) {
        let current_state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        let threshold = current_state.threshold.clone();
        let exclude_border = current_state.exclude_border;
        drop(current_state);
        self.update_parameters(new_value, threshold, exclude_border);
    }

    fn adjust_threshold_value(&mut self, new_value: f32) {
        let current_state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        let new_threshold = match current_state.threshold {
            ThresholdType::Absolute(_) => ThresholdType::Absolute(new_value),
            ThresholdType::Relative(_) => ThresholdType::Relative(new_value),
            ThresholdType::None => ThresholdType::Absolute(new_value),
        };
        let min_distance = current_state.min_distance;
        let exclude_border = current_state.exclude_border;
        drop(current_state);
        self.update_parameters(min_distance, new_threshold, exclude_border);
    }

    fn toggle_exclude_border(
        &mut self,
        current_value: bool,
        _window: &mut Window,
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
            current_state.threshold.clone(),
            !current_value,
        );
    }

    fn toggle_threshold_mode(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
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
            .expect("State lock should not be poisoned")
            .is_enabled;

        self.toggle_enabled(current_enabled, window, cx);
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &LocalMaximaState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut LocalMaximaView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<LocalMaximaView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Min distance control component
    fn render_min_distance_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.min_distance_control
            .clone()
            .expect("min_distance_control should be initialized before use")
    }

    // Threshold control component with mode toggle
    fn render_threshold_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        let (mode_label, is_enabled) = match &state.threshold {
            ThresholdType::Absolute(_) => ("Absolute", true),
            ThresholdType::Relative(_) => ("Relative", true),
            ThresholdType::None => ("None", false),
        };

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
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme::CONTROL)
                            .cursor_pointer().hover(|style| style.bg(theme::CONTROL_HOVER))
                            .text_sm()
                            .child(mode_label)
                            .on_mouse_down(gpui::MouseButton::Left, {
                                cx.listener(move |this: &mut LocalMaximaView, _event: &MouseDownEvent, window: &mut Window, cx: &mut Context<LocalMaximaView>| {
                                    this.toggle_threshold_mode(window, cx);
                                })
                            }),
                    )
            )
            .when(is_enabled, |div| {
                let tc = self
                    .threshold_control
                    .clone()
                    .expect("threshold_control should be initialized before use");
                div.child(tc)
            })
    }

    // Exclude border toggle component
    fn render_exclude_border_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
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
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(if state.exclude_border {
                        theme::SUCCESS
                    } else {
                        theme::DANGER
                    })
                    .text_sm()
                    .child(if state.exclude_border { "Yes" } else { "No" })
                    .on_mouse_down(gpui::MouseButton::Left, {
                        let exclude_border = state.exclude_border;
                        cx.listener(
                            move |this: &mut LocalMaximaView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<LocalMaximaView>| {
                                this.toggle_exclude_border(exclude_border, window, cx);
                            },
                        )
                    }),
            )
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .bg(theme::CONTROL)
            .cursor_pointer().hover(|style| style.bg(theme::CONTROL_HOVER))
            .rounded_md()
            .text_sm()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut LocalMaximaView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<LocalMaximaView>| {
                        this.update_parameters(min_distance, threshold.clone(), exclude_border);
                    },
                ),
            )
    }

    // Presets section component
    fn render_presets_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(self.render_preset_button(
                        "Strict".to_string(),
                        10,
                        ThresholdType::Absolute(0.2),
                        true,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Default".to_string(),
                        5,
                        ThresholdType::Relative(0.01),
                        true,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Relaxed".to_string(),
                        3,
                        ThresholdType::Relative(0.005),
                        false,
                        cx,
                    ))
                    .child(self.render_preset_button(
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
        _window: &mut Window,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_min_distance_control(cx, _window))
            .child(self.render_threshold_control(cx, _window))
            .child(self.render_exclude_border_control(cx))
    }
}

actions!([LocalMaximaToggleActivity]);

impl_tool_view_basics!(LocalMaximaView, "Local Maxima");

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
                c.value = new_state.min_distance as f32;
                cx.notify();
            });
        }
        if let Some(ref ctrl) = self.threshold_control {
            let threshold_value = match &new_state.threshold {
                ThresholdType::Absolute(val) => *val,
                ThresholdType::Relative(val) => *val,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for rendering: {e}");
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .bg(theme::WINDOW_BG)
                    .size_full()
                    .text_sm()
                    .p_4()
                    .shadow_lg()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read local maxima state");
            }
        };

        let is_enabled = state.is_enabled;

        div()
            .track_focus(&self.base.focus_handle.clone().expect("initialized"))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .size_full()
            .text_sm()
            .p_4()
            .shadow_lg()
            .text_color(theme::TEXT)
            .on_mouse_down(gpui::MouseButton::Left, {
                cx.listener(
                    move |this: &mut LocalMaximaView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<LocalMaximaView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_controls_section(cx, _window))
            .child(self.render_presets_section(cx))
    }
}

impl ToolFrontend for LocalMaximaView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Local Maxima")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(320.), px(500.)),
                origin: Point::new(px(750.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.base.receive_channel = Some(receiver);
        self.base.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self
            .base
            .command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::LocalMaxima,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | LocalMaxima");

        cx.bind_keys([KeyBinding::new("cmd-t", LocalMaximaToggleActivity, None)]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let min_distance_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.0}", state.min_distance),
                state.min_distance as f32,
                "Min Distance".to_string(),
                1.0,
                "min_distance".to_string(),
                1.0,
                100.0,
            )
        });

        let threshold_value = match &state.threshold {
            ThresholdType::Absolute(val) => *val,
            ThresholdType::Relative(val) => *val,
            ThresholdType::None => 0.0,
        };

        let threshold_control = cx.new(|cx| {
            ValueControl::new(
                format!("{threshold_value:.3}"),
                threshold_value,
                "Threshold".to_string(),
                0.01,
                "threshold".to_string(),
                0.0,
                1.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &min_distance_control,
            |this, _control, event: &ValueControlEvent, cx| {
                if event.id == "min_distance" {
                    this.adjust_min_distance_value(event.value as u32);
                }
            },
        )
        .detach();

        cx.subscribe(
            &threshold_control,
            |this, _control, event: &ValueControlEvent, cx| {
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
