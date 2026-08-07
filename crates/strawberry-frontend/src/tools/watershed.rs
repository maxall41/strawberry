use crate::gui::theme;
// Implements UI for watershed tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Point,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*,
    px, size,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    PipelineCommand, PipelineObjectMessage, ToolState, WatershedMessage, WatershedState,
};

use strawberry_types::ToolFrontend;

pub struct WatershedView {
    pub state: Arc<RwLock<WatershedState>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    pub min_distance_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for WatershedView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl WatershedView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(WatershedState::default())),
            sigma_control: None,
            min_distance_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Watershed,
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

    fn update_parameters(&mut self, sigma: f32, min_distance: u32) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::WatershedMessage(WatershedMessage::SetParameters(
                sigma,
                min_distance,
            )),
        )) {
            eprintln!("Failed to send watershed parameters update: {e}");
        }
    }

    fn adjust_sigma(&mut self, new_sigma: f32, current_min_distance: u32) {
        self.update_parameters(new_sigma, current_min_distance);
    }

    fn adjust_min_distance(&mut self, current_sigma: f32, new_min_distance: u32) {
        self.update_parameters(current_sigma, new_min_distance);
    }

    fn toggle_activity_action(
        &mut self,
        _: &WatershedToggleActivity,
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
    fn render_status(
        &self,
        state: &WatershedState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Sigma control component
    fn render_sigma_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement + use<> {
        self.sigma_control
            .clone()
            .expect("sigma_control should be initialized before use")
    }

    // Min Distance control component
    fn render_min_distance_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement + use<> {
        self.min_distance_control
            .clone()
            .expect("min_distance_control should be initialized before use")
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        sigma: f32,
        min_distance: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .px_3()
            .py_2()
            .bg(theme::CONTROL)
            .cursor_pointer()
            .hover(|style| style.bg(theme::CONTROL_HOVER))
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
    fn render_presets_section(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_sigma_control(cx, _window))
            .child(self.render_min_distance_control(cx, _window))
    }
}

actions!([WatershedToggleActivity]);

impl_tool_view_basics!(WatershedView, "Watershed");

impl FrontendMessageHandler for WatershedView {
    type State = WatershedState;

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
            ToolState::WatershedState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &WatershedState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.sigma_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.sigma;
                cx.notify();
            });
        }
        if let Some(ref ctrl) = self.min_distance_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.min_distance as f32;
                cx.notify();
            });
        }
    }
}

impl Render for WatershedView {
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
                    .child("Error: Unable to read watershed state");
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
                    move |this: &mut WatershedView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<WatershedView>| {
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
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([KeyBinding::new("cmd-t", WatershedToggleActivity, None)]);

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

        let min_distance_control = cx.new(|_cx| {
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
                    let current_state = this
                        .state
                        .read()
                        .expect("State lock should not be poisoned");
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
                    let current_state = this
                        .state
                        .read()
                        .expect("State lock should not be poisoned");
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
