// Implements UI for local maxima tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::pipeline::{
    local_maxima::LocalMaximaState, BackendToolType, FrontendMessage, LocalMaximaMessage,
    PipelineCommand, PipelineObjectMessage, ToolState,
};
use crate::processing::local_maxima::ThresholdType;

use super::ToolFrontend;

pub struct LocalMaximaView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<LocalMaximaState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub min_distance_control: Option<Entity<ValueControl>>,
    pub threshold_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    last_absolute_value: f32,
    last_relative_value: f32,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for LocalMaximaView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl LocalMaximaView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(LocalMaximaState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            min_distance_control: None,
            threshold_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::LocalMaxima,
            ),
            last_absolute_value: 0.1,
            last_relative_value: 0.01,
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::LocalMaxima,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | LocalMaxima");

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

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let channel_receiver_ref = self
            .receive_channel
            .clone()
            .expect("Should never fire: Created during init");

        // Spawn message processing thread
        cx.spawn_in(window, async move |this, cx| loop {
            if let Ok(command) = channel_receiver_ref.recv_async().await {
                if let PipelineCommand::FrontendMessage(message) = command { match message {
                    FrontendMessage::ReturnState(tool_state) => if let ToolState::LocalMaximaState(new_state) = tool_state {
                        if let Ok(mut locked_state) = state.write() {
                            if *locked_state != new_state {
                                let min_distance_ref = new_state.min_distance;
                                let threshold_value_ref = match &new_state.threshold {
                                    ThresholdType::Absolute(val) => *val,
                                    ThresholdType::Relative(val) => *val,
                                    ThresholdType::None => 0.0,
                                };
                                *locked_state = new_state;
                                if let Err(e) = this.update(cx, |this, cx| {
                                    this.min_distance_control
                                        .as_ref()
                                        .expect("min_distance_control should be initialized before use")
                                        .update(cx, |control, cx| {
                                            control.value = min_distance_ref as f32;
                                            cx.notify();
                                        });
                                    this.threshold_control
                                        .as_ref()
                                        .expect("threshold_control should be initialized before use")
                                        .update(cx, |control, cx| {
                                            control.value = threshold_value_ref;
                                            cx.notify();
                                        },
                                    );
                                    cx.notify();
                                }) {
                                    eprintln!(
                                        "Failed to update UI after state change: {e}"
                                    );
                                }
                            }
                        } else {
                            eprintln!("Failed to acquire write lock on local maxima state");
                        }
                    },
                    FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                        println!("Received backend tools | Frontend LocalMaxima");
                        if let Err(e) = this.update(cx, |this, cx| {
                            if let Ok(mut current_tool_index) = this.current_tool_index.write()
                            {
                                *current_tool_index = backend_ids.first().copied();
                            } else {
                                eprintln!("Failed to acquire write lock on current tool index");
                                return;
                            }
                            this.backend_tool_indicies = backend_ids;
                            this.send_channels = senders;

                            // Request initial state when backend is discovered
                            if let Some(sender) = this.send_channels.first() {
                                if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                    eprintln!("Failed to request initial state: {e}");
                                }
                            }
                        }) {
                            eprintln!("Failed to update backend tools: {e}");
                        }
                    }
                    _ => {}
                } }
            } else {
                break;
            }
        })
        .detach();
    }

    fn get_current_tool_index(&self) -> Option<u16> {
        match self.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => Some(index),
                None => {
                    eprintln!("No current tool index available");
                    None
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                None
            }
        }
    }

    fn toggle_enabled(&mut self, current_value: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ToggleActive(!current_value),
            )) {
                eprintln!("Failed to send toggle enabled command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn update_parameters(
        &mut self,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
    ) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };
        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::LocalMaximaMessage(LocalMaximaMessage::SetParameters(
                    min_distance,
                    threshold,
                    exclude_border,
                )),
            )) {
                eprintln!("Failed to send parameter update: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
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

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Local Maxima")
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
        .render(cx)
    }

    // Min distance control component
    fn render_min_distance_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        
        self
            .min_distance_control
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
                            .bg(rgb(0x444444))
                            .hover(|style| style.bg(rgb(0x555555)))
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
                        rgb(0x00aa00)
                    } else {
                        rgb(0xaa0000)
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
            .bg(rgb(0x444444))
            .hover(|style| style.bg(rgb(0x555555)))
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
                    .bg(rgb(0x505050))
                    .size_full()
                    .p_4()
                    .shadow_lg()
                    .text_color(rgb(0xffffff))
                    .child("Error: Unable to read local maxima state");
            }
        };

        let is_enabled = state.is_enabled;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
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
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::LocalMaxima,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | LocalMaxima");

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
