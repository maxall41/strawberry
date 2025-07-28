// Implements UI for gaussian blur tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::pipeline::{
    gaussian_blur::GaussianBlurState, BackendToolType, FrontendMessage, GaussianBlurMessage,
    PipelineCommand, PipelineObjectMessage, ToolState,
};

use super::ToolFrontend;

pub struct GaussianBlurView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<GaussianBlurState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub radius_control: Option<Entity<ValueControl>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for GaussianBlurView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl GaussianBlurView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(GaussianBlurState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            radius_control: None,
            sigma_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::GaussianBlur,
            ),
            focus_handle: None,
        }
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
                    FrontendMessage::ReturnState(tool_state) => if let ToolState::GaussianBlurState(new_state) = tool_state {
                        if let Ok(mut locked_state) = state.write() {
                            if *locked_state != new_state {
                                let sigma_ref = new_state.sigma;
                                let radius_ref = new_state.radius;
                                *locked_state = new_state;
                                if let Err(e) = this.update(cx, |this, cx| {
                                    this.sigma_control
                                        .as_ref()
                                        .expect("sigma_control should be initialized before use")
                                        .update(cx, |this, cx| {
                                            this.value = sigma_ref;
                                            cx.notify();
                                        });
                                    this.radius_control
                                        .as_ref()
                                        .expect("radius_control should be initialized before use")
                                        .update(cx, |this, cx| {
                                            this.value = radius_ref;
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
                            eprintln!(
                                "Failed to acquire write lock on gaussian blur state"
                            );
                        }
                    },
                    FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                        println!("Received backend tools | Frontend GaussianBlur");
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

    fn update_radius_sigma(&mut self, radius: f32, sigma: f32) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::GaussianBlurMessage(GaussianBlurMessage::SetRadiusSigma(
                    radius, sigma,
                )),
            )) {
                eprintln!("Failed to send radius/sigma update: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Gaussian Blur")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &GaussianBlurState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut GaussianBlurView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<GaussianBlurView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .render(cx)
    }

    // Value control component for radius and sigma

    // Radius control component
    fn render_radius_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.radius_control
            .clone()
            .expect("radius_control should be initialized before use")
    }

    // Sigma control component
    fn render_sigma_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.sigma_control
            .clone()
            .expect("sigma_control should be initialized before use")
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        radius: f32,
        sigma: f32,
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
                    move |this: &mut GaussianBlurView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<GaussianBlurView>| {
                        this.update_radius_sigma(radius, sigma);
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
                    .child(self.render_preset_button("Light".to_string(), 2.0, 1.0, cx))
                    .child(self.render_preset_button("Medium".to_string(), 5.0, 2.0, cx))
                    .child(self.render_preset_button("Heavy".to_string(), 10.0, 4.0, cx))
                    .child(self.render_preset_button("Extreme".to_string(), 15.0, 6.0, cx)),
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
            .child(self.render_radius_control(cx, _window))
            .child(self.render_sigma_control(cx, _window))
    }
}

impl Render for GaussianBlurView {
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
                    .child("Error: Unable to read gaussian blur state");
            }
        };

        let is_enabled = state.is_enabled;

        if self.sigma_control.is_none() {
            return div()
                .bg(rgb(0x505050))
                .text_color(rgb(0xffffff))
                .child("Initialising...");
        }

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
                    move |this: &mut GaussianBlurView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<GaussianBlurView>| {
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

impl ToolFrontend for GaussianBlurView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Gaussian Blur")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(400.)),
                origin: Point::new(px(450.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        self.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::GaussianBlur,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | GaussianBlur");

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();

        let radius_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.2}", state.radius),
                state.radius,
                "Radius".to_string(),
                0.1,
                "radius".to_string(),
                0.1,
                10.0,
            )
        });

        let sigma_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.2}", state.sigma),
                state.sigma,
                "Sigma".to_string(),
                0.1,
                "sigma".to_string(),
                0.1,
                10.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &radius_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "radius" {
                    let current_state = this
                        .state
                        .read()
                        .expect("State lock should not be poisoned");
                    let s = current_state.sigma;
                    drop(current_state);
                    this.update_radius_sigma(event.value, s);
                }
            },
        )
        .detach();

        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this
                        .state
                        .read()
                        .expect("State lock should not be poisoned");
                    let r = current_state.radius;
                    drop(current_state);
                    this.update_radius_sigma(r, event.value);
                }
            },
        )
        .detach();

        self.radius_control = Some(radius_control);
        self.sigma_control = Some(sigma_control);

        cx.notify();

        println!("Initialized");

        self.process_messages(window, cx);
    }
}
