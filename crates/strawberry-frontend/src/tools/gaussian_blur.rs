use crate::gui::actions::actions;
use crate::gui::theme;
// Implements UI for gaussian blur tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Point,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    GaussianBlurMessage, GaussianBlurState, PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct GaussianBlurView {
    pub state: Arc<RwLock<GaussianBlurState>>,
    pub sigma_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for GaussianBlurView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl GaussianBlurView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(GaussianBlurState::default())),
            sigma_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::GaussianBlur,
                cx.focus_handle(),
            ),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &mut Context<Self>) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    fn update_sigma(&self, sigma: f32) {
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;

        self.toggle_enabled(current_enabled, window, cx);
    }

    // Status component showing enabled/disabled state
    fn render_status(state: &GaussianBlurState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut Self,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
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
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.sigma_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    fn render_preset_button(
        label: String,
        sigma: f32,
        cx: &Context<Self>,
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
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          _window: &mut Window,
                          _cx: &mut Context<Self>| {
                        this.update_sigma(sigma);
                    },
                ),
            )
    }

    fn render_presets_section(cx: &Context<Self>) -> impl IntoElement + use<> {
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
                    .child(Self::render_preset_button("Light".to_string(), 2.0, cx))
                    .child(Self::render_preset_button("Medium".to_string(), 5.0, cx))
                    .child(Self::render_preset_button("Heavy".to_string(), 10.0, cx))
                    .child(Self::render_preset_button("Extreme".to_string(), 15.0, cx)),
            )
    }

    fn render_controls_section(
        &self,
        cx: &mut Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_sigma_control(cx, window))
    }
}

actions!([GaussianBlurToggleActivity]);

impl FrontendMessageHandler for GaussianBlurView {
    type State = GaussianBlurState;

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
            ToolState::GaussianBlurState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &GaussianBlurState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.sigma_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.sigma;
                cx.notify();
            });
        }
    }
}

impl Render for GaussianBlurView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child("Error: Unable to read gaussian blur state");
            }
        };

        div()
            .track_focus(&self.focus_handle(cx))
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
                    move |this: &mut Self,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<Self>| {
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(self.render_header())
            .child(Self::render_status(&state, cx))
            .child(self.render_controls_section(cx, window))
            .child(Self::render_presets_section(cx))
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
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new("cmd-t", GaussianBlurToggleActivity, None)]);

        // Create and initialize ValueControl components
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let sigma_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.2}", state.sigma),
                state.sigma,
                ValueControlConfig {
                    label: "Sigma".to_string(),
                    step: 0.1,
                    id: "sigma".to_string(),
                    min: 0.1,
                    max: 10.0,
                },
                cx,
            )
        });

        cx.subscribe(
            &sigma_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "sigma" {
                    let current_state = this
                        .state
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
