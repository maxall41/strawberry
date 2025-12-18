// Implements UI for gaussian blur tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use strawberry_types::{
    BackendToolType, FrontendMessage, GaussianBlurMessage, GaussianBlurState, PipelineCommand,
    PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct GaussianBlurView {
    pub state: Arc<RwLock<GaussianBlurState>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for GaussianBlurView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl GaussianBlurView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(GaussianBlurState::default())),
            sigma_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::GaussianBlur,
            ),
        }
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
                            if let ToolState::GaussianBlurState(new_state) = tool_state {
                                if let Ok(mut locked_state) = state.write() {
                                    if *locked_state != new_state {
                                        let sigma_ref = new_state.sigma;
                                        *locked_state = new_state;
                                        if let Err(e) = this.update(cx, |this, cx| {
                                            this.sigma_control
                                        .as_ref()
                                        .expect("sigma_control should be initialized before use")
                                        .update(cx, |control, cx| {
                                            control.value = sigma_ref;
                                            cx.notify();
                                        });
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
                            }
                        }
                        FrontendMessage::ReturnBackendTools(backend_ids, senders) => {
                            println!("Received backend tools | Frontend GaussianBlur");
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

    fn update_sigma(&mut self, sigma: f32) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::GaussianBlurMessage(GaussianBlurMessage::SetSigma(sigma)),
        )) {
            eprintln!("Failed to send radius/sigma update: {e}");
        }
    }

    fn toggle_activity_action(
        &mut self,
        _: &GaussianBlurToggleActivity,
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
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Value control component for radius and sigma
    fn render_sigma_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.sigma_control
            .clone()
            .expect("sigma_control should be initialized before use")
    }

    fn render_preset_button(
        &self,
        label: String,
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
                        this.update_sigma(sigma);
                    },
                ),
            )
    }

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
                    .child(self.render_preset_button("Light".to_string(), 2.0, cx))
                    .child(self.render_preset_button("Medium".to_string(), 5.0, cx))
                    .child(self.render_preset_button("Heavy".to_string(), 10.0, cx))
                    .child(self.render_preset_button("Extreme".to_string(), 15.0, cx)),
            )
    }

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
    }
}

actions!([GaussianBlurToggleActivity]);

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
                    move |this: &mut GaussianBlurView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<GaussianBlurView>| {
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

impl_tool_view_basics!(GaussianBlurView, "Gaussian Blur");

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
        self.base.receive_channel = Some(receiver);
        self.base.focus_handle = Some(cx.focus_handle());
        if let Err(e) = self
            .base
            .command_sender
            .send(PipelineCommand::DiscoverBackend(
                BackendToolType::GaussianBlur,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | GaussianBlur");

        cx.bind_keys([KeyBinding::new("cmd-t", GaussianBlurToggleActivity, None)]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();

        let sigma_control = cx.new(|_cx| {
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

        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this
                        .state
                        .read()
                        .expect("State lock should not be poisoned");
                    drop(current_state);
                    this.update_sigma(event.value);
                }
            },
        )
        .detach();

        self.sigma_control = Some(sigma_control);

        cx.notify();

        println!("Initialized");

        self.process_messages(window, cx);
    }
}
