use crate::gui::Button;
use crate::gui::actions::actions;
use num_traits::ToPrimitive;

use crate::gui::{theme, tool_panel};
// Implements UI for morphology tool

use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};

use crate::gui::ToggleButton;
use crate::gui::value_control::{ValueControl, ValueControlConfig, ValueControlEvent};
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
    operation_focus: tool_panel::ListFocus<usize>,
}

impl Focusable for MorphologyView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl MorphologyView {
    const OPERATIONS: [(MorphologyOperation, &'static str); 4] = [
        (MorphologyOperation::Dilation, "Dilation"),
        (MorphologyOperation::Erosion, "Erosion"),
        (MorphologyOperation::Opening, "Opening"),
        (MorphologyOperation::Closing, "Closing"),
    ];

    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(MorphologyState::default())),
            element_size_control: None,
            operation_focus: tool_panel::ListFocus::new(),
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::Morphology,
                cx.focus_handle(),
            ),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &Context<Self>) {
        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ToggleActive(!current_value),
        )) {
            eprintln!("Failed to send toggle enabled command: {e}");
        }
    }

    fn update_operation_and_size(
        &self,
        operation: MorphologyOperation,
        element_size: u32,
        _window: &Window,
        _cx: &Context<Self>,
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

    fn set_operation(&self, operation: MorphologyOperation, window: &Window, cx: &Context<Self>) {
        let element_size = match self.state.read() {
            Ok(state) => state.element_size,
            Err(e) => {
                eprintln!("Failed to read state for operation change: {e}");
                return;
            }
        };
        self.update_operation_and_size(operation, element_size, window, cx);
    }

    fn adjust_element_size_value(&self, new_size: u32) {
        let current_state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;

        self.toggle_enabled(current_enabled, window, cx);
    }

    // Status component showing enabled/disabled state
    fn render_status(state: &MorphologyState, cx: &Context<Self>) -> impl IntoElement + use<> {
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

    // Operation selection component
    fn render_operation_control(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut choices = div()
            .id("morphology-operation-choices")
            .flex()
            .flex_col()
            .gap_1()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let Some(direction) = tool_panel::ListDirection::from_key(event) else {
                    return;
                };
                let target = match this.state.read() {
                    Ok(state) => {
                        let operation_ids = (0..Self::OPERATIONS.len()).collect::<Vec<_>>();
                        let current = this
                            .operation_focus
                            .focused_index(&operation_ids, window)
                            .or_else(|| {
                                Self::OPERATIONS
                                    .iter()
                                    .position(|(operation, _)| *operation == state.operation)
                            });
                        tool_panel::next_list_index(current, Self::OPERATIONS.len(), direction)
                    }
                    Err(error) => {
                        eprintln!("Failed to read state for operation navigation: {error}");
                        None
                    }
                };
                let Some(target) = target else {
                    return;
                };
                this.set_operation(Self::OPERATIONS[target].0, window, cx);
                this.operation_focus.focus(target, window, cx);
                cx.stop_propagation();
                cx.notify();
            }));
        let operation_ids = (0..Self::OPERATIONS.len()).collect::<Vec<_>>();
        self.operation_focus.sync(&operation_ids, cx);
        for (index, &(operation, label)) in Self::OPERATIONS.iter().enumerate() {
            let focus = self.operation_focus.handle(index, cx);
            choices = choices.child(Self::render_operation_button(
                label.to_string(),
                operation,
                state.operation == operation,
                focus,
                cx,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Operation:"),
            )
            .child(choices)
    }

    fn render_operation_button(
        label: String,
        operation: MorphologyOperation,
        is_selected: bool,
        focus: FocusHandle,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        Button::new(gpui::ElementId::Name(
            format!("render_operation_button-{label}").into(),
        ))
        .focus_handle(&focus)
        .selected(is_selected)
        .label(label)
        .on_click(cx.listener(
            move |this: &mut Self,
                  _event: &gpui::ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.set_operation(operation, window, cx);
            },
        ))
    }

    // Element size control component
    fn render_element_size_control(
        &self,
        _cx: &Context<Self>,
        _window: &Window,
    ) -> impl IntoElement + use<> {
        self.element_size_control.clone().map_or_else(
            || div().into_any_element(),
            gpui::IntoElement::into_any_element,
        )
    }

    // Preset button component
    fn render_preset_button(
        label: String,
        operation: MorphologyOperation,
        element_size: u32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        Button::new(gpui::ElementId::Name(
            format!("render_preset_button-{label}").into(),
        ))
        .label(label)
        .on_click(cx.listener(
            move |this: &mut Self,
                  _event: &gpui::ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                this.update_operation_and_size(operation, element_size, window, cx);
            },
        ))
    }

    // Presets section component
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
                    .child(Self::render_preset_button(
                        "Small Dilation".to_string(),
                        MorphologyOperation::Dilation,
                        3,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Medium Erosion".to_string(),
                        MorphologyOperation::Erosion,
                        5,
                        cx,
                    ))
                    .child(Self::render_preset_button(
                        "Small Opening".to_string(),
                        MorphologyOperation::Opening,
                        3,
                        cx,
                    ))
                    .child(Self::render_preset_button(
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
        cx: &Context<Self>,
        window: &Window,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_operation_control(cx))
            .child(self.render_element_size_control(cx, window))
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
                // This approximation is only used to display the integer backend state.
                let Some(display_value) = new_state.element_size.to_f32() else {
                    eprintln!("Cannot display element_size in the numeric control");
                    return;
                };
                c.value = display_value;
                cx.notify();
            });
        }
    }
}

impl Render for MorphologyView {
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
                    .w_full()
                    .text_sm()
                    .p_4()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read morphology state");
            }
        };

        div()
            .key_context("MorphologyView")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme::WINDOW_BG)
            .w_full()
            .text_sm()
            .p_4()
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
            .child(Self::render_status(&state, cx))
            .child(self.render_controls_section(cx, window))
            .child(Self::render_presets_section(cx))
    }
}

impl ToolFrontend for MorphologyView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            MorphologyToggleActivity,
            Some("MorphologyView"),
        )]);

        // Create and initialize ValueControl component
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(display_value) = state.element_size.to_f32() else {
            eprintln!("Cannot display element_size in the numeric control");
            return;
        };
        let element_size_control = cx.new(|cx| {
            ValueControl::new(
                format!("{:.0}", state.element_size),
                display_value,
                ValueControlConfig {
                    label: "Element Size".to_string(),
                    step: 1.0,
                    id: "element_size".to_string(),
                    min: 1.0,
                    max: 50.0,
                },
                cx,
            )
        });

        // Handle ValueControl events
        cx.subscribe(
            &element_size_control,
            |this, _control, event: &ValueControlEvent, _cx| {
                if event.id == "element_size"
                    && event.value.is_finite()
                    && event.value >= 0.0
                    && let Some(value) = event.value.to_u32()
                {
                    this.adjust_element_size_value(value);
                }
            },
        )
        .detach();

        self.element_size_control = Some(element_size_control);

        self.process_messages(window, cx);
    }
}
