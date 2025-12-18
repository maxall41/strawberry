use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    BackendToolType, FrontendMessage, PipelineCommand, PipelineObjectMessage, SobelMessage,
    SobelState, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct SobelView {
    pub state: Arc<RwLock<SobelState>>,
    pub threshold_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for SobelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl SobelView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(SobelState::default())),
            threshold_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Sobel,
            ),
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

    fn toggle_activity_action(
        &mut self,
        _: &SobelToggleActivity,
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

    fn toggle_threshold_enabled(&mut self, current_value: bool) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::SobelMessage(SobelMessage::ToggleThreshold(!current_value)),
        )) {
            eprintln!("Failed to send toggle threshold command: {e}");
        }
    }

    fn update_threshold_value(&mut self, threshold_value: f32) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::SobelMessage(SobelMessage::SetThreshold(threshold_value)),
        )) {
            eprintln!("Failed to send threshold value update: {e}");
        }
    }

    fn adjust_threshold(&mut self, new_threshold: f32) {
        let clamped_threshold = new_threshold.clamp(0.0, 1.0);
        self.update_threshold_value(clamped_threshold);
    }

    fn render_status(&self, state: &SobelState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut SobelView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<SobelView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    fn render_threshold_toggle(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let threshold_enabled = state.use_threshold;
        ToggleButton::new(
            "Use Threshold".to_string(),
            threshold_enabled,
            move |this: &mut SobelView,
                  _event: &MouseDownEvent,
                  _window: &mut Window,
                  _cx: &mut Context<SobelView>| {
                this.toggle_threshold_enabled(threshold_enabled);
            },
        )
        .render(cx)
    }

    fn render_threshold_control(
        &self,
        state: &SobelState,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        if state.use_threshold {
            div().child(
                self.threshold_control
                    .clone()
                    .expect("threshold_control should be initialized before use"),
            )
        } else {
            div()
        }
    }

    fn render_preset_button(
        &self,
        label: String,
        threshold_value: f32,
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
                    move |this: &mut SobelView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<SobelView>| {
                        this.update_threshold_value(threshold_value);
                    },
                ),
            )
    }

    fn render_presets_section(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if state.use_threshold {
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
                        .child(self.render_preset_button("Low (0.1)".to_string(), 0.1, cx))
                        .child(self.render_preset_button("Medium (0.3)".to_string(), 0.3, cx))
                        .child(self.render_preset_button("High (0.5)".to_string(), 0.5, cx))
                        .child(self.render_preset_button("Very High (0.7)".to_string(), 0.7, cx)),
                )
        } else {
            div()
        }
    }

    fn render_controls_section(
        &self,
        state: &SobelState,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_threshold_toggle(state, cx))
            .child(self.render_threshold_control(state, cx, _window))
    }
}

actions!([SobelToggleActivity]);

impl FrontendMessageHandler for SobelView {
    type State = SobelState;

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
            ToolState::SobelState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &SobelState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.threshold_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.threshold_value;
                cx.notify();
            });
        }
    }
}

impl Render for SobelView {
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
                    .child("Error: Unable to read sobel state");
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
                    move |this: &mut SobelView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<SobelView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_controls_section(&state, cx, _window))
            .child(self.render_presets_section(&state, cx))
    }
}

impl_tool_view_basics!(SobelView, "Sobel Edge Detection");

impl ToolFrontend for SobelView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Sobel Edge Detection")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(400.)),
                origin: Point::new(px(1050.), px(200.)),
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
                BackendToolType::Sobel,
                sender,
            ))
        {
            eprintln!("Failed to send backend discovery request: {e}");
            return;
        }
        println!("Sent discovery request to backend | Sobel");

        cx.bind_keys([KeyBinding::new("cmd-t", SobelToggleActivity, None)]);

        // Create and initialize ValueControl component
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let threshold_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.3}", state.threshold_value),
                state.threshold_value,
                "Threshold".to_string(),
                0.001,
                "threshold".to_string(),
                0.0,
                1.0,
            )
        });

        cx.subscribe(
            &threshold_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "threshold" {
                    this.adjust_threshold(event.value);
                }
            },
        )
        .detach();

        self.threshold_control = Some(threshold_control);

        self.process_messages(window, cx);
    }
}
