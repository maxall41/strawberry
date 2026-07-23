use crate::gui::theme;
// Implements UI for threshold tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    PipelineCommand, PipelineObjectMessage, ThresholdMessage, ThresholdState, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct ThresholdView {
    pub state: Arc<RwLock<ThresholdState>>,
    pub threshold_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for ThresholdView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl ThresholdView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(ThresholdState::default())),
            threshold_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Threshold,
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
        _: &ThresholdToggleActivity,
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

    fn update_threshold_value(&mut self, threshold_value: f32) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ThresholdMessage(ThresholdMessage::SetThresholdValue(
                threshold_value,
            )),
        )) {
            eprintln!("Failed to send threshold value update: {e}");
        }
    }

    fn adjust_threshold(&mut self, new_threshold: f32) {
        let clamped_threshold = new_threshold.clamp(0.0, 1.0);
        self.update_threshold_value(clamped_threshold);
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &ThresholdState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut ThresholdView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<ThresholdView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Threshold value control component
    fn render_threshold_control(
        &self,
        cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.threshold_control
            .clone()
            .expect("threshold_control should be initialized before use")
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        threshold_value: f32,
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
                    move |this: &mut ThresholdView,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<ThresholdView>| {
                        this.update_threshold_value(threshold_value);
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
                    .child(self.render_preset_button("Dark (0.25)".to_string(), 0.25, cx))
                    .child(self.render_preset_button("Mid (0.5)".to_string(), 0.5, cx))
                    .child(self.render_preset_button("Light (0.75)".to_string(), 0.75, cx))
                    .child(self.render_preset_button("Bright (0.9)".to_string(), 0.9, cx)),
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
            .child(self.render_threshold_control(cx, _window))
    }
}

actions!([ThresholdToggleActivity]);

impl FrontendMessageHandler for ThresholdView {
    type State = ThresholdState;

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
            ToolState::ThresholdState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &ThresholdState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.threshold_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.threshold_value;
                cx.notify();
            });
        }
    }
}

impl Render for ThresholdView {
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
                    .child("Error: Unable to read threshold state");
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
                    move |this: &mut ThresholdView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<ThresholdView>| {
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

impl_tool_view_basics!(ThresholdView, "Threshold");

impl ToolFrontend for ThresholdView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Threshold")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(350.)),
                origin: Point::new(px(750.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([KeyBinding::new("cmd-t", ThresholdToggleActivity, None)]);

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
                0.01,
                "threshold".to_string(),
                0.0,
                1.0,
            )
        });

        // Handle ValueControl events
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
