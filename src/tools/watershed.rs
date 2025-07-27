// Implements UI for watershed tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::pipeline::{
    watershed::WatershedState, BackendToolType, FrontendMessage, PipelineCommand,
    PipelineObjectMessage, ToolState, WatershedMessage,
};

use super::ToolFrontend;

pub struct WatershedView {
    pub command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
    pub command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    pub state: Arc<RwLock<WatershedState>>,
    pub backend_tool_indicies: Vec<u16>,
    pub current_tool_index: Arc<RwLock<Option<u16>>>,
    pub send_channels: Vec<flume::Sender<PipelineCommand>>,
    pub receive_channel: Option<flume::Receiver<PipelineCommand>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    pub min_distance_control: Option<Entity<ValueControl>>,
    _cleanup_guard: crate::tools::CleanupGuard,
    focus_handle: Option<FocusHandle>,
}

impl Focusable for WatershedView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone().unwrap()
    }
}

impl WatershedView {
    pub fn new(
        command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            command_receiver,
            command_sender: command_sender.clone(),
            state: Arc::new(RwLock::new(WatershedState::default())),
            backend_tool_indicies: vec![],
            current_tool_index: Arc::new(RwLock::new(None)),
            send_channels: vec![],
            receive_channel: None,
            sigma_control: None,
            min_distance_control: None,
            _cleanup_guard: crate::tools::CleanupGuard::new(
                command_sender,
                crate::pipeline::BackendToolType::Watershed,
            ),
            focus_handle: None,
        }
    }

    pub fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (sender, receiver) = flume::unbounded();
        self.receive_channel = Some(receiver);
        if let Err(e) = self.command_sender.send(PipelineCommand::DiscoverBackend(
            BackendToolType::Watershed,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Watershed");

        // Create and initialize ValueControl components
        let state = self.state.read().unwrap().clone();
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

        // Handle ValueControl events
        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this.state.read().unwrap();
                    let min_dist = current_state.min_distance;
                    drop(current_state);
                    this.adjust_sigma(event.value, min_dist);
                }
            },
        )
        .detach();

        cx.subscribe(
            &min_distance_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min_distance" {
                    let current_state = this.state.read().unwrap();
                    let sigma = current_state.sigma;
                    drop(current_state);
                    this.adjust_min_distance(sigma, event.value as u32);
                }
            },
        )
        .detach();

        self.sigma_control = Some(sigma_control);
        self.min_distance_control = Some(min_distance_control);

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
                            ToolState::WatershedState(new_state) => {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        let sigma_ref = new_state.sigma;
                                        let min_distance_ref = new_state.min_distance;
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| {
                                            this.sigma_control.as_ref().unwrap().update(
                                                cx,
                                                |control, cx| {
                                                    control.value = sigma_ref;
                                                    cx.notify();
                                                },
                                            );
                                            this.min_distance_control.as_ref().unwrap().update(
                                                cx,
                                                |control, cx| {
                                                    control.value = min_distance_ref as f32;
                                                    cx.notify();
                                                },
                                            );
                                            cx.notify();
                                        }) {
                                            eprintln!(
                                                "Failed to update UI after state change: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    eprintln!("Failed to acquire write lock on watershed state");
                                }
                            }
                            _ => {}
                        },
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend Watershed");
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

    fn update_parameters(&mut self, sigma: f32, min_distance: u32) {
        let Some(current_tool_index) = self.get_current_tool_index() else {
            return;
        };

        if let Some(sender) = self.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::WatershedMessage(WatershedMessage::SetParameters(
                    sigma,
                    min_distance,
                )),
            )) {
                eprintln!("Failed to send watershed parameters update: {}", e);
            }
        } else {
            eprintln!("Invalid tool index: {}", current_tool_index);
        }
    }

    fn adjust_sigma(&mut self, new_sigma: f32, current_min_distance: u32) {
        self.update_parameters(new_sigma, current_min_distance);
    }

    fn adjust_min_distance(&mut self, current_sigma: f32, new_min_distance: u32) {
        self.update_parameters(current_sigma, new_min_distance);
    }

    // Header component
    fn render_header(&self) -> impl IntoElement {
        div()
            .text_xl()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_4()
            .child("Watershed")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &WatershedState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut WatershedView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<WatershedView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .render(cx)
    }

    // Sigma control component
    fn render_sigma_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        let sc = self.sigma_control.clone().unwrap();
        sc
    }

    // Min Distance control component
    fn render_min_distance_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        let mdc = self.min_distance_control.clone().unwrap();
        mdc
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        sigma: f32,
        min_distance: u32,
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
                    move |this: &mut WatershedView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<WatershedView>| {
                        this.update_parameters(sigma, min_distance);
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
                    .child(self.render_preset_button("Fine".to_string(), 0.5, 5, cx))
                    .child(self.render_preset_button("Normal".to_string(), 1.0, 10, cx))
                    .child(self.render_preset_button("Coarse".to_string(), 2.0, 20, cx))
                    .child(self.render_preset_button("Very Coarse".to_string(), 3.0, 30, cx)),
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
            .child(self.render_sigma_control(cx, _window))
            .child(self.render_min_distance_control(cx, _window))
    }
}

impl Render for WatershedView {
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
                    .child("Error: Unable to read watershed state");
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
                    move |this: &mut WatershedView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<WatershedView>| {
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

impl ToolFrontend for WatershedView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Watershed")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(450.)),
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
            BackendToolType::Watershed,
            sender,
        )) {
            eprintln!("Failed to send backend discovery request: {}", e);
            return;
        }
        println!("Sent discovery request to backend | Watershed");

        // Create and initialize ValueControl components
        let state = self.state.read().unwrap().clone();
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

        // Handle ValueControl events
        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this.state.read().unwrap();
                    let min_dist = current_state.min_distance;
                    drop(current_state);
                    this.adjust_sigma(event.value, min_dist);
                }
            },
        )
        .detach();

        cx.subscribe(
            &min_distance_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "min_distance" {
                    let current_state = this.state.read().unwrap();
                    let sigma = current_state.sigma;
                    drop(current_state);
                    this.adjust_min_distance(sigma, event.value as u32);
                }
            },
        )
        .detach();

        self.sigma_control = Some(sigma_control);
        self.min_distance_control = Some(min_distance_control);

        self.process_messages(window, cx);
    }
}
