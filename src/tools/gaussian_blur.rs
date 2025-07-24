// Implements UI for gaussian blur tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, Bounds, Context, MouseDownEvent, Point, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{render_button_value_control, ButtonControlConfig};
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
    _cleanup_guard: crate::tools::CleanupGuard,
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
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::GaussianBlur,
            ),
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::GaussianBlur,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | GaussianBlur");
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
                match command {
                    PipelineCommand::FrontendMessage(message) => match message {
                        FrontendMessage::ReturnState(tool_state) => match tool_state {
                            ToolState::GaussianBlurState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| cx.notify()) {
                                            eprintln!(
                                                "Failed to update UI after state change: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!(
                                        "Failed to acquire write lock on gaussian blur state"
                                    );
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend GaussianBlur");
                            if let Err(e) = this.update(cx, |this, cx| {
                                if let Ok(mut current_tool_index) = this.current_tool_index.write()
                                {
                                    *current_tool_index = backend_ids.get(0).copied();
                                } else {
                                    eprintln!("Failed to acquire write lock on current tool index");
                                    return;
                                }
                                this.backend_tool_indicies = backend_ids;
                                this.send_channels = senders;

                                // Request initial state when backend is discovered
                                if let Some(sender) = this.send_channels.get(0) {
                                    if let Err(e) = sender.send(PipelineCommand::RequestState()) {
                                        eprintln!("Failed to request initial state: {}", e);
                                    }
                                }
                            }) {
                                eprintln!("Failed to update backend tools: {}", e);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
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
                eprintln!("Failed to read current tool index: {}", e);
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
                eprintln!("Failed to send toggle enabled command: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
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
                eprintln!("Failed to send radius/sigma update: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_radius(&mut self, new_radius: f32, current_sigma: f32) {
        self.update_radius_sigma(new_radius, current_sigma);
    }

    fn adjust_sigma(&mut self, current_radius: f32, new_sigma: f32) {
        self.update_radius_sigma(current_radius, new_sigma);
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
    fn render_status(&self, state: &GaussianBlurState) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .mb_3()
            .child("Enabled:")
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(if state.is_enabled {
                        rgb(0x00aa00)
                    } else {
                        rgb(0xaa0000)
                    })
                    .child(if state.is_enabled { "Yes" } else { "No" }),
            )
    }

    // Value control component for radius and sigma

    // Radius control component
    fn render_radius_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read().unwrap();
        let config = ButtonControlConfig {
            step: 0.1,
            decimal_places: 2,
        };

        render_button_value_control(
            "Radius".to_string(),
            state.radius,
            config,
            |this: &mut GaussianBlurView,
             delta: f32,
             _window: &mut Window,
             _cx: &mut Context<GaussianBlurView>| {
                let (current_radius, current_sigma) = {
                    let current_state = this.state.read().unwrap();
                    (current_state.radius, current_state.sigma)
                };
                let new_radius = (current_radius + delta).max(0.1);
                this.adjust_radius(new_radius, current_sigma);
            },
            cx,
        )
    }

    // Sigma control component
    fn render_sigma_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read().unwrap();
        let config = ButtonControlConfig {
            step: 0.1,
            decimal_places: 2,
        };

        render_button_value_control(
            "Sigma".to_string(),
            state.sigma,
            config,
            |this: &mut GaussianBlurView,
             delta: f32,
             _window: &mut Window,
             _cx: &mut Context<GaussianBlurView>| {
                let (current_radius, current_sigma) = {
                    let current_state = this.state.read().unwrap();
                    (current_state.radius, current_state.sigma)
                };
                let new_sigma = (current_sigma + delta).max(0.1);
                this.adjust_sigma(current_radius, new_sigma);
            },
            cx,
        )
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
    fn render_controls_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_radius_control(cx))
            .child(self.render_sigma_control(cx))
    }
}

impl Render for GaussianBlurView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state.clone(),
            Err(e) => {
                eprintln!("Failed to read state for rendering: {}", e);
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

        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .p_4()
            .shadow_lg()
            .text_color(rgb(0xffffff))
            .child(self.render_header())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.render_status(&state))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut GaussianBlurView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<GaussianBlurView>| {
                                this.toggle_enabled(is_enabled, window, cx);
                            },
                        ),
                    ),
            )
            .child(self.render_controls_section(cx))
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
}
