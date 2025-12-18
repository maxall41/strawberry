// Implements UI for contrast tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use strawberry_types::{
    BackendToolType, ContrastMessage, ContrastState, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct ContrastView {
    pub state: Arc<RwLock<ContrastState>>,
    pub min_control: Option<Entity<ValueControl>>,
    pub max_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for ContrastView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl ContrastView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(ContrastState::default())),
            min_control: None,
            max_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Contrast,
            ),
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.base.receive_channel = Some(receiver);
        if let Err(e) = self
            .base
            .command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::Contrast,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | Contrast");

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let min_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.min),
                state.min,
                "Min".to_string(),
                0.01,
                "min".to_string(),
                0.0,
                1.0,
            )
        });

        let max_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.max),
                state.max,
                "Max".to_string(),
                0.01,
                "max".to_string(),
                0.0,
                1.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &min_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min" {
                    this.adjust_min_value(event.value);
                }
            },
        )
        .detach();

        cx.subscribe(
            &max_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "max" {
                    this.adjust_max_value(event.value);
                }
            },
        )
        .detach();

        self.min_control = Some(min_control);
        self.max_control = Some(max_control);

        self.process_messages(window, cx);
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let channel_receiver_ref = self
            .base
            .receive_channel
            .clone()
            .expect("Should never fire: Created during init");

        // Spawn message processing thread
        cx.spawn_in(window, async move |this, cx| loop {
            if let Ok(command) = channel_receiver_ref.recv_async().await {
                if let PipelineCommand::FrontendMessage(message) = command {
                    match message {
                        FrontendMessage::ReturnState(tool_state) => {
                            if let ToolState::ContrastState(new_state) = tool_state {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        let min_ref = new_state.min;
                                        let max_ref = new_state.max;
                                        *locked_state = new_state;
                                        if let Err(e) =
                                            this.update(cx, |this, cx| {
                                                this.min_control
                                        .as_ref()
                                        .expect("min_control should be initialized before use")
                                        .update(cx, |control, cx| {
                                            control.value = min_ref;
                                            cx.notify();
                                        });
                                                this.max_control
                                        .as_ref()
                                        .expect("max_control should be initialized before use")
                                        .update(cx, |control, cx| {
                                            control.value = max_ref;
                                            cx.notify();
                                        });
                                                cx.notify();
                                            })
                                        {
                                            eprintln!(
                                                "Failed to update UI after state change: {e}"
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on contrast state");
                                }
                            }
                        }
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Contrast");
                            if let Err(e) = this.update(cx, |this, cx| {
                                if let Ok(mut current_tool_index) =
                                    this.base.current_tool_index.write()
                                {
                                    *current_tool_index = backend_ids.first().copied();
                                } else {
                                    eprintln!("Failed to acquire write lock on current tool index");
                                    return;
                                }
                                this.base.backend_tool_indicies = backend_ids;
                                this.base.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.base.send_channels.first() {
                                    if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                        eprintln!("Failed to request initial state: {e}");
                                    }
                                }
                            }) {
                                eprintln!("Failed to update backend tools: {e}");
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                break;
            }
        })
        .detach();
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

    fn update_min_max(&mut self, min: f32, max: f32) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ContrastMessage(ContrastMessage::SetMinMax(min, max)),
        )) {
            eprintln!("Failed to send min/max update command: {e}");
        }
    }

    fn adjust_min_value(&mut self, new_min: f32) {
        let current_state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        let max_val = current_state.max;
        drop(current_state);

        self.update_min_max(new_min, max_val);
    }

    fn adjust_max_value(&mut self, new_max: f32) {
        let current_state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        let min_val = current_state.min;
        drop(current_state);

        self.update_min_max(min_val, new_max);
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &ContrastState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut ContrastView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<ContrastView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Min value control component
    fn render_min_control(&self, cx: &mut Context<Self>, _window: &mut Window) -> impl IntoElement {
        self.min_control
            .clone()
            .expect("min_control should be initialized before use")
    }

    // Max value control component
    fn render_max_control(&self, cx: &mut Context<Self>, _window: &mut Window) -> impl IntoElement {
        self.max_control
            .clone()
            .expect("max_control should be initialized before use")
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        min: f32,
        max: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x666666))
            .text_sm()
            .text_center()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut ContrastView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<ContrastView>| {
                        this.update_min_max(min, max);
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
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Quick Presets:"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.render_preset_button("Reset (0.0 - 1.0)".to_string(), 0.0, 1.0, cx))
                    .child(self.render_preset_button(
                        "High Contrast (0.2 - 0.8)".to_string(),
                        0.2,
                        0.8,
                        cx,
                    )),
            )
    }

    fn toggle_activity_action(
        &mut self,
        _: &ContrastToggleActivity,
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
            .child(self.render_min_control(cx, _window))
            .child(self.render_max_control(cx, _window))
    }
}

impl_tool_view_basics!(ContrastView, "Contrast");

actions!([ContrastToggleActivity]);

impl Render for ContrastView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
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
                    .child("Error: Unable to read contrast state");
            }
        };

        div()
            .track_focus(&self.base.focus_handle.clone().expect("initialized"))
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
                    move |this: &mut ContrastView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<ContrastView>| {
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

impl ToolFrontend for ContrastView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Contrast")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(400.)),
                origin: Point::new(px(460.), px(200.)),
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
                BackendToolType::Contrast,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | Contrast");

        cx.bind_keys([KeyBinding::new("cmd-t", ContrastToggleActivity, None)]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let min_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.min),
                state.min,
                "Min".to_string(),
                0.01,
                "min".to_string(),
                0.0,
                1.0,
            )
        });

        let max_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.max),
                state.max,
                "Max".to_string(),
                0.01,
                "max".to_string(),
                0.0,
                1.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &min_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min" {
                    this.adjust_min_value(event.value);
                }
            },
        )
        .detach();

        cx.subscribe(
            &max_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "max" {
                    this.adjust_max_value(event.value);
                }
            },
        )
        .detach();

        self.min_control = Some(min_control);
        self.max_control = Some(max_control);

        self.process_messages(window, cx);
    }
}
