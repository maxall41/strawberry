use crate::gui::actions::actions;
use crate::gui::{Button, ButtonVariant};
use strawberry_types::RgbaColor;

use crate::gui::{theme, tool_panel};
// Implements UI for counter tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, ElementId, FocusHandle, Focusable, KeyBinding, MouseDownEvent, Window, div,
    prelude::*,
};
use rfd::FileDialog;

use crate::gui::{ToggleButton, icon::Icon};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    CountData, CountMessage, CounterMode, CounterState, CounterTool, PipelineCommand,
    PipelineObjectMessage, ToolState, consts::CIRCLE_COLORS,
};

use strawberry_types::ToolFrontend;

pub struct CounterView {
    pub state: Arc<RwLock<CounterState>>,
    pub next_count_type_id: Arc<RwLock<u32>>,
    pub last_directory: Arc<RwLock<Option<PathBuf>>>,
    pub image_directory: Arc<RwLock<Option<PathBuf>>>,
    pub base: crate::tools::base::ToolBase,
    count_types_scroll: gpui::ScrollHandle,
}

impl Focusable for CounterView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}

impl CounterView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        image_directory: Arc<RwLock<Option<PathBuf>>>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(CounterState::default())),
            count_types_scroll: gpui::ScrollHandle::new(),
            next_count_type_id: Arc::new(RwLock::new(0)),
            last_directory: Arc::new(RwLock::new(None)),
            image_directory,
            base: crate::tools::base::ToolBase::new(
                command_sender,
                strawberry_types::BackendToolType::Counter,
                cx.focus_handle(),
            ),
        }
    }

    fn process_messages(&mut self, window: &Window, cx: &Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn save_counts(&self, window: &Window, cx: &Context<Self>) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            return;
        };

        let image_directory = self.image_directory.clone();

        cx.spawn_in(window, async move |this, cx| {
            let default_dir = image_directory
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~/".to_string());

            println!("default_dir {default_dir:?}");

            let file = FileDialog::new()
                .add_filter("xml", &["xml"])
                .set_directory(&default_dir)
                .save_file();

            if let Some(file) = file
                && let Err(e) = this.update(cx, |this, _cx| {
                    if let Some(sender) =
                        this.base.send_channels.get(usize::from(current_tool_index))
                    {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::SaveCounts(file)),
                        )) {
                            eprintln!("Failed to send save counts command: {e}");
                        }
                    } else {
                        eprintln!("Invalid tool index: {current_tool_index}");
                    }
                })
            {
                eprintln!("Failed to update count types: {e}");
            }
        })
        .detach();
    }

    fn load_counts(&self, window: &Window, cx: &Context<Self>) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            return;
        };

        let last_directory = self.last_directory.clone();
        let image_directory = self.image_directory.clone();

        cx.spawn_in(window, async move |this, cx| {
            let default_dir = last_directory
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                .or_else(|| {
                    image_directory
                        .read()
                        .ok()
                        .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                })
                .unwrap_or_else(|| "~/".to_string());

            let file = FileDialog::new()
                .add_filter("xml", &["xml"])
                .set_directory(&default_dir)
                .pick_file();

            if let Some(file) = file {
                // Store the directory for future save/load operations
                if let Some(parent) = file.parent()
                    && let Ok(mut dir) = last_directory.write()
                {
                    *dir = Some(parent.to_path_buf());
                }

                if let Err(e) = this.update(cx, |this, _cx| {
                    if let Some(sender) =
                        this.base.send_channels.get(usize::from(current_tool_index))
                    {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::LoadCounts(file)),
                        )) {
                            eprintln!("Failed to send load counts command: {e}");
                        }
                    } else {
                        eprintln!("Invalid tool index: {current_tool_index}");
                    }
                }) {
                    eprintln!("Failed to update count types: {e}");
                }
            }
        })
        .detach();
    }

    fn add_count_type(&self) {
        let Ok(hint) = self.next_count_type_id.read() else {
            return;
        };
        let Ok(state) = self.state.read() else {
            return;
        };
        let Ok(first_id) = u16::try_from(*hint) else {
            eprintln!("No count type IDs available");
            return;
        };
        let Some(new_id) = (first_id..=u16::MAX).find(|id| !state.manual_counts.contains_key(id))
        else {
            eprintln!("No count type IDs available");
            return;
        };
        drop(state);
        drop(hint);

        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::NewCountType(new_id)),
            )) {
                eprintln!("Failed to send new count type command: {e}");
                return;
            }
            println!("Sent new count type message!");
        } else {
            eprintln!(
                "Failed to send new count type message. Invalid tool index: {current_tool_index}"
            );
            return;
        }

        // Reserve the ID until the next backend state arrives.
        if let Ok(mut next_id) = self.next_count_type_id.write() {
            *next_id = u32::from(new_id).saturating_add(1);
        }

        println!("Added count type {new_id}");
    }

    fn delete_count_type(&self, count_type_id: u16) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::DeleteCountType(count_type_id)),
            )) {
                eprintln!("Failed to send delete count type command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }

        // Clear selection if this was the selected type
        if let Ok(state) = self.state.read()
            && state.active_count_type == Some(count_type_id)
            && let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index))
        {
            let mut remaining_types: Vec<u16> = state
                .manual_counts
                .keys()
                .filter(|&&id| id != count_type_id)
                .copied()
                .collect();
            remaining_types.sort_unstable();
            let new_active = remaining_types
                .iter()
                .find(|&&id| id > count_type_id)
                .or_else(|| remaining_types.iter().rev().find(|&&id| id < count_type_id))
                .copied();

            if let Some(new_active_id) = new_active
                && let Err(e) = sender.send(PipelineCommand::PipelineObject(
                    PipelineObjectMessage::CountMessage(CountMessage::SetActiveCountType(
                        new_active_id,
                    )),
                ))
            {
                eprintln!("Failed to send set active count type command: {e}");
            }
        }
    }

    fn select_count_type(&self, count_type_id: u16, _window: &Window, _cx: &Context<Self>) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetActiveCountType(
                    count_type_id,
                )),
            )) {
                eprintln!("Failed to send set active count type command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn copy_to_manual(&self) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            eprintln!("No current tool index available for copy to manual");
            return;
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::CopyToManual),
            )) {
                eprintln!("Failed to send copy to manual command: {e}");
            }
        } else {
            eprintln!("No sender available for copy to manual command");
        }
    }

    fn toggle_count_type_visibility(&self, count_type_id: u16) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            eprintln!("No current tool index available for toggle count type visibility");
            return;
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::ToggleCountTypeVisibility(
                    count_type_id,
                )),
            )) {
                eprintln!("Failed to send toggle count type visibility command: {e}");
            }
        } else {
            eprintln!("No sender available for toggle count type visibility command");
        }
    }

    fn set_tool(&self, tool: Option<CounterTool>, _window: &Window, _cx: &Context<Self>) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetActiveTool(tool)),
            )) {
                eprintln!("Failed to send set active tool command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn toggle_enabled(&self, current_value: bool, _window: &Window, _cx: &Context<Self>) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ToggleActive(!current_value),
            )) {
                eprintln!("Failed to send toggle enabled command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn toggle_activity_action(
        &mut self,
        _: &CounterToggleActivity,
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

    fn place_tool_action(
        &mut self,
        _: &CounterPlaceTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_tool = match self.state.read() {
            Ok(state) => state.active_tool,
            Err(_) => return,
        };

        if current_tool == Some(CounterTool::Place) {
            self.set_tool(None, window, cx);
        } else {
            self.set_tool(Some(CounterTool::Place), window, cx);
        }
    }

    fn delete_tool_action(
        &mut self,
        _: &CounterDeleteTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_tool = match self.state.read() {
            Ok(state) => state.active_tool,
            Err(_) => return,
        };

        if current_tool == Some(CounterTool::Delete) {
            self.set_tool(None, window, cx);
        } else {
            self.set_tool(Some(CounterTool::Delete), window, cx);
        }
    }

    fn lasso_tool_action(
        &mut self,
        _: &CounterLassoTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_tool = match self.state.read() {
            Ok(state) => state.active_tool,
            Err(_) => return,
        };

        if current_tool == Some(CounterTool::LassoDelete) {
            self.set_tool(None, window, cx);
        } else {
            self.set_tool(Some(CounterTool::LassoDelete), window, cx);
        }
    }

    fn review_tool_action(
        &mut self,
        _: &CounterReviewTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_tool = match self.state.read() {
            Ok(state) => state.active_tool,
            Err(_) => return,
        };

        if current_tool == Some(CounterTool::Review) {
            self.set_tool(None, window, cx);
        } else {
            self.set_tool(Some(CounterTool::Review), window, cx);
        }
    }

    fn review_previous_action(
        &mut self,
        _: &CounterReviewPrevious,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_review_navigate_previous();
        cx.notify();
    }

    fn review_next_action(
        &mut self,
        _: &CounterReviewNext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_review_navigate_next();
        cx.notify();
    }

    fn delete_count_action(
        &mut self,
        _: &CounterDeleteCount,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.send_delete_current_count();
        cx.notify();
    }

    fn save_counts_action(
        &mut self,
        _: &CounterSaveCounts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_counts(window, cx);
    }

    fn load_counts_action(
        &mut self,
        _: &CounterLoadCounts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_counts(window, cx);
    }

    fn new_count_type_action(
        &mut self,
        _: &CounterNewCountType,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_count_type();
        cx.notify();
    }

    fn select_count_type_by_position(&self, position: usize, window: &Window, cx: &Context<Self>) {
        let Ok(state) = self.state.read() else {
            return;
        };

        let mut sorted_counts: Vec<_> = state.manual_counts.iter().collect();
        sorted_counts.sort_by_key(|(id, _)| **id);

        if let Some(&(&count_type_id, _)) = sorted_counts.get(position) {
            drop(state); // Release the read lock before calling select_count_type
            self.select_count_type(count_type_id, window, cx);
        }
    }

    fn select_count_type_1_action(
        &mut self,
        _: &CounterSelectType1,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(0, window, cx);
    }

    fn select_count_type_2_action(
        &mut self,
        _: &CounterSelectType2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(1, window, cx);
    }

    fn select_count_type_3_action(
        &mut self,
        _: &CounterSelectType3,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(2, window, cx);
    }

    fn select_count_type_4_action(
        &mut self,
        _: &CounterSelectType4,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(3, window, cx);
    }

    fn select_count_type_5_action(
        &mut self,
        _: &CounterSelectType5,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(4, window, cx);
    }

    fn select_count_type_6_action(
        &mut self,
        _: &CounterSelectType6,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(5, window, cx);
    }

    fn select_count_type_7_action(
        &mut self,
        _: &CounterSelectType7,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(6, window, cx);
    }

    fn select_count_type_8_action(
        &mut self,
        _: &CounterSelectType8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(7, window, cx);
    }

    fn select_count_type_9_action(
        &mut self,
        _: &CounterSelectType9,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_count_type_by_position(8, window, cx);
    }

    fn set_mode(&self, mode: CounterMode) {
        if let Err(error) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::CountMessage(CountMessage::SetMode(mode)),
        )) {
            eprintln!("Failed to set counter mode: {error}");
        }
    }

    fn render_status(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".into(),
            enabled,
            move |this: &mut Self, _, window, cx| this.toggle_enabled(enabled, window, cx),
        )
        .with_keyboard_shortcut("⌘T")
        .with_no_margin()
        .render(cx)
    }

    fn render_mode_toggle(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        tool_panel::segmented().children(
            [
                (CounterMode::Manual, "Manual"),
                (CounterMode::Auto, "Automatic"),
            ]
            .into_iter()
            .map(|(mode, label)| {
                Button::new(ElementId::Name(format!("counter-mode-{label}").into()))
                    .variant(ButtonVariant::Ghost)
                    .selected(state.mode == mode)
                    .flex_1()
                    .min_w_0()
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, _| this.set_mode(mode)))
            }),
        )
    }

    fn render_count_type_item(
        count_type_id: u16,
        name: &str,
        count_data: &[CountData],
        is_selected: bool,
        is_visible: bool,
        position: usize,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let index = usize::from(count_type_id)
            .checked_rem(CIRCLE_COLORS.len())
            .unwrap_or_default();
        let color: gpui::Rgba = CIRCLE_COLORS
            .get(index)
            .copied()
            .unwrap_or(RgbaColor::WHITE)
            .into();
        let shortcut = if position < 9 {
            format!("Select count type (⌘{})", position.saturating_add(1))
        } else {
            "Select count type".into()
        };
        tool_panel::row()
            .id(("counter-type-row", usize::from(count_type_id)))
            .w_full()
            .role(gpui::Role::RadioButton)
            .aria_label(format!("Select {name}"))
            .aria_toggled(if is_selected {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .focusable()
            .cursor_pointer()
            .hover(|style| style.bg(theme::SURFACE_ALT))
            .focus_visible(|style| style.bg(theme::SURFACE_ALT))
            .tooltip(move |_, cx| {
                cx.new(|_| crate::gui::tooltip::TooltipView {
                    text: shortcut.clone(),
                })
                .into()
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.select_count_type(count_type_id, window, cx);
            }))
            .child(tool_panel::radio_indicator(is_selected, color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .text_xs()
                    .truncate()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(name.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .min_w(gpui::px(28.0))
                    .text_right()
                    .text_xs()
                    .text_color(if is_selected {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    })
                    .child(count_data.len().to_string()),
            )
            .child(
                Button::new(("count-type-visibility", usize::from(count_type_id)))
                    .variant(ButtonVariant::Ghost)
                    .icon(
                        if is_visible {
                            Icon::eye_open()
                        } else {
                            Icon::eye_closed()
                        },
                        if is_visible {
                            "Hide count type"
                        } else {
                            "Show count type"
                        },
                    )
                    .tooltip_text(if is_visible {
                        "Hide count type"
                    } else {
                        "Show count type"
                    })
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.toggle_count_type_visibility(count_type_id)
                    })),
            )
            .child(
                Button::new(("count-type-delete", usize::from(count_type_id)))
                    .variant(ButtonVariant::Delete)
                    .aria_label("Delete count type")
                    .tooltip_text("Delete count type")
                    .on_click(
                        cx.listener(move |this, _, _, _| this.delete_count_type(count_type_id)),
                    ),
            )
    }

    fn render_count_types_list(
        state: &CounterState,
        scroll: &gpui::ScrollHandle,
        owner: &FocusHandle,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut list = tool_panel::list_for_tool("counter-types-list", scroll, owner)
            .role(gpui::Role::RadioGroup);
        if state.manual_counts.is_empty() {
            list = list.child(tool_panel::empty("No count types"));
        }
        let mut types: Vec<_> = state.manual_counts.iter().collect();
        types.sort_by_key(|(id, _)| **id);
        for (position, (&id, counts)) in types.into_iter().enumerate() {
            let name = state
                .count_type_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("Type {}", position.saturating_add(1)));
            list = list.child(Self::render_count_type_item(
                id,
                &name,
                counts,
                state.active_count_type == Some(id),
                state
                    .count_type_visibility
                    .get(&id)
                    .copied()
                    .unwrap_or(true),
                position,
                cx,
            ));
        }
        list
    }

    fn render_add_button(cx: &Context<Self>) -> impl IntoElement + use<> {
        Button::new("add-count-type-button")
            .compact()
            .w_full()
            .child(Icon::plus())
            .label("Add type")
            .tooltip_text("⌘N")
            .on_click(cx.listener(|this, _, _, _| this.add_count_type()))
    }

    fn render_tool_button(
        tool: CounterTool,
        label: &'static str,
        is_active: bool,
        shortcut: &'static str,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(ElementId::Name(format!("tool-button-{tool:?}").into()))
            .variant(ButtonVariant::Ghost)
            .compact()
            .px_1()
            .selected(is_active)
            .flex_1()
            .min_w_0()
            .label(label)
            .tooltip_text(shortcut)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_tool(if is_active { None } else { Some(tool) }, window, cx)
            }))
    }

    fn render_tools_bar(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        tool_panel::segmented().children(
            [
                (CounterTool::Place, "Place", "⌘W"),
                (CounterTool::Delete, "Delete", "⌘E"),
                (CounterTool::LassoDelete, "Lasso", "⌘A"),
                (CounterTool::Review, "Review", "⌘R"),
            ]
            .into_iter()
            .map(|(tool, label, shortcut)| {
                Self::render_tool_button(tool, label, state.active_tool == Some(tool), shortcut, cx)
            }),
        )
    }

    fn render_storage_buttons(cx: &Context<Self>) -> impl IntoElement + use<> {
        tool_panel::section("Count data").child(
            tool_panel::file_actions()
                .child(
                    Button::new("counter-load")
                        .compact()
                        .flex_1()
                        .min_w_0()
                        .label("Load counts")
                        .tooltip_text("⌘L")
                        .on_click(cx.listener(|this, _, window, cx| this.load_counts(window, cx))),
                )
                .child(
                    Button::new("counter-save")
                        .compact()
                        .flex_1()
                        .min_w_0()
                        .label("Save counts")
                        .tooltip_text("⌘S")
                        .on_click(cx.listener(|this, _, window, cx| this.save_counts(window, cx))),
                ),
        )
    }

    fn render_manual_ui(
        &self,
        state: &CounterState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut ui = div()
            .flex()
            .flex_col()
            .gap_4()
            .min_w_0()
            .child(tool_panel::section("Annotate").child(Self::render_tools_bar(state, cx)))
            .child(
                tool_panel::section("Count types")
                    .child(Self::render_count_types_list(
                        state,
                        &self.count_types_scroll,
                        &self.base.focus_handle,
                        cx,
                    ))
                    .child(Self::render_add_button(cx)),
            );
        if state.active_tool == Some(CounterTool::Review) {
            ui = ui.child(Self::render_review_ui(state, cx));
        }
        ui
    }

    fn render_auto_ui(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        tool_panel::section("Detection")
            .child(
                div()
                    .text_2xl()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(state.auto_counts.len().to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("Detected counts"),
            )
            .child(
                Button::new("copy-to-manual")
                    .variant(ButtonVariant::Success)
                    .large()
                    .w_full()
                    .label("Copy to manual")
                    .on_click(cx.listener(|this, _, _, _| this.copy_to_manual())),
            )
    }

    fn render_review_ui(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let counts = state
            .active_count_type
            .and_then(|id| state.manual_counts.get(&id))
            .map_or(&[][..], Vec::as_slice);
        let mut review = tool_panel::section("Review");
        if counts.is_empty() {
            return review.child(tool_panel::empty("No counts to review"));
        }
        let index = state.review_current_index;
        let mut summary = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .text_xs()
            .text_color(theme::TEXT_MUTED)
            .child(format!("{} / {}", index.saturating_add(1), counts.len()));
        if let Some(count) = counts.get(index) {
            summary = summary.child(format!("({}, {})", count.position.0, count.position.1));
        }
        review = review
            .child(summary)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("prev-count")
                            .compact()
                            .flex_1()
                            .min_w_0()
                            .label("Previous")
                            .tooltip_text("Previous count (←)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send_review_navigate_previous();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("next-count")
                            .compact()
                            .flex_1()
                            .min_w_0()
                            .label("Next")
                            .tooltip_text("Next count (→)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send_review_navigate_next();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Button::new("delete-current-count")
                    .variant(ButtonVariant::Danger)
                    .compact()
                    .w_full()
                    .label("Delete count")
                    .tooltip_text("⌘D")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.send_delete_current_count();
                        cx.notify();
                    })),
            );
        review
    }

    // Review functionality methods - now sends messages to backend
    fn send_review_navigate_previous(&self) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::ReviewNavigatePrevious),
            )) {
                eprintln!("Failed to send review navigate previous command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn send_review_navigate_next(&self) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::ReviewNavigateNext),
            )) {
                eprintln!("Failed to send review navigate next command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn send_delete_current_count(&self) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => {
                if let Some(index) = *guard {
                    index
                } else {
                    eprintln!("No current tool index available");
                    return;
                }
            }
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::DeleteCurrentCount),
            )) {
                eprintln!("Failed to send delete current count command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }
}

actions!([
    CounterToggleActivity,
    CounterPlaceTool,
    CounterDeleteTool,
    CounterLassoTool,
    CounterReviewTool,
    CounterReviewPrevious,
    CounterReviewNext,
    CounterDeleteCount,
    CounterSaveCounts,
    CounterLoadCounts,
    CounterNewCountType,
    CounterSelectType1,
    CounterSelectType2,
    CounterSelectType3,
    CounterSelectType4,
    CounterSelectType5,
    CounterSelectType6,
    CounterSelectType7,
    CounterSelectType8,
    CounterSelectType9
]);

impl FrontendMessageHandler for CounterView {
    type State = CounterState;

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
            ToolState::CounterState(s) => Some(s),
            _ => None,
        }
    }

    fn on_state_changed(&mut self, new_state: &CounterState, _cx: &mut Context<Self>) {
        if let Ok(mut next_id) = self.next_count_type_id.write() {
            *next_id = new_state
                .manual_counts
                .keys()
                .max()
                .map_or(0, |&k| u32::from(k).saturating_add(1));
        }
    }
}

impl Render for CounterView {
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
                    .w_full()
                    .text_sm()
                    .p_4()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read counter state");
            }
        };

        tool_panel::panel()
            .key_context("CounterView")
            .track_focus(&self.focus_handle(cx))
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
            .on_action(cx.listener(Self::place_tool_action))
            .on_action(cx.listener(Self::delete_tool_action))
            .on_action(cx.listener(Self::lasso_tool_action))
            .on_action(cx.listener(Self::review_tool_action))
            .on_action(cx.listener(Self::review_previous_action))
            .on_action(cx.listener(Self::review_next_action))
            .on_action(cx.listener(Self::delete_count_action))
            .on_action(cx.listener(Self::save_counts_action))
            .on_action(cx.listener(Self::load_counts_action))
            .on_action(cx.listener(Self::new_count_type_action))
            .on_action(cx.listener(Self::select_count_type_1_action))
            .on_action(cx.listener(Self::select_count_type_2_action))
            .on_action(cx.listener(Self::select_count_type_3_action))
            .on_action(cx.listener(Self::select_count_type_4_action))
            .on_action(cx.listener(Self::select_count_type_5_action))
            .on_action(cx.listener(Self::select_count_type_6_action))
            .on_action(cx.listener(Self::select_count_type_7_action))
            .on_action(cx.listener(Self::select_count_type_8_action))
            .on_action(cx.listener(Self::select_count_type_9_action))
            .child(Self::render_status(&state, cx))
            .child(Self::render_mode_toggle(&state, cx))
            .child(match state.mode {
                CounterMode::Manual => div().child(self.render_manual_ui(&state, cx)),
                CounterMode::Auto => div().child(Self::render_auto_ui(&state, cx)),
            })
            .child(Self::render_storage_buttons(cx))
    }
}

impl ToolFrontend for CounterView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect() {
            return;
        }

        cx.bind_keys([
            KeyBinding::new("cmd-t", CounterToggleActivity, Some("CounterView")),
            KeyBinding::new("cmd-w", CounterPlaceTool, Some("CounterView")),
            KeyBinding::new("cmd-e", CounterDeleteTool, Some("CounterView")),
            KeyBinding::new("cmd-a", CounterLassoTool, Some("CounterView")),
            KeyBinding::new("cmd-r", CounterReviewTool, Some("CounterView")),
            KeyBinding::new("left", CounterReviewPrevious, Some("CounterView")),
            KeyBinding::new("right", CounterReviewNext, Some("CounterView")),
            KeyBinding::new("cmd-d", CounterDeleteCount, Some("CounterView")),
            KeyBinding::new("cmd-s", CounterSaveCounts, Some("CounterView")),
            KeyBinding::new("cmd-l", CounterLoadCounts, Some("CounterView")),
            KeyBinding::new("cmd-n", CounterNewCountType, Some("CounterView")),
            KeyBinding::new("cmd-1", CounterSelectType1, Some("CounterView")),
            KeyBinding::new("cmd-2", CounterSelectType2, Some("CounterView")),
            KeyBinding::new("cmd-3", CounterSelectType3, Some("CounterView")),
            KeyBinding::new("cmd-4", CounterSelectType4, Some("CounterView")),
            KeyBinding::new("cmd-5", CounterSelectType5, Some("CounterView")),
            KeyBinding::new("cmd-6", CounterSelectType6, Some("CounterView")),
            KeyBinding::new("cmd-7", CounterSelectType7, Some("CounterView")),
            KeyBinding::new("cmd-8", CounterSelectType8, Some("CounterView")),
            KeyBinding::new("cmd-9", CounterSelectType9, Some("CounterView")),
        ]);

        self.process_messages(window, cx);
    }
}
