use crate::gui::theme;
use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::ToggleButton;
use crate::tools::{spawn_message_processor, FrontendMessageHandler, ToolBase};
use strawberry_types::{GrayscaleState, PipelineCommand, PipelineObjectMessage, ToolState};

use strawberry_types::ToolFrontend;

pub struct GrayscaleView {
    pub state: Arc<RwLock<GrayscaleState>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for GrayscaleView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl FrontendMessageHandler for GrayscaleView {
    type State = GrayscaleState;

    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }
    fn base(&self) -> &ToolBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut ToolBase {
        &mut self.base
    }
    fn extract_state(ts: ToolState) -> Option<Self::State> {
        match ts {
            ToolState::GrayscaleState(s) => Some(s),
            _ => None,
        }
    }
}

impl GrayscaleView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(GrayscaleState::default())),
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Grayscale,
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
        _: &GrayscaleToggleActivity,
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

    fn render_status(&self, state: &GrayscaleState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut GrayscaleView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<GrayscaleView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }
}

actions!([GrayscaleToggleActivity]);

// Use macro to implement common tool methods
impl_tool_view_basics!(GrayscaleView, "Grayscale");

impl Render for GrayscaleView {
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
                    .child("Error: Unable to read grayscale state");
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
                    move |this: &mut GrayscaleView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<GrayscaleView>| {
                        this.focus_handle(cx).focus(window);
                    },
                )
            })
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(self.render_header())
            .child(self.render_status(&state, cx))
    }
}

impl ToolFrontend for GrayscaleView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Grayscale")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(280.), px(200.)),
                origin: Point::new(px(750.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([KeyBinding::new("cmd-t", GrayscaleToggleActivity, None)]);

        self.process_messages(window, cx);
    }
}
