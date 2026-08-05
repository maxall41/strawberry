use crate::gui::theme;
// Implements UI for morphology tool

use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::gui::value_control::{ValueControl, ValueControlEvent};
use crate::gui::ToggleButton;
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_processing::morphology::MorphologyOperation;
use strawberry_types::{
    MorphologyMessage, MorphologyState, PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct MorphologyView {
    pub state: Arc<RwLock<MorphologyState>>,
    pub element_size_control: Option<Entity<ValueControl>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for MorphologyView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl MorphologyView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(MorphologyState::default())),
            element_size_control: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Morphology,
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

    fn update_operation_and_size(
        &mut self,
        operation: MorphologyOperation,
        element_size: u32,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetOperation(operation)),
        )) {
            eprintln!("Failed to send operation update command: {e}");
        }
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetElementSize(
                element_size,
            )),
        )) {
            eprintln!("Failed to send element size update command: {e}");
        }
    }

    fn set_operation(
        &mut self,
        operation: MorphologyOperation,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let element_size = match self.state.read() {
            Ok(state) => state.element_size,
            Err(e) => {
                eprintln!("Failed to read state for operation change: {e}");
                return;
            }
        };
        self.update_operation_and_size(operation, element_size, _window, _cx);
    }

    fn adjust_element_size_value(&mut self, new_size: u32) {
        let current_state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        let operation = current_state.operation;
        drop(current_state);

        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetOperation(operation)),
        )) {
            eprintln!("Failed to send operation update command: {e}");
        }

        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::MorphologyMessage(MorphologyMessage::SetElementSize(new_size)),
        )) {
            eprintln!("Failed to send element size update command: {e}");
        }
    }

    fn toggle_activity_action(
        &mut self,
        _: &MorphologyToggleActivity,
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
    fn render_status(&self, state: &MorphologyState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut MorphologyView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<MorphologyView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Operation selection component
    fn render_operation_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Operation:"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.render_operation_button(
                        "Dilation".to_string(),
                        MorphologyOperation::Dilation,
                        state.operation == MorphologyOperation::Dilation,
                        cx,
                    ))
                    .child(self.render_operation_button(
                        "Erosion".to_string(),
                        MorphologyOperation::Erosion,
                        state.operation == MorphologyOperation::Erosion,
                        cx,
                    ))
                    .child(self.render_operation_button(
                        "Opening".to_string(),
                        MorphologyOperation::Opening,
                        state.operation == MorphologyOperation::Opening,
                        cx,
                    ))
                    .child(self.render_operation_button(
                        "Closing".to_string(),
                        MorphologyOperation::Closing,
                        state.operation == MorphologyOperation::Closing,
                        cx,
                    )),
            )
    }

    fn render_operation_button(
        &self,
        label: String,
        operation: MorphologyOperation,
        is_selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(if is_selected {
                theme::ACCENT
            } else {
                theme::CONTROL
            })
            .cursor_pointer()
            .hover(move |style| {
                style.bg(if is_selected {
                    theme::ACCENT_HOVER
                } else {
                    theme::CONTROL_HOVER
                })
            })
            .text_sm()
            .text_center()
            .child(label)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut MorphologyView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<MorphologyView>| {
                        this.set_operation(operation, window, cx);
                    },
                ),
            )
    }

    // Element size control component
    fn render_element_size_control(
        &self,
        _cx: &mut Context<Self>,
        _window: &mut Window,
    ) -> impl IntoElement {
        self.element_size_control
            .clone()
            .expect("element_size_control should be initialized before use")
    }

    // Preset button component
    fn render_preset_button(
        &self,
        label: String,
        operation: MorphologyOperation,
        element_size: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    move |this: &mut MorphologyView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<MorphologyView>| {
                        this.update_operation_and_size(operation, element_size, window, cx);
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
                    .child(self.render_preset_button(
                        "Small Dilation".to_string(),
                        MorphologyOperation::Dilation,
                        3,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Medium Erosion".to_string(),
                        MorphologyOperation::Erosion,
                        5,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Small Opening".to_string(),
                        MorphologyOperation::Opening,
                        3,
                        cx,
                    ))
                    .child(self.render_preset_button(
                        "Medium Closing".to_string(),
                        MorphologyOperation::Closing,
                        5,
                        cx,
                    )),
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
            .child(self.render_operation_control(cx))
            .child(self.render_element_size_control(cx, _window))
    }
}

actions!([MorphologyToggleActivity]);

impl FrontendMessageHandler for MorphologyView {
    type State = MorphologyState;

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
            ToolState::MorphologyState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &MorphologyState, cx: &mut Context<Self>) {
        if let Some(ref ctrl) = self.element_size_control {
            ctrl.update(cx, |c, cx| {
                c.value = new_state.element_size as f32;
                cx.notify();
            });
        }
    }
}

impl Render for MorphologyView {
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
                    .child("Error: Unable to read morphology state");
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
                    move |this: &mut MorphologyView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<MorphologyView>| {
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

impl_tool_view_basics!(MorphologyView, "Morphology");

impl ToolFrontend for MorphologyView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Morphological Operations")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(300.), px(500.)),
                origin: Point::new(px(470.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([KeyBinding::new("cmd-t", MorphologyToggleActivity, None)]);

        // Create and initialize ValueControl component
        let state = self
            .state
            .read()
            .expect("State lock should not be poisoned")
            .clone();
        let element_size_control = cx.new(|_cx| {
            ValueControl::new(
                format!("{:.0}", state.element_size),
                state.element_size as f32,
                "Element Size".to_string(),
                1.0,
                "element_size".to_string(),
                1.0,
                50.0,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &element_size_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "element_size" {
                    this.adjust_element_size_value(event.value as u32);
                }
            },
        )
        .detach();

        self.element_size_control = Some(element_size_control);

        self.process_messages(window, cx);
    }
}
