use crate::gui::theme;
// Implements UI for pipeline manager tool

use std::sync::{Arc, RwLock};

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Div, DragMoveEvent, FocusHandle, Focusable,
    MouseDownEvent, Pixels, Point, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};
use rfd::FileDialog;

use crate::tools::{spawn_message_processor, FrontendMessageHandler};
use strawberry_types::{
    ManagerMessage, ManagerState, PipelineCommand, PipelineObjectMessage, ToolProxy, ToolState,
};

use strawberry_types::ToolFrontend;

/// Vertical space reserved between rows for the insertion marker.
const SLOT_HEIGHT: f32 = 8.0;

#[derive(Clone)]
pub struct DraggedTool {
    row: usize,
    name: SharedString,
    order_label: SharedString,
}

/// Element that follows the cursor while dragging.
pub struct ToolDragPreview {
    tool: DraggedTool,
    width: Pixels,
}

impl Render for ToolDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        row_frame(self.tool.name.clone(), self.tool.order_label.clone())
            .w(self.width)
            .bg(theme::CONTROL_HOVER)
            .border_color(theme::ACCENT)
            .text_color(theme::TEXT)
            .shadow_lg()
    }
}

fn drag_grip() -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .gap(px(2.))
        .children((0..2).map(|_| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .children((0..3).map(|_| div().size(px(2.)).rounded_full().bg(theme::TEXT_MUTED)))
        }))
}

fn row_frame(name: SharedString, order_label: SharedString) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .text_sm()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(drag_grip())
                .child(div().font_weight(gpui::FontWeight::MEDIUM).child(name)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::TEXT_MUTED)
                .child(order_label),
        )
}

fn drop_slot(active: bool) -> impl IntoElement {
    div()
        .h(px(SLOT_HEIGHT))
        .flex()
        .flex_none()
        .items_center()
        .child(
            div()
                .h(px(2.))
                .w_full()
                .mx_1()
                .rounded_full()
                .when(active, |this| this.bg(theme::ACCENT)),
        )
}

pub struct ManagerView {
    pub state: Arc<RwLock<ManagerState>>,
    /// Row being dragged, rendered ghosted in place.
    drag_row: Option<usize>,
    /// Slot a drop would insert into, in `0..=row_count` of the pre-drop order.
    drop_slot: Option<usize>,
    pub base: crate::tools::base::ToolBase,
}

impl Focusable for ManagerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base
            .focus_handle
            .clone()
            .expect("focus_handle should be initialized before use")
    }
}

impl ManagerView {
    pub fn new(
        _command_receiver: tokio::sync::broadcast::Receiver<PipelineCommand>,
        command_sender: tokio::sync::broadcast::Sender<PipelineCommand>,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(ManagerState::default())),
            drag_row: None,
            drop_slot: None,
            base: crate::tools::base::ToolBase::new(
                command_sender.clone(),
                strawberry_types::BackendToolType::Manager,
            ),
        }
    }

    fn process_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        spawn_message_processor(self, window, cx);
    }

    fn render_row(
        &self,
        row: usize,
        row_count: usize,
        tool: &ToolProxy,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = SharedString::from(tool.name.clone());
        let order_label = SharedString::from(format!("#{row}"));
        let is_source = self.drag_row == Some(row);
        let payload = DraggedTool {
            row,
            name: name.clone(),
            order_label: order_label.clone(),
        };
        let entity = cx.entity();

        div()
            .flex()
            .flex_col()
            .child(drop_slot(self.drop_slot == Some(row)))
            .child(
                row_frame(name, order_label)
                    .id(("tool-row", row))
                    .cursor_grab()
                    .bg(if is_source {
                        theme::SURFACE_DEEP
                    } else {
                        theme::CONTROL
                    })
                    .border_color(if is_source {
                        theme::BORDER_MUTED
                    } else {
                        theme::BORDER
                    })
                    .when(is_source, |this| this.opacity(0.4))
                    .when(!is_source, |this| {
                        this.hover(|style| style.bg(theme::CONTROL_HOVER))
                    })
                    .on_drag(
                        payload,
                        move |dragged: &DraggedTool, _offset, window, cx| {
                            entity.update(cx, |this, cx| {
                                this.drag_row = Some(dragged.row);
                                this.drop_slot = Some(dragged.row);
                                cx.notify();
                            });
                            let available = window.viewport_size().width - px(32.);
                            let tool = dragged.clone();
                            cx.new(move |_| ToolDragPreview {
                                tool,
                                width: if available < px(120.) {
                                    px(120.)
                                } else {
                                    available
                                },
                            })
                        },
                    )
                    .on_drag_move(cx.listener(
                        move |this: &mut ManagerView,
                              event: &DragMoveEvent<DraggedTool>,
                              _window: &mut Window,
                              cx: &mut Context<ManagerView>| {
                            this.track_drag(row, row_count, event.bounds, event.event.position, cx);
                        },
                    )),
            )
    }

    fn track_drag(
        &mut self,
        row: usize,
        row_count: usize,
        bounds: Bounds<Pixels>,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let half_slot = px(SLOT_HEIGHT / 2.0);
        let below_top = row == 0 || pointer.y >= bounds.top() - half_slot;
        let above_bottom = row + 1 == row_count || pointer.y < bounds.bottom() + half_slot;
        if !(below_top && above_bottom) {
            return;
        }

        let slot = if pointer.y < bounds.center().y {
            row
        } else {
            row + 1
        };
        if self.drop_slot != Some(slot) {
            self.drop_slot = Some(slot);
            cx.notify();
        }
    }

    fn commit_drop(&mut self, cx: &mut Context<Self>) {
        let from = self.drag_row.take();
        let slot = self.drop_slot.take();
        cx.notify();

        let (Some(from), Some(slot)) = (from, slot) else {
            return;
        };
        // Both slots bordering the dragged row leave the order unchanged.
        if slot == from || slot == from + 1 {
            return;
        }

        let sorted = match self.state.read() {
            Ok(state) => {
                let mut sorted = state.proxy_tools.clone();
                sorted.sort_by_key(|tool| tool.position);
                sorted
            }
            Err(e) => {
                eprintln!("Failed to read manager state while reordering: {e}");
                return;
            }
        };
        if from >= sorted.len() || slot > sorted.len() {
            return;
        }

        let start_index = sorted[from].position;
        let new_index = if slot < from {
            sorted[slot].position
        } else {
            sorted[slot - 1].position
        };

        if let Err(e) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::ManagerMessage(ManagerMessage::ReorderTool(
                start_index,
                new_index,
            )),
        )) {
            eprintln!("Failed to send reorder command: {e}");
        }
    }

    // Tools list component
    fn render_tools(&self, tools: &[ToolProxy], cx: &mut Context<Self>) -> impl IntoElement {
        let row_count = tools.len();
        let rows: Vec<_> = tools
            .iter()
            .enumerate()
            .map(|(row, tool)| self.render_row(row, row_count, tool, cx))
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Tools"),
                    )
                    .when(row_count > 1, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child("drag to reorder"),
                        )
                    }),
            )
            .child(
                div()
                    .id("tool-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(row_count == 0, |this| {
                        this.child(
                            div()
                                .pt_2()
                                .text_xs()
                                .text_color(theme::TEXT_MUTED)
                                .child("No tools enabled."),
                        )
                    })
                    .children(rows)
                    .child(drop_slot(self.drop_slot == Some(row_count))),
            )
    }

    fn save_pipeline(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("toml", &["toml"])
                .set_directory("~/")
                .save_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.base.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::ManagerMessage(ManagerMessage::SavePipeline(
                                file,
                            )),
                        )) {
                            eprintln!("Failed to send save pipeline command: {e}");
                        }
                    } else {
                        eprintln!("Invalid tool index: {current_tool_index}");
                    }
                }) {
                    eprintln!("Failed to save pipeline: {e}");
                }
            }
        })
        .detach();
    }

    fn load_pipeline(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(current_tool_index) = self.base.get_current_tool_index() else {
            return;
        };

        cx.spawn_in(_window, async move |this, cx| {
            let file = FileDialog::new()
                .add_filter("toml", &["toml"])
                .set_directory("~/")
                .pick_file();

            if let Some(file) = file {
                if let Err(e) = this.update(cx, |this, cx| {
                    if let Some(sender) = this.base.send_channels.get(current_tool_index as usize) {
                        if let Err(e) = sender.send(PipelineCommand::PipelineObject(
                            PipelineObjectMessage::ManagerMessage(ManagerMessage::LoadPipeline(
                                file,
                            )),
                        )) {
                            eprintln!("Failed to send load pipeline command: {e}");
                        }
                    } else {
                        eprintln!("Invalid tool index: {current_tool_index}");
                    }
                }) {
                    eprintln!("Failed to load pipeline: {e}");
                }
            }
        })
        .detach();
    }

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
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut ManagerView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<ManagerView>| {
                                _this.save_pipeline(_window, _cx);
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
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(
                            |_this: &mut ManagerView,
                             _event: &MouseDownEvent,
                             _window: &mut Window,
                             _cx: &mut Context<ManagerView>| {
                                _this.load_pipeline(_window, _cx);
                            },
                        ),
                    ),
            )
    }
}

impl FrontendMessageHandler for ManagerView {
    type State = ManagerState;

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
            ToolState::ManagerState(s) => Some(s),
            _ => None,
        }
    }
}

impl Render for ManagerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.drag_row.is_some() && !cx.has_active_drag() {
            self.drag_row = None;
            self.drop_slot = None;
        }

        let tools = match self.state.read() {
            Ok(state) => {
                let mut tools = state.proxy_tools.clone();
                tools.sort_by_key(|tool| tool.position);
                tools
            }
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
                    .child("Error: Unable to read manager state");
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
                    move |this: &mut ManagerView,
                          _event: &MouseDownEvent,
                          window: &mut Window,
                          cx: &mut Context<ManagerView>| {
                        this.focus_handle(cx).focus(window, cx);
                    },
                )
            })
            .on_drop(cx.listener(
                |this: &mut ManagerView,
                 _dragged: &DraggedTool,
                 _window: &mut Window,
                 cx: &mut Context<ManagerView>| {
                    this.commit_drop(cx);
                },
            ))
            .child(self.render_header())
            .child(self.render_tools(&tools, cx))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(self.render_storage_buttons(cx)),
            )
    }
}

impl_tool_view_basics!(ManagerView, "Pipeline Manager");

impl ToolFrontend for ManagerView {
    fn get_window_options(&self) -> WindowOptions {
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("Pipeline Manager")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(250.), px(350.)),
                origin: Point::new(px(200.), px(200.)),
            })),

            ..Default::default()
        }
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.base.connect(cx.focus_handle()) {
            return;
        }
        self.process_messages(window, cx);
    }
}
