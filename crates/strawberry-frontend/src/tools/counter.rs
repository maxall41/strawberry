use crate::gui::actions::actions;
use crate::gui::{Button, ButtonVariant};
use num_traits::ToPrimitive;

use crate::gui::theme;
// Implements UI for counter tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    App, Context, ElementId, FocusHandle, Focusable, KeyBinding, MouseDownEvent, SharedString,
    Window, div, prelude::*,
};
use rfd::FileDialog;

use crate::gui::{
    ToggleButton,
    icon::Icon,
    value_control::{ValueControl, ValueControlConfig},
};
use crate::tools::{FrontendMessageHandler, spawn_message_processor};
use strawberry_types::{
    CountData, CountMessage, CounterMode, CounterState, CounterTool, FilterProxy, PipelineCommand,
    PipelineObjectMessage, ToolState,
    consts::{CIRCLE_COLORS, image_rgba_to_gpui_rgba},
};

use strawberry_types::ToolFrontend;

pub struct CounterView {
    pub state: Arc<RwLock<CounterState>>,
    pub next_count_type_id: Arc<RwLock<u32>>,
    pub last_directory: Arc<RwLock<Option<PathBuf>>>,
    pub image_directory: Arc<RwLock<Option<PathBuf>>>,
    pub base: crate::tools::base::ToolBase,
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

    fn toggle_mode(&self, _window: &Window, _cx: &Context<Self>) {
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

        let new_mode = match self.state.read() {
            Ok(state) => match state.mode {
                CounterMode::Manual => CounterMode::Auto,
                CounterMode::Auto => CounterMode::Manual,
            },
            Err(e) => {
                eprintln!("Failed to read state for mode toggle: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(usize::from(current_tool_index)) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetMode(new_mode)),
            )) {
                eprintln!("Failed to send set mode command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    // Subheader component
    fn render_subheader() -> impl IntoElement + use<> {
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::BOLD)
            .child("Counts:")
    }

    // Status component showing enabled/disabled state
    fn render_status(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
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
        .with_no_margin()
        .render(cx)
    }

    // Mode toggle component
    fn render_mode_toggle(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        div().flex().items_center().gap_2().child("Mode:").child(
            Button::new("counter-mode")
                .label(match state.mode {
                    CounterMode::Manual => "Manual",
                    CounterMode::Auto => "Auto",
                })
                .tooltip_text("Switch counting mode")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_mode(window, cx);
                })),
        )
    }

    // Individual count type item component
    fn render_count_type_item(
        count_type_id: u16,
        name: &str,
        count_data: &[CountData],
        is_selected: bool,
        is_visible: bool,
        position: Option<usize>,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let ii = usize::from(count_type_id)
            .checked_rem(CIRCLE_COLORS.len())
            .unwrap_or_default();
        let color =
            image_rgba_to_gpui_rgba(CIRCLE_COLORS.get(ii).copied().unwrap_or(CIRCLE_COLORS[0]));
        div()
            .flex()
            .items_center()
            .gap_2()
            .mb_1()
            .bg(theme::CONTROL)
            .rounded_md()
            .child(div().bg(color).w_2().h_full().rounded_l_md())
            .child(
                // X button
                Button::new(("count-type-delete", usize::from(count_type_id)))
                    .variant(ButtonVariant::Delete)
                    .m_2()
                    .aria_label("Delete count type")
                    .on_click(cx.listener(
                        move |this: &mut Self,
                              _event: &gpui::ClickEvent,
                              _window: &mut Window,
                              _cx: &mut Context<Self>| {
                            this.delete_count_type(count_type_id);
                        },
                    )),
            )
            .child(
                // Visibility button
                Button::new(("count-type-visibility", usize::from(count_type_id)))
                    .variant(ButtonVariant::Ghost)
                    .m_1()
                    .icon(
                        if is_visible {
                            Icon::eye_open().size(21.0)
                        } else {
                            Icon::eye_closed().size(21.0)
                        },
                        if is_visible {
                            "Hide count type"
                        } else {
                            "Show count type"
                        },
                    )
                    .on_click(cx.listener(
                        move |this: &mut Self,
                              _event: &gpui::ClickEvent,
                              _window: &mut Window,
                              _cx: &mut Context<Self>| {
                            this.toggle_count_type_visibility(count_type_id);
                        },
                    )),
            )
            .child(
                // Type label and count
                div()
                    .m_2()
                    .flex_1()
                    .child(format!("{} ({})", name, count_data.len())),
            )
            .child(
                // Selection indicator
                div()
                    .m_2()
                    .w_4()
                    .h_4()
                    .id(ElementId::Name(SharedString::from(format!(
                        "count-type-{count_type_id}-selection-circle"
                    ))))
                    .when_some(position.filter(|&pos| pos <= 8), |this, position| {
                        let pos = position.saturating_add(1);
                        this.tooltip(move |_window, cx| {
                            let tooltip_view = crate::gui::tooltip::TooltipView {
                                text: format!("⌘{pos}"),
                            };
                            cx.new(|_cx| tooltip_view).into()
                        })
                    })
                    .rounded_full()
                    .bg(if is_selected {
                        color
                    } else {
                        theme::CONTROL_HOVER
                    })
                    .when(!is_selected, |el| {
                        el.cursor_pointer()
                            .hover(|style| style.bg(theme::lighten(theme::CONTROL_HOVER, 0.15)))
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut Self,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<Self>| {
                                this.select_count_type(count_type_id, window, cx);
                            },
                        ),
                    ),
            )
    }

    // Count types list component
    fn render_count_types_list(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let Ok(state) = self.state.read().map(|state| state.clone()) else {
            return div().child("Error reading state");
        };

        let selected_type = state.active_count_type;

        div().flex().flex_col().gap_1().children({
            let mut sorted_counts: Vec<_> = state.manual_counts.iter().collect();
            sorted_counts.sort_by_key(|(id, _)| **id);
            sorted_counts
                .into_iter()
                .enumerate()
                .map(|(position, (&count_type_id, count_data))| {
                    let is_selected = selected_type == Some(count_type_id);
                    let is_visible = state
                        .count_type_visibility
                        .get(&count_type_id)
                        .unwrap_or(&true);
                    let name = state
                        .count_type_names
                        .get(&count_type_id)
                        .cloned()
                        .unwrap_or_else(|| format!("Type {}", position.saturating_add(1)));
                    Self::render_count_type_item(
                        count_type_id,
                        &name,
                        count_data,
                        is_selected,
                        *is_visible,
                        Some(position),
                        cx,
                    )
                })
        })
    }

    // Plus button component
    fn render_add_button(cx: &Context<Self>) -> impl IntoElement + use<> {
        Button::new("add-count-type-button")
            .large()
            .w_full()
            .child(Icon::plus().size(14.))
            .aria_label("Add count type")
            .tooltip_text("⌘N")
            .on_click(cx.listener(
                |this: &mut Self,
                 _event: &gpui::ClickEvent,
                 _window: &mut Window,
                 _cx: &mut Context<Self>| {
                    this.add_count_type();
                },
            ))
    }

    // Tool button component
    fn render_tool_button(
        tool: CounterTool,
        label: String,
        is_active: bool,
        keyboard_shortcut: &str,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(ElementId::Name(SharedString::from(format!(
            "tool-button-{tool:?}"
        ))))
        .selected(is_active)
        .large()
        .px_2()
        .flex_1()
        .min_w_0()
        .rounded_none()
        .border_0()
        .label(label)
        .tooltip_text(keyboard_shortcut.to_string())
        .on_click(cx.listener(
            move |this: &mut Self,
                  _event: &gpui::ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
                if is_active {
                    this.set_tool(None, window, cx);
                } else {
                    this.set_tool(Some(tool), window, cx);
                }
            },
        ))
    }

    // Tools subheader component
    fn render_tools_subheader() -> impl IntoElement + use<> {
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::BOLD)
            .child("Tools:")
    }

    // Tools bar component
    fn render_tools_bar(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let Ok(state) = self.state.read().map(|state| state.clone()) else {
            return div().child("Error reading state");
        };
        let current_tool = state.active_tool;

        div()
            .flex()
            .bg(theme::SURFACE)
            .rounded_md()
            .overflow_hidden()
            .child(
                Self::render_tool_button(
                    CounterTool::Place,
                    "Place".to_string(),
                    current_tool == Some(CounterTool::Place),
                    "⌘W",
                    cx,
                )
                .rounded_l_md(),
            )
            .child(Self::render_tool_button(
                CounterTool::Delete,
                "Delete".to_string(),
                current_tool == Some(CounterTool::Delete),
                "⌘E",
                cx,
            ))
            .child(Self::render_tool_button(
                CounterTool::LassoDelete,
                "Lasso".to_string(),
                current_tool == Some(CounterTool::LassoDelete),
                "⌘A",
                cx,
            ))
            .child(
                Self::render_tool_button(
                    CounterTool::Review,
                    "Review".to_string(),
                    current_tool == Some(CounterTool::Review),
                    "⌘R",
                    cx,
                )
                .rounded_r_md(),
            )
    }

    // Save button component
    fn render_storage_buttons(cx: &Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .gap_2()
            .child(
                Button::new("save-button")
                    .variant(ButtonVariant::Success)
                    .large()
                    .label("Save")
                    .tooltip_text("⌘S")
                    .on_click(cx.listener(
                        |this: &mut Self,
                         _event: &gpui::ClickEvent,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
                            this.save_counts(window, cx);
                        },
                    )),
            )
            .child(
                Button::new("load-button")
                    .variant(ButtonVariant::Primary)
                    .large()
                    .label("Load")
                    .tooltip_text("⌘L")
                    .on_click(cx.listener(
                        |this: &mut Self,
                         _event: &gpui::ClickEvent,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
                            this.load_counts(window, cx);
                        },
                    )),
            )
    }

    // Manual UI rendering
    fn render_manual_ui(
        &self,
        state: &CounterState,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut ui = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Self::render_subheader())
                    .when(!state.manual_counts.is_empty(), |el| {
                        el.child(self.render_count_types_list(cx))
                    })
                    .child(Self::render_add_button(cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Self::render_tools_subheader())
                    .child(self.render_tools_bar(cx)),
            );

        // Add review UI when Review tool is active
        if state.active_tool == Some(CounterTool::Review) {
            ui = ui.child(Self::render_review_ui(state, cx));
        }

        ui
    }

    // Auto UI rendering (placeholder)
    fn render_auto_ui(state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Auto Detection"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .mb_3()
                    .child("Detected Counts:")
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme::CONTROL)
                            .child(format!("{}", state.auto_counts.len())),
                    ),
            )
            .child(Self::render_filters_section(state, cx))
    }

    fn render_filters_section(
        state: &CounterState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_md()
            .bg(theme::SURFACE_DEEP)
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_lg()
                    .child("Filters"),
            )
            .when(state.filter_proxys.is_empty(), |el| {
                el.child(
                    div()
                        .text_sm()
                        .text_color(theme::TEXT_MUTED)
                        .child("No filters available"),
                )
            })
            .when(!state.filter_proxys.is_empty(), |el| {
                el.child(
                    div().flex().flex_col().gap_2().children(
                        state
                            .filter_proxys
                            .iter()
                            .enumerate()
                            .map(|(index, filter_proxy)| {
                                Self::render_filter_item(filter_proxy, index, cx)
                            }),
                    ),
                )
            })
    }

    fn render_filter_item(
        filter_proxy: &FilterProxy,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_enabled = filter_proxy.is_enabled;
        let Ok(filter_index) = u16::try_from(index) else {
            return div().into_any_element();
        };

        let is_size_filter = filter_proxy.name == "Filter by Size";

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(if is_enabled {
                theme::SURFACE_ALT
            } else {
                theme::SURFACE
            })
            .border_1()
            .border_color(if is_enabled {
                theme::SUCCESS
            } else {
                theme::CONTROL
            })
            .child(
                Button::new(("count-filter", usize::from(filter_index)))
                    .variant(ButtonVariant::Ghost)
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .child(filter_proxy.name.clone()),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .bg(if is_enabled {
                                        theme::SUCCESS
                                    } else {
                                        theme::DANGER
                                    })
                                    .child(if is_enabled { "ACTIVE" } else { "DISABLED" }),
                            ),
                    )
                    .on_click(cx.listener(
                        move |this: &mut Self,
                              _event: &gpui::ClickEvent,
                              _window: &mut Window,
                              _cx: &mut Context<Self>| {
                            this.toggle_filter(filter_index, !is_enabled);
                        },
                    )),
            )
            .when(is_size_filter, |el| {
                let Some(current_value) = filter_proxy.values.first().map_or(Some(10.0), |value| {
                    value.to_f32().filter(|value| value.is_finite())
                }) else {
                    return el.child("Minimum object size cannot be displayed");
                };

                // Create a ValueControl for this filter
                let control = cx.new(|cx| {
                    ValueControl::new(
                        format!("{current_value:.0}"),
                        current_value,
                        ValueControlConfig {
                            label: "Minimum Object Size".to_string(),
                            step: 1.0,
                            id: format!("size_filter_{filter_index}"),
                            min: 1.0,
                            max: 10000.0,
                        },
                        cx,
                    )
                });

                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .pt_2()
                        .border_t_1()
                        .border_color(theme::CONTROL)
                        .child(control),
                )
            })
            .into_any_element()
    }

    fn toggle_filter(&self, filter_index: u16, enabled: bool) {
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
                PipelineObjectMessage::CountMessage(CountMessage::ToggleFilter(
                    filter_index,
                    enabled,
                )),
            )) {
                eprintln!("Failed to send toggle filter command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn render_review_ui(state: &CounterState, cx: &Context<Self>) -> impl IntoElement + use<> {
        let current_index = state.review_current_index;

        let review_counts = state
            .active_count_type
            .and_then(|id| state.manual_counts.get(&id))
            .map_or(&[][..], Vec::as_slice);

        if review_counts.is_empty() {
            return div().p_3().bg(theme::SURFACE).rounded_md().child(
                div()
                    .text_center()
                    .text_sm()
                    .child("No counts to review. Add some counts first."),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(theme::SURFACE)
            .rounded_md()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_sm()
                            .child("Review Mode"),
                    )
                    .child(div().text_sm().child(format!(
                        "{} / {}",
                        current_index.saturating_add(1),
                        review_counts.len()
                    ))),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("prev-count")
                            .label("← Prev")
                            .tooltip_text("←")
                            .on_click(cx.listener(
                                |this: &mut Self,
                                 _event: &gpui::ClickEvent,
                                 _window: &mut Window,
                                 cx: &mut Context<Self>| {
                                    this.send_review_navigate_previous();
                                    cx.notify();
                                },
                            )),
                    )
                    .child(
                        Button::new("next-count")
                            .label("Next →")
                            .tooltip_text("→")
                            .on_click(cx.listener(
                                |this: &mut Self,
                                 _event: &gpui::ClickEvent,
                                 _window: &mut Window,
                                 cx: &mut Context<Self>| {
                                    this.send_review_navigate_next();
                                    cx.notify();
                                },
                            )),
                    )
                    .child(
                        Button::new("delete-button")
                            .variant(ButtonVariant::Danger)
                            .label("Delete")
                            .tooltip_text("⌘D")
                            .on_click(cx.listener(
                                |this: &mut Self,
                                 _event: &gpui::ClickEvent,
                                 _window: &mut Window,
                                 cx: &mut Context<Self>| {
                                    this.send_delete_current_count();
                                    cx.notify();
                                },
                            )),
                    )
                    .child(review_counts.get(current_index).map_or_else(div, |count| {
                        div()
                            .flex_1()
                            .text_right()
                            .text_sm()
                            .child(format!("({}, {})", count.position.0, count.position.1))
                    })),
            )
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

        div()
            .key_context("CounterView")
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
            .child(
                div()
                    .flex()
                    .justify_between()
                    .mt_auto()
                    .child(Self::render_storage_buttons(cx))
                    .when(state.mode == CounterMode::Auto, |this| {
                        this.child(
                            Button::new("copy-to-manual")
                                .variant(ButtonVariant::Primary)
                                .large()
                                .label("Copy to Manual")
                                .on_click(cx.listener(
                                    |this: &mut Self,
                                     _event: &gpui::ClickEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<Self>| {
                                        this.copy_to_manual();
                                    },
                                )),
                        )
                    }),
            )
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
