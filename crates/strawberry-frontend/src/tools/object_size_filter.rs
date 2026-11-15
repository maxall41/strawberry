use std::sync::{Arc, RwLock};
use strawberry_types::{CommandSender, ToolId};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};
use num_traits::ToPrimitive;
use strawberry_types::{
    ObjectSizeFilterMessage, ObjectSizeFilterMode, ObjectSizeFilterState, PipelineObjectMessage,
    ToolFrontend, ToolState,
};

use crate::{
    gui::{
        Button, ToggleButton,
        actions::actions,
        tool_panel,
        value_control::{ValueControl, ValueControlConfig, ValueControlEvent},
    },
    tools::{FrontendMessageHandler, ToolBase, spawn_message_processor},
};

pub struct ObjectSizeFilterView {
    state: Arc<RwLock<ObjectSizeFilterState>>,
    base: ToolBase,
    size_control: Option<Entity<ValueControl>>,
    mode_focus: tool_panel::ListFocus<usize>,
}

actions!([ObjectSizeFilterToggleActivity]);

impl Focusable for ObjectSizeFilterView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl ObjectSizeFilterView {
    const MODES: [(ObjectSizeFilterMode, &'static str); 2] = [
        (
            ObjectSizeFilterMode::RemoveSmallerThan,
            "Remove smaller than",
        ),
        (ObjectSizeFilterMode::RemoveLargerThan, "Remove larger than"),
    ];

    pub fn new(id: ToolId, sender: CommandSender, cx: &App) -> Self {
        Self {
            state: Arc::new(RwLock::new(ObjectSizeFilterState::default())),
            base: ToolBase::new(sender, id, cx.focus_handle()),
            size_control: None,
            mode_focus: tool_panel::ListFocus::new(),
        }
    }

    fn send(&self, message: PipelineObjectMessage) {
        if let Err(error) = self.base.send_to_backend(message) {
            eprintln!("Failed to update Object Size Filter: {error}");
        }
    }

    fn set_mode(&self, mode: ObjectSizeFilterMode) {
        self.send(PipelineObjectMessage::ObjectSizeFilterMessage(
            ObjectSizeFilterMessage::SetMode(mode),
        ));
    }

    fn toggle_activity_action(
        &mut self,
        _: &ObjectSizeFilterToggleActivity,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let enabled = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled;
        let _ = self.base.set_enabled(!enabled);
    }

    fn render_modes(
        &self,
        mode: ObjectSizeFilterMode,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let ids = [0, 1];
        self.mode_focus.sync(&ids, cx);
        div()
            .id("object-size-filter-modes")
            .flex()
            .flex_col()
            .gap_1()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let Some(direction) = tool_panel::ListDirection::from_key(event) else {
                    return;
                };
                let current = this.mode_focus.focused_index(&[0, 1], window);
                if let Some(index) =
                    tool_panel::next_list_index(current, Self::MODES.len(), direction)
                    && let Some(&(mode, _)) = Self::MODES.get(index)
                {
                    this.set_mode(mode);
                    this.mode_focus.focus(index, window, cx);
                    cx.stop_propagation();
                }
            }))
            .children(
                Self::MODES
                    .iter()
                    .enumerate()
                    .map(|(index, &(choice, label))| {
                        Button::new(("object-size-filter-mode", index))
                            .focus_handle(&self.mode_focus.handle(index, cx))
                            .label(label)
                            .selected(mode == choice)
                            .on_click(cx.listener(move |this, _, _, _| this.set_mode(choice)))
                    }),
            )
    }
}

impl FrontendMessageHandler for ObjectSizeFilterView {
    type State = ObjectSizeFilterState;

    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }

    fn base(&self) -> &ToolBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ToolBase {
        &mut self.base
    }

    fn extract_state(state: ToolState) -> Option<Self::State> {
        match state {
            ToolState::ObjectSizeFilterState(state) => Some(state),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, state: &Self::State, cx: &mut Context<Self>) {
        if let Some(control) = &self.size_control {
            control.update(cx, |control, cx| {
                control.set_integer_value(state.size);
                cx.notify();
            });
        }
    }
}

impl Render for ObjectSizeFilterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        tool_panel::panel()
            .key_context("ObjectSizeFilterView")
            .track_focus(&self.focus_handle(cx))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.base.select();
                    this.focus_handle(cx).focus(window, cx);
                }),
            )
            .on_action(cx.listener(Self::toggle_activity_action))
            .child(
                ToggleButton::new(
                    "Enabled".to_string(),
                    state.is_enabled,
                    move |this: &mut Self, _, _, _| {
                        let _ = this.base.set_enabled(!state.is_enabled);
                    },
                )
                .with_keyboard_shortcut("⌘T")
                .render(cx),
            )
            .child(self.render_modes(state.mode, cx))
            .children(self.size_control.clone())
    }
}

impl ToolFrontend for ObjectSizeFilterView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }
        cx.bind_keys([KeyBinding::new(
            "cmd-t",
            ObjectSizeFilterToggleActivity,
            Some("ObjectSizeFilterView"),
        )]);
        let size = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .size;
        let control = cx.new(|cx| {
            ValueControl::new_integer(
                size,
                ValueControlConfig {
                    label: "Size (pixels)".to_string(),
                    id: "object-size-filter-size".to_string(),
                    step: 1.0,
                    min: 0.0,
                    max: u32::MAX.to_f32().unwrap_or(f32::MAX),
                },
                cx,
            )
        });
        cx.subscribe(&control, |this, _, event: &ValueControlEvent, _| {
            if let Some(size) = event.integer_value {
                this.send(PipelineObjectMessage::ObjectSizeFilterMessage(
                    ObjectSizeFilterMessage::SetSize(size),
                ));
            }
        })
        .detach();
        self.size_control = Some(control);
        spawn_message_processor(self, window, cx);
        cx.notify();
    }
}
