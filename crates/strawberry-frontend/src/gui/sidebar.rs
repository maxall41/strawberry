use crate::gui::{Button, ButtonVariant};
use std::{
    rc::Rc,
    sync::{Arc, RwLock},
};

use gpui::{
    AnyView, App, Bounds, Context, DragMoveEvent, EventEmitter, FocusHandle, Focusable, FontWeight,
    Pixels, Point, Render, ScrollHandle, Window, div, prelude::*, px,
};
use rfd::FileDialog;
use strawberry_types::{
    BackendToolType, ManagerMessage, ManagerState, PipelineCommand, PipelineObjectMessage,
    ToolProxy, ToolState,
};

use super::{
    busy::BusyOverlay,
    icon::Icon,
    theme,
    tool_picker::{ToolChoice, ToolPicker, ToolPickerEvent},
    tooltip::TooltipView,
};
use crate::tools::{FrontendMessageHandler, ToolBase, spawn_message_processor};

type PanelFactory = Rc<dyn Fn(&mut Window, &mut App) -> ToolPanel>;
const SLOT_HEIGHT: f32 = 8.0;

pub struct ToolPanel {
    pub view: AnyView,
    pub focus: FocusHandle,
}

/// This struct tracks tool registrations for both the command pallete and the sidebar.
pub struct ToolRegistration {
    kind: BackendToolType,
    label: &'static str,
    factory: PanelFactory,
    panel: Option<ToolPanel>,
    expanded: bool,
}

impl ToolRegistration {
    pub fn new(
        kind: BackendToolType,
        label: &'static str,
        factory: impl Fn(&mut Window, &mut App) -> ToolPanel + 'static,
    ) -> Self {
        Self {
            kind,
            label,
            factory: Rc::new(factory),
            panel: None,
            expanded: false,
        }
    }
}

#[derive(Clone)]
struct DraggedTool {
    kind: BackendToolType,
    label: &'static str,
    width: Pixels,
}

struct ToolDragPreview(DraggedTool);

impl Render for ToolDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(self.0.width)
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(theme::ACCENT)
            .bg(theme::CONTROL)
            .text_color(theme::TEXT)
            .text_sm()
            .shadow_lg()
            .child(self.0.label)
    }
}

pub struct ToolSelected;

pub struct ToolSidebar {
    tools: Vec<ToolRegistration>,
    state: Arc<RwLock<ManagerState>>,
    base: ToolBase,
    scroll: ScrollHandle,
    focus_requested: Option<BackendToolType>,
    add_menu: Option<gpui::Entity<ToolPicker>>,
    drag_tool: Option<BackendToolType>,
    drop_slot: Option<usize>,
    queued_commands: Vec<ManagerMessage>,
    busy_overlay: gpui::Entity<BusyOverlay>,
}

impl EventEmitter<ToolSelected> for ToolSidebar {}

impl ToolSidebar {
    pub fn new(
        sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut sidebar = Self {
            tools: Vec::new(),
            state: Arc::new(RwLock::new(ManagerState::default())),
            base: ToolBase::new(sender, BackendToolType::Manager, cx.focus_handle()),
            scroll: ScrollHandle::new(),
            focus_requested: None,
            add_menu: None,
            drag_tool: None,
            drop_slot: None,
            queued_commands: Vec::new(),
            busy_overlay: cx.new(|cx| BusyOverlay::new(true, cx)),
        };
        if sidebar.base.connect() {
            spawn_message_processor(&mut sidebar, window, cx);
        }
        sidebar
    }

    pub fn register(&mut self, tool: ToolRegistration) {
        self.tools.push(tool);
    }

    pub fn reveal(&mut self, cx: &mut Context<Self>) {
        self.add_menu = None;
        cx.emit(ToolSelected);
        cx.notify();
    }

    pub fn add_tool(&mut self, kind: &BackendToolType, cx: &mut Context<Self>) {
        if let Some(tool) = self.tools.iter_mut().find(|tool| tool.kind == *kind) {
            tool.expanded = true;
            self.focus_requested = Some(kind.clone());
            self.add_menu = None;
            self.queued_commands
                .push(ManagerMessage::AddTool(kind.clone()));
            self.reveal(cx);
        }
    }

    fn pipeline_tools(&self) -> Vec<ToolProxy> {
        let mut tools = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .proxy_tools
            .clone();
        tools.sort_by_key(|tool| tool.position);
        tools
    }

    fn flush_commands(&mut self) {
        if self.base.send_channels.is_empty() {
            return;
        }
        for message in std::mem::take(&mut self.queued_commands) {
            if let Err(error) = self.base.send_to_backend(PipelineCommand::PipelineObject(
                PipelineObjectMessage::ManagerMessage(message),
            )) {
                eprintln!("Failed to update pipeline: {error}");
            }
        }
    }

    fn remove_tool(&mut self, kind: &BackendToolType, window: &mut Window, cx: &mut Context<Self>) {
        self.queued_commands
            .push(ManagerMessage::RemoveTool(kind.clone()));
        self.base.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn toggle_expanded(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tool) = self.tools.get_mut(index) {
            tool.expanded = !tool.expanded;
            if tool.expanded {
                self.focus_requested = Some(tool.kind.clone());
            } else {
                self.base.focus_handle.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn pipeline_file(save: bool, window: &Window, cx: &Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let dialog = FileDialog::new().add_filter("Strawberry pipeline", &["sbp"]);
            let file = if save {
                dialog.set_file_name("pipeline.sbp").save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(path) = file {
                let _ = this.update(cx, |this, cx| {
                    this.queued_commands.push(if save {
                        ManagerMessage::SavePipeline(path)
                    } else {
                        ManagerMessage::LoadPipeline(path)
                    });
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn track_drag(
        &mut self,
        row: usize,
        count: usize,
        bounds: Bounds<Pixels>,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let half_slot = SLOT_HEIGHT / 2.0;
        let below_top = row == 0 || f32::from(pointer.y) >= f32::from(bounds.top()) - half_slot;
        let above_bottom = row.saturating_add(1) == count
            || f32::from(pointer.y) < f32::from(bounds.bottom()) + half_slot;
        if below_top && above_bottom {
            let slot = if pointer.y < bounds.center().y {
                row
            } else {
                row.saturating_add(1)
            };
            if self.drop_slot != Some(slot) {
                self.drop_slot = Some(slot);
                cx.notify();
            }
        }
    }

    fn commit_drop(&mut self, cx: &mut Context<Self>) {
        let dragged = self.drag_tool.take();
        let slot = self.drop_slot.take();
        if let (Some(kind), Some(slot)) = (dragged, slot)
            && let Some((from, to)) =
                reorder_indices(&self.pipeline_tools(), &kind.to_string(), slot)
        {
            self.queued_commands
                .push(ManagerMessage::ReorderTool(from, to));
        }
        cx.notify();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep tool card layout and bindings together"
    )]
    fn render_row(
        &self,
        row: usize,
        count: usize,
        index: usize,
        enabled: bool,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let Some(tool) = self.tools.get(index) else {
            return div().into_any_element();
        };
        let kind = tool.kind.clone();
        let remove_kind = kind.clone();
        let dragged = self.drag_tool.as_ref() == Some(&kind);
        let payload = DraggedTool {
            kind,
            label: tool.label,
            width: px((f32::from(self.scroll.bounds().size.width) - 16.0).max(120.0)),
        };
        let entity = cx.entity();
        div()
            .flex()
            .flex_none()
            .flex_col()
            .child(drop_slot(self.drop_slot == Some(row)))
            .child(
                div()
                    .id(("pipeline-tool", index))
                    .flex()
                    .flex_none()
                    .flex_col()
                    .rounded_md()
                    .border_1()
                    .border_color(theme::BORDER_MUTED)
                    .overflow_hidden()
                    .bg(theme::WINDOW_BG)
                    .when(dragged, |el| el.opacity(0.4))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(38.))
                            .px_2()
                            .bg(theme::SURFACE)
                            .child(
                                div()
                                    .id(("tool-drag-handle", index))
                                    .w(px(20.))
                                    .h(px(24.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .cursor_grab()
                                    .hover(|style| style.bg(theme::CONTROL_HOVER))
                                    .text_color(theme::TEXT_MUTED)
                                    .child(drag_grip())
                                    .tooltip(|_, cx| {
                                        cx.new(|_| TooltipView {
                                            text: "Drag to reorder".into(),
                                        })
                                        .into()
                                    })
                                    .on_drag(payload, move |dragged, _, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.drag_tool = Some(dragged.kind.clone());
                                            this.drop_slot = Some(row);
                                            cx.notify();
                                        });
                                        cx.new(|_| ToolDragPreview(dragged.clone()))
                                    }),
                            )
                            .child(
                                Button::new(("tool-expand", index))
                                    .variant(ButtonVariant::Ghost)
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .px_1()
                                    .h_full()
                                    .justify_start()
                                    .aria_expanded(tool.expanded)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(
                                        div()
                                            .w(px(20.))
                                            .h(px(24.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::disclosure(tool.expanded)
                                                    .size(16.)
                                                    .color(theme::TEXT_MUTED),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_color(if enabled {
                                                theme::TEXT
                                            } else {
                                                theme::TEXT_MUTED
                                            })
                                            .child(tool.label),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.toggle_expanded(index, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("tool-remove", index))
                                    .variant(ButtonVariant::Ghost)
                                    .icon(
                                        Icon::x().size(16.).color(theme::TEXT_MUTED),
                                        "Remove tool",
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_tool(&remove_kind, window, cx);
                                    })),
                            ),
                    )
                    .when(tool.expanded, |el| {
                        el.child(
                            div()
                                .border_t_1()
                                .border_color(theme::BORDER_MUTED)
                                .children(tool.panel.as_ref().map(|panel| panel.view.clone())),
                        )
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DraggedTool>, _, cx| {
                            this.track_drag(row, count, event.bounds, event.event.position, cx);
                        },
                    )),
            )
            .into_any_element()
    }

    fn available_tools(&self) -> Vec<ToolChoice> {
        let pipeline = self.pipeline_tools();
        self.tools
            .iter()
            .filter(|tool| {
                !pipeline
                    .iter()
                    .any(|active| active.name == tool.kind.to_string())
            })
            .map(|tool| ToolChoice {
                kind: tool.kind.clone(),
                label: tool.label,
            })
            .collect()
    }

    fn toggle_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_menu.take().is_some() {
            self.base.focus_handle.focus(window, cx);
            cx.notify();
            return;
        }
        let menu = cx.new(|cx| ToolPicker::new(self.available_tools(), cx));
        cx.subscribe_in(
            &menu,
            window,
            |this, _, event: &ToolPickerEvent, window, cx| match event {
                ToolPickerEvent::Selected(kind) => this.add_tool(kind, cx),
                ToolPickerEvent::Dismissed => {
                    this.add_menu = None;
                    this.base.focus_handle.focus(window, cx);
                    cx.notify();
                }
            },
        )
        .detach();
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.add_menu = Some(menu);
        cx.notify();
    }

    fn render_add_menu(&self) -> impl IntoElement + use<> {
        div()
            .id("add-tool-menu")
            .absolute()
            .bottom(px(60.))
            .left(px(8.))
            .right(px(8.))
            .children(self.add_menu.clone())
    }
}

impl FrontendMessageHandler for ToolSidebar {
    type State = ManagerState;

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
        if let ToolState::ManagerState(state) = state {
            Some(state)
        } else {
            None
        }
    }

    fn on_state_changed(&mut self, _new_state: &Self::State, cx: &mut Context<Self>) {
        if let Some(menu) = &self.add_menu {
            let tools = self.available_tools();
            menu.update(cx, |menu, cx| menu.set_tools(tools, cx));
        }
    }
}

impl Render for ToolSidebar {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep sidebar layout and bindings together"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.flush_commands();
        if self.drag_tool.is_some() && !cx.has_active_drag() {
            self.drag_tool = None;
            self.drop_slot = None;
        }
        let pipeline = self.pipeline_tools();
        let mut rows = Vec::new();
        for (row, proxy) in pipeline.iter().enumerate() {
            let Some(index) = self
                .tools
                .iter()
                .position(|tool| tool.kind.to_string() == proxy.name)
            else {
                continue;
            };
            if let Some(tool) = self.tools.get_mut(index)
                && tool.expanded
            {
                if tool.panel.is_none() {
                    tool.panel = Some((tool.factory)(window, cx));
                }
                if self.focus_requested.as_ref() == Some(&tool.kind) {
                    if let Some(panel) = &tool.panel {
                        panel.focus.focus(window, cx);
                    }
                    self.scroll.scroll_to_top_of_item(row);
                    self.focus_requested = None;
                }
            }
            rows.push(self.render_row(row, pipeline.len(), index, proxy.is_enabled, cx));
        }
        div()
            .id("pipeline-sidebar")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .track_focus(&self.base.focus_handle)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.add_menu.take().is_some() {
                    this.base.focus_handle.focus(window, cx);
                    cx.notify();
                }
            }))
            .bg(theme::WINDOW_BG)
            .text_color(theme::TEXT)
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_b_1()
                    .border_color(theme::BORDER_MUTED)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Pipeline"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                Button::new("load-pipeline")
                                    .variant(ButtonVariant::Ghost)
                                    .compact()
                                    .label("Load")
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        Self::pipeline_file(false, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("save-pipeline")
                                    .variant(ButtonVariant::Ghost)
                                    .compact()
                                    .label("Save")
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        Self::pipeline_file(true, window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("pipeline-stack")
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            if this.add_menu.take().is_some() {
                                this.base.focus_handle.focus(window, cx);
                                cx.notify();
                            }
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_2()
                    .pb_2()
                    .when(pipeline.is_empty(), |el| {
                        el.child(
                            div()
                                .px_1()
                                .py_4()
                                .text_sm()
                                .text_color(theme::TEXT_MUTED)
                                .child("No tools added"),
                        )
                    })
                    .children(rows)
                    .child(drop_slot(self.drop_slot == Some(pipeline.len())))
                    .on_drop(cx.listener(|this, _: &DraggedTool, _, cx| this.commit_drop(cx))),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(56.))
                    .flex()
                    .items_center()
                    .p_2()
                    .border_t_1()
                    .border_color(theme::BORDER_MUTED)
                    .child(
                        Button::new("add-tool")
                            .large()
                            .w_full()
                            .child(Icon::plus())
                            .label("Add Tool")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_add_menu(window, cx);
                            })),
                    ),
            )
            .when(self.add_menu.is_some(), |el| {
                el.child(self.render_add_menu())
            })
            .child(self.busy_overlay.clone())
    }
}

fn drag_grip() -> impl IntoElement {
    div().flex().gap(px(2.5)).children((0..2).map(|_| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.5))
            .children((0..3).map(|_| div().size(px(1.5)).rounded_full().bg(theme::TEXT_MUTED)))
    }))
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
                .rounded_full()
                .when(active, |el| el.bg(theme::ACCENT)),
        )
}

/// Translate a visible insertion slot into backend indices. Tools outside the
/// stack can leave gaps, so visible row numbers cannot be sent directly.
fn reorder_indices(tools: &[ToolProxy], name: &str, slot: usize) -> Option<(u16, u16)> {
    let from = tools.iter().position(|tool| tool.name == name)?;
    if slot > tools.len() || slot == from || slot == from.saturating_add(1) {
        return None;
    }
    let target = if slot < from {
        slot
    } else {
        slot.saturating_sub(1)
    };
    Some((tools.get(from)?.position, tools.get(target)?.position))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline() -> Vec<ToolProxy> {
        [("Contrast", 2), ("Sobel", 5), ("Counter", 9)]
            .into_iter()
            .map(|(name, position)| ToolProxy {
                name: name.into(),
                position,
                is_enabled: name != "Sobel",
            })
            .collect()
    }

    #[test]
    fn reorder_uses_backend_positions_and_includes_disabled_stack_items() {
        assert_eq!(reorder_indices(&pipeline(), "Counter", 0), Some((9, 2)));
        assert_eq!(reorder_indices(&pipeline(), "Contrast", 3), Some((2, 9)));
        assert_eq!(reorder_indices(&pipeline(), "Counter", 1), Some((9, 5)));
    }

    #[test]
    fn adjacent_and_invalid_drops_do_not_reorder() {
        assert_eq!(reorder_indices(&pipeline(), "Sobel", 1), None);
        assert_eq!(reorder_indices(&pipeline(), "Sobel", 2), None);
        assert_eq!(reorder_indices(&pipeline(), "Sobel", 8), None);
        assert_eq!(reorder_indices(&pipeline(), "Missing", 0), None);
        assert_eq!(reorder_indices(&[], "Sobel", 0), None);
    }
}
