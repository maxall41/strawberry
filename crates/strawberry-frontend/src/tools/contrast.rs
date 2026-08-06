use crate::gui::theme;
// Implements UI for contrast tool

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
    ContrastMessage, ContrastState, PipelineCommand, PipelineObjectMessage, ToolState,
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
    fn render_status(&self, state: &ContrastState, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
    fn render_min_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement + use<> {
        self.min_control
            .clone()
            .expect("min_control should be initialized before use")
    }

    // Max value control component
    fn render_max_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement + use<> {
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
    ) -> impl IntoElement + use<> {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::CONTROL)
            .cursor_pointer()
            .hover(|style| style.bg(theme::CONTROL_HOVER))
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
    fn render_presets_section(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_min_control(cx, _window))
            .child(self.render_max_control(cx, _window))
    }
}

impl_tool_view_basics!(ContrastView, "Contrast");

impl FrontendMessageHandler for ContrastView {
    type State = ContrastState;

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
            ToolState::ContrastState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &ContrastState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.min_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.min;
                cx.notify();
            });
        }
        if let Some(ref ctrl) = self.max_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.max;
                cx.notify();
            });
        }
    }
}

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
                    .bg(theme::WINDOW_BG)
                    .size_full()
                    .text_sm()
                    .p_4()
                    .shadow_lg()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read contrast state");
            }
        };

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
                    move |this: &mut ContrastView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<ContrastView>| {
                        this.focus_handle(cx).focus(window, cx);
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
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([KeyBinding::new("cmd-t", ContrastToggleActivity, None)]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let min_control = cx.new(|_cx| {
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

        let max_control = cx.new(|_cx| {
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
