use crate::gui::theme;
// Implements UI for counter tool

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, ElementId, FocusHandle, Focusable,
    KeyBinding, MouseDownEvent, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};
use rfd::FileDialog;

use crate::gui::{icon::Icon, value_control::ValueControl, ToggleButton};
use crate::tools::{spawn_message_processor, FrontendMessageHandler};
use strawberry_types::{
    consts::{image_rgba_to_gpui_rgba, CIRCLE_COLORS},
    CountData, CountMessage, CounterMode, CounterState, CounterTool, FilterMessage, FilterProxy,
    PipelineCommand, PipelineObjectMessage, ToolState,
};

use strawberry_types::ToolFrontend;

pub struct CounterView {
    pub state: Arc<RwLock<CounterState>>,
    pub next_count_type_id: Arc<RwLock<u16>>,
    pub last_directory: Arc<RwLock<Option<PathBuf>>>,
    pub image_directory: Arc<RwLock<Option<PathBuf>>>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for CounterView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl CounterView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        image_directory: Arc<RwLock<Option<PathBuf>>>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(CounterState::default())),
            next_count_type_id: Arc::new(RwLock::new(0)),
            last_directory: Arc::new(RwLock::new(None)),
            image_directory,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Counter,
            ),
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn save_counts(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            return;
        };

        let last_directory = self.last_directory.clone();
        let image_directory = self.image_directory.clone();

        cx.spawn_in(_window, async move |this, cx| {
            // let default_dir = last_directory
            //     .read()
            //     .ok()
            //     .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
            //     .or_else(|| {
            //         image_directory
            //             .read()
            //             .ok()
            //             .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
            //     })
            //     .unwrap_or_else(|| "~/".to_string());
            let default_dir = image_directory
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(|p| p.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~/".to_string());

            println!("default_dir {:?}", default_dir);

            let file = FileDialog::new()
                .add_filter("xml", &["xml"])
                .set_directory(&default_dir)
                .save_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.base.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::SaveCounts(file)),
                        )) {
                            eprintln!("Failed to send save counts command: {e}");
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

    fn load_counts(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            return;
        };

        let last_directory = self.last_directory.clone();
        let image_directory = self.image_directory.clone();

        cx.spawn_in(_window, async move |this, cx| {
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
                if let Some(parent) = file.parent() {
                    if let Ok(mut dir) = last_directory.write() {
                        *dir = Some(parent.to_path_buf());
                    }
                }

                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.base.send_channels.get(current_tool_index as usize) {
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

    fn add_count_type(&mut self) {
        let new_id = match self.next_count_type_id.read() {
            Ok(guard) => *guard,
            Err(e) => {
                eprintln!("Failed to read next count type ID: {e}");
                return;
            }
        };

        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::NewCountType(new_id)),
            )) {
                eprintln!("Failed to send new count type command: {e}");
                return;
            }
            println!("Sent new count type message!")
        } else {
            eprintln!(
                "Failed to send new count type message. Invalid tool index: {current_tool_index}"
            );
            return;
        }

        // Increment the next ID
        if let Ok(mut next_id) = self.next_count_type_id.write() {
            *next_id += 1;
        }

        println!("Added count type {new_id}");
    }

    fn delete_count_type(&mut self, count_type_id: u16) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::DeleteCountType(count_type_id)),
            )) {
                eprintln!("Failed to send delete count type command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }

        // Clear selection if this was the selected type
        if let Ok(state) = self.state.read() {
            if state.active_count_type == Some(count_type_id) {
                if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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

                    if let Some(new_active_id) = new_active {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::CountMessage(CountMessage::SetActiveCountType(
                                new_active_id,
                            )),
                        )) {
                            eprintln!("Failed to send set active count type command: {e}");
                        }
                    }
                }
            }
        }
    }

    fn select_count_type(
        &mut self,
        count_type_id: u16,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
        let current_tool_index = match self.base.get_current_tool_index() {
            Some(index) => index,
            None => {
                eprintln!("No current tool index available for copy to manual");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
        let current_tool_index = match self.base.get_current_tool_index() {
            Some(index) => index,
            None => {
                eprintln!("No current tool index available for toggle count type visibility");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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

    fn set_tool(
        &mut self,
        tool: Option<CounterTool>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::SetActiveTool(tool)),
            )) {
                eprintln!("Failed to send set active tool command: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn toggle_enabled(
        &mut self,
        current_value: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
            .expect("State lock should not be poisoned")
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
            Ok(state) => state.active_tool.clone(),
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
            Ok(state) => state.active_tool.clone(),
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
            Ok(state) => state.active_tool.clone(),
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
            Ok(state) => state.active_tool.clone(),
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

    fn select_count_type_by_position(
        &mut self,
        position: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(_) => return,
        };

        let mut sorted_counts: Vec<_> = state.manual_counts.iter().collect();
        sorted_counts.sort_by_key(|(id, _)| **id);

        if let Some((&count_type_id, _)) = sorted_counts.get(position) {
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

    fn toggle_mode(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
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

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
    fn render_subheader(&self) -> impl IntoElement {
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_2()
            .child("Counts:")
    }

    // Status component showing enabled/disabled state
    fn render_status(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        let is_enabled = state.is_enabled;
        ToggleButton::new(
            "Enabled".to_string(),
            is_enabled,
            move |this: &mut CounterView,
                  _event: &MouseDownEvent,
                  window: &mut Window,
                  cx: &mut Context<CounterView>| {
                this.toggle_enabled(is_enabled, window, cx);
            },
        )
        .with_keyboard_shortcut("⌘T")
        .render(cx)
    }

    // Mode toggle component
    fn render_mode_toggle(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .mb_3()
            .child("Mode:")
            .child(
                // Padding + negative margin extend the click region past the
                // visible button without shifting layout.
                div()
                    .p_1()
                    .m_neg_1()
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, {
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<CounterView>| {
                                this.toggle_mode(window, cx);
                            },
                        )
                    })
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme::CONTROL)
                            .hover(|style| style.bg(theme::CONTROL_HOVER))
                            .child(match state.mode {
                                CounterMode::Manual => "Manual",
                                CounterMode::Auto => "Auto",
                            }),
                    ),
            )
    }

    // Individual count type item component
    fn render_count_type_item(
        &self,
        count_type_id: u16,
        name: String,
        count_data: &Vec<CountData>,
        is_selected: bool,
        is_visible: bool,
        position: Option<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ii = count_type_id as usize % CIRCLE_COLORS.len();
        let color = image_rgba_to_gpui_rgba(CIRCLE_COLORS[ii]);
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
                div()
                    .m_2()
                    .w_5()
                    .h_5()
                    .bg(theme::DANGER)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::DANGER_HOVER))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::x().size(12.0))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<CounterView>| {
                                this.delete_count_type(count_type_id);
                            },
                        ),
                    ),
            )
            .child(
                // Eye toggle button. Padding traded against the margin keeps the
                // icon in place while the click region extends past the visual.
                div()
                    .m_1()
                    .p_1()
                    .w_7()
                    .h_7()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::CONTROL_HOVER))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if is_visible {
                        Icon::eye_open().size(21.0)
                    } else {
                        Icon::eye_closed().size(21.0)
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<CounterView>| {
                                this.toggle_count_type_visibility(count_type_id);
                            },
                        ),
                    ),
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
                        "count-type-{}-selection-circle",
                        count_type_id
                    ))))
                    .when(position.is_some() && position.unwrap() <= 8, |this| {
                        let pos = position.unwrap() + 1;
                        this.tooltip(move |_window, cx| {
                            let tooltip_view = crate::gui::tooltip::TooltipView {
                                text: format!("⌘{}", pos),
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
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  window: &mut Window,
                                  cx: &mut Context<CounterView>| {
                                this.select_count_type(count_type_id, window, cx);
                            },
                        ),
                    ),
            )
    }

    // Count types list component
    fn render_count_types_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(_) => {
                return div().child("Error reading state");
            }
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
                        .unwrap_or_else(|| format!("Type {}", position + 1));
                    self.render_count_type_item(
                        count_type_id,
                        name,
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
    fn render_add_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .p_2()
            .mt_2()
            .bg(theme::CONTROL)
            .cursor_pointer()
            .hover(|style| style.bg(theme::CONTROL_HOVER))
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .text_lg()
            .child("+")
            .id("add-count-type-button")
            .tooltip(move |_window, cx| {
                let tooltip_view = crate::gui::tooltip::TooltipView {
                    text: "⌘N".to_string(),
                };
                cx.new(|_cx| tooltip_view).into()
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    |this: &mut CounterView,
                     _event: &MouseDownEvent,
                     _window: &mut Window,
                     _cx: &mut Context<CounterView>| {
                        this.add_count_type();
                    },
                ),
            )
    }

    // Tool button component
    fn render_tool_button(
        &self,
        tool: CounterTool,
        label: String,
        is_active: bool,
        keyboard_shortcut: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(ElementId::Name(SharedString::from(format!(
                "tool-button-{:?}",
                tool
            ))))
            .flex_1()
            .p_2()
            .cursor_pointer()
            .bg(if is_active {
                theme::ACCENT
            } else {
                theme::CONTROL
            })
            .hover(|style| {
                style.bg(if is_active {
                    theme::ACCENT_HOVER
                } else {
                    theme::CONTROL_HOVER
                })
            })
            .text_center()
            .text_sm()
            .child(label)
            .tooltip({
                let shortcut = keyboard_shortcut.to_string();
                move |_window, cx| {
                    let tooltip_view = crate::gui::tooltip::TooltipView {
                        text: shortcut.clone(),
                    };
                    cx.new(|_cx| tooltip_view).into()
                }
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(
                    move |this: &mut CounterView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<CounterView>| {
                        if is_active {
                            this.set_tool(None, window, cx);
                        } else {
                            this.set_tool(Some(tool.clone()), window, cx);
                        }
                    },
                ),
            )
    }

    // Tools subheader component
    fn render_tools_subheader(&self) -> impl IntoElement {
        div()
            .text_lg()
            .font_weight(gpui::FontWeight::BOLD)
            .mb_2()
            .child("Tools:")
    }

    // Tools bar component
    fn render_tools_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(_) => {
                return div().child("Error reading state");
            }
        };
        let current_tool = &state.active_tool;

        div()
            .flex()
            .bg(theme::SURFACE)
            .rounded_md()
            .overflow_hidden()
            .mb_4()
            .child(self.render_tool_button(
                CounterTool::Place,
                "Place".to_string(),
                *current_tool == Some(CounterTool::Place),
                "⌘W",
                cx,
            ))
            .child(self.render_tool_button(
                CounterTool::Delete,
                "Delete".to_string(),
                *current_tool == Some(CounterTool::Delete),
                "⌘E",
                cx,
            ))
            .child(self.render_tool_button(
                CounterTool::LassoDelete,
                "Lasso | Delete".to_string(),
                *current_tool == Some(CounterTool::LassoDelete),
                "⌘A",
                cx,
            ))
            .child(self.render_tool_button(
                CounterTool::Review,
                "Review".to_string(),
                *current_tool == Some(CounterTool::Review),
                "⌘R",
                cx,
            ))
    }

    // Save button component
    fn render_storage_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(theme::SUCCESS)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::SUCCESS_HOVER))
                    .rounded_md()
                    .text_sm()
                    .child("Save")
                    .id("save-button")
                    .tooltip(move |_window, cx| {
                        let tooltip_view = crate::gui::tooltip::TooltipView {
                            text: "⌘S".to_string(),
                        };
                        cx.new(|_cx| tooltip_view).into()
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut CounterView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<CounterView>| {
                                _this.save_counts(_window, _cx);
                            },
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(theme::ACCENT)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::ACCENT_HOVER))
                    .rounded_md()
                    .text_sm()
                    .child("Load")
                    .id("load-button")
                    .tooltip(move |_window, cx| {
                        let tooltip_view = crate::gui::tooltip::TooltipView {
                            text: "⌘L".to_string(),
                        };
                        cx.new(|_cx| tooltip_view).into()
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut CounterView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<CounterView>| {
                                _this.load_counts(_window, _cx);
                            },
                        ),
                    ),
            )
    }

    // Manual UI rendering
    fn render_manual_ui(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        let mut ui = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.render_subheader())
            .child(self.render_count_types_list(cx))
            .child(self.render_add_button(cx))
            .child(self.render_tools_subheader())
            .child(self.render_tools_bar(cx));

        // Add review UI when Review tool is active
        if state.active_tool == Some(CounterTool::Review) {
            ui = ui.child(self.render_review_ui(state, cx));
        }

        ui
    }

    // Auto UI rendering (placeholder)
    fn render_auto_ui(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.render_filters_section(state, cx))
    }

    fn render_filters_section(
        &self,
        state: &CounterState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                                self.render_filter_item(filter_proxy, index, cx)
                            }),
                    ),
                )
            })
    }

    fn render_filter_item(
        &self,
        filter_proxy: &FilterProxy,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_enabled = filter_proxy.is_enabled;
        let filter_index = index as u16;

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
            .hover(|style| {
                style.bg(if is_enabled {
                    theme::WINDOW_BG
                } else {
                    theme::SURFACE_ALT
                })
            })
            .border_1()
            .border_color(if is_enabled {
                theme::SUCCESS
            } else {
                theme::CONTROL
            })
            .child(
                div()
                    .flex()
                    .items_center()
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::TEXT_MUTED)
                            .child("Click to toggle"),
                    )
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            move |this: &mut CounterView,
                                  _event: &MouseDownEvent,
                                  _window: &mut Window,
                                  _cx: &mut Context<CounterView>| {
                                this.toggle_filter(filter_index, !is_enabled);
                            },
                        ),
                    ),
            )
            .when(is_size_filter, |el| {
                let current_value = filter_proxy
                    .values
                    .first()
                    .map(|v| *v as f32)
                    .unwrap_or(10.0);

                // Create a ValueControl for this filter
                let control = cx.new(|cx| {
                    ValueControl::new(
                        format!("{current_value:.0}"),
                        current_value,
                        "Minimum Object Size".to_string(),
                        1.0,
                        format!("size_filter_{filter_index}"),
                        1.0,
                        10000.0,
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
    }

    fn toggle_filter(&mut self, filter_index: u16, enabled: bool) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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

    fn adjust_size_filter(&mut self, delta: i16) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
            let current_value = {
                let state = self
                    .state
                    .read()
                    .expect("State lock should not be poisoned");
                state
                    .filter_proxys
                    .iter()
                    .find(|f| f.name == "Filter by Size")
                    .and_then(|f| f.values.first())
                    .copied()
                    .unwrap_or(10.0)
            };

            let new_min_size = ((current_value as i16) + delta).max(1) as u16;

            if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                PipelineObjectMessage::CountMessage(CountMessage::FilterMessage(
                    FilterMessage::ConfigureSizeFilter(new_min_size),
                )),
            )) {
                eprintln!("Failed to send size filter configuration: {e}");
            }
        } else {
            eprintln!("Invalid tool index: {current_tool_index}");
        }
    }

    fn render_review_ui(&self, state: &CounterState, cx: &mut Context<Self>) -> impl IntoElement {
        let current_index = state.review_current_index;

        static EMPTY: Vec<CountData> = Vec::new();
        let review_counts = state
            .active_count_type
            .and_then(|id| state.manual_counts.get(&id))
            .unwrap_or(&EMPTY);

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
                    .child(
                        div()
                            .text_sm()
                            .child(format!("{} / {}", current_index + 1, review_counts.len())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("prev-count")
                            .px_3()
                            .py_1()
                            .bg(theme::CONTROL)
                            .cursor_pointer().hover(|style| style.bg(theme::CONTROL_HOVER))
                            .rounded_md()
                            .text_sm()
                            .child("← Prev")
                            .tooltip(move |_window, cx| {
                                let tooltip_view = crate::gui::tooltip::TooltipView {
                                    text: "←".to_string(),
                                };
                                cx.new(|_cx| tooltip_view).into()
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    |this: &mut CounterView,
                                     _event: &MouseDownEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<CounterView>| {
                                        this.send_review_navigate_previous();
                                        _cx.notify();
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("next-count")
                            .px_3()
                            .py_1()
                            .bg(theme::CONTROL)
                            .cursor_pointer().hover(|style| style.bg(theme::CONTROL_HOVER))
                            .rounded_md()
                            .text_sm()
                            .child("Next →")
                            .tooltip(move |_window, cx| {
                                let tooltip_view = crate::gui::tooltip::TooltipView {
                                    text: "→".to_string(),
                                };
                                cx.new(|_cx| tooltip_view).into()
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    |this: &mut CounterView,
                                     _event: &MouseDownEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<CounterView>| {
                                        this.send_review_navigate_next();
                                        _cx.notify();
                                    },
                                ),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(theme::DANGER)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme::DANGER_HOVER))
                            .rounded_md()
                            .text_sm()
                            .child("Delete")
                            .id("delete-button")
                            .tooltip(move |_window, cx| {
                                let tooltip_view = crate::gui::tooltip::TooltipView {
                                    text: "⌘D".to_string(),
                                };
                                cx.new(|_cx| tooltip_view).into()
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(
                                    |this: &mut CounterView,
                                     _event: &MouseDownEvent,
                                     _window: &mut Window,
                                     _cx: &mut Context<CounterView>| {
                                        this.send_delete_current_count();
                                        _cx.notify();
                                    },
                                ),
                            ),
                    )
                    .child(if current_index < review_counts.len() {
                        let count = &review_counts[current_index];
                        div()
                            .flex_1()
                            .text_right()
                            .text_sm()
                            .child(format!("({}, {})", count.position.0, count.position.1))
                    } else {
                        div()
                    }),
            )
    }

    // Review functionality methods - now sends messages to backend
    fn send_review_navigate_previous(&self) {
        let current_tool_index = match self.base.current_tool_index.read() {
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
            Ok(guard) => match *guard {
                Some(index) => index,
                None => {
                    eprintln!("No current tool index available");
                    return;
                }
            },
            Err(e) => {
                eprintln!("Failed to read current tool index: {e}");
                return;
            }
        };

        if let Some(sender) = self.base.send_channels.get(current_tool_index as usize) {
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
                .map(|&k| k + 1)
                .unwrap_or(0);
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
                    .size_full()
                    .text_sm()
                    .p_4()
                    .shadow_lg()
                    .text_color(theme::TEXT)
                    .child("Error: Unable to read counter state");
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
                    move |this: &mut CounterView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<CounterView>| {
                        this.focus_handle(cx).focus(window);
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
            .child(self.render_header())
            .child(self.render_status(&state, cx))
            .child(self.render_mode_toggle(&state, cx))
            .child(match state.mode {
                CounterMode::Manual => div().child(self.render_manual_ui(&state, cx)),
                CounterMode::Auto => div().child(self.render_auto_ui(&state, cx)),
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .mt_auto()
                    .child(self.render_storage_buttons(cx))
                    .when(state.mode == CounterMode::Auto, |this| {
                        this.child(
                            div()
                                .px_4()
                                .py_2()
                                .bg(theme::ACCENT)
                                .cursor_pointer().hover(|style| style.bg(theme::ACCENT_HOVER))
                                .rounded_md()
                                .text_sm()
                                .child("Copy to Manual")
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(
                                        |this: &mut CounterView,
                                         _event: &MouseDownEvent,
                                         _window: &mut Window,
                                         _cx: &mut Context<CounterView>| {
                                            this.copy_to_manual();
                                        },
                                    ),
                                )
                        )
                    }),
            )
    }
}

impl_tool_view_basics!(CounterView, "Counter");

impl ToolFrontend for CounterView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Counter")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(400.), px(700.)),
                origin: Point::new(px(720.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect(cx.focus_handle()) {
            return;
        }

        cx.bind_keys([
            KeyBinding::new("cmd-t", CounterToggleActivity, None),
            KeyBinding::new("cmd-w", CounterPlaceTool, None),
            KeyBinding::new("cmd-e", CounterDeleteTool, None),
            KeyBinding::new("cmd-a", CounterLassoTool, None),
            KeyBinding::new("cmd-r", CounterReviewTool, None),
            KeyBinding::new("left", CounterReviewPrevious, None),
            KeyBinding::new("right", CounterReviewNext, None),
            KeyBinding::new("cmd-d", CounterDeleteCount, None),
            KeyBinding::new("cmd-s", CounterSaveCounts, None),
            KeyBinding::new("cmd-l", CounterLoadCounts, None),
            KeyBinding::new("cmd-n", CounterNewCountType, None),
            KeyBinding::new("cmd-1", CounterSelectType1, None),
            KeyBinding::new("cmd-2", CounterSelectType2, None),
            KeyBinding::new("cmd-3", CounterSelectType3, None),
            KeyBinding::new("cmd-4", CounterSelectType4, None),
            KeyBinding::new("cmd-5", CounterSelectType5, None),
            KeyBinding::new("cmd-6", CounterSelectType6, None),
            KeyBinding::new("cmd-7", CounterSelectType7, None),
            KeyBinding::new("cmd-8", CounterSelectType8, None),
            KeyBinding::new("cmd-9", CounterSelectType9, None),
        ]);

        self.process_messages(window, cx);
    }
}
