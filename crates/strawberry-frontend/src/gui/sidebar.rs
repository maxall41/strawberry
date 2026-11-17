use crate::gui::{Button, ButtonVariant};
use std::{collections::HashMap, rc::Rc};

use gpui::{
    AnyView, App, Bounds, Context, DragMoveEvent, EventEmitter, FocusHandle, Focusable, FontWeight,
    Pixels, Point, Render, ScrollHandle, Window, div, prelude::*, px,
};
use rfd::FileDialog;
use strawberry_types::{
    BackendToolType, CommandSender, PipelineCommand, PipelineState, ToolId, ToolSummary,
};

use super::{
    busy::BusyOverlay,
    icon::Icon,
    theme,
    tool_picker::{ToolChoice, ToolPicker, ToolPickerEvent},
    tooltip::TooltipView,
};

type PanelFactory = Rc<dyn Fn(ToolId, &mut Window, &mut App) -> ToolPanel>;
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
}

impl ToolRegistration {
    /// Create `ToolRegistration` with its initial state and configuration.
    pub fn new(
        kind: BackendToolType,
        label: &'static str,
        factory: impl Fn(ToolId, &mut Window, &mut App) -> ToolPanel + 'static,
    ) -> Self {
        Self {
            kind,
            label,
            factory: Rc::new(factory),
        }
    }
}

#[derive(Default)]
struct InstancePanel {
    panel: Option<ToolPanel>,
    expanded: bool,
}

#[derive(Clone)]
struct DraggedTool {
    id: ToolId,
    label: String,
    width: Pixels,
}

struct ToolDragPreview(DraggedTool);

impl Render for ToolDragPreview {
    /// Build the sidebar view and its interaction handlers.
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
            .child(self.0.label.clone())
    }
}

pub struct ToolSelected;

pub struct ToolSidebar {
    tools: Vec<ToolRegistration>,
    instances: HashMap<ToolId, InstancePanel>,
    state: PipelineState,
    sender: CommandSender,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    focus_requested: Option<ToolId>,
    add_menu: Option<gpui::Entity<ToolPicker>>,
    drag_tool: Option<ToolId>,
    drop_slot: Option<usize>,
    busy_overlay: gpui::Entity<BusyOverlay>,
}

impl EventEmitter<ToolSelected> for ToolSidebar {}

impl ToolSidebar {
    /// Create `ToolSidebar` with its initial state and configuration.
    pub fn new(sender: CommandSender, window: &Window, cx: &mut Context<Self>) -> Self {
        let (reply, subscription) = tokio::sync::oneshot::channel();
        let _ = sender.send(PipelineCommand::SubscribePipeline(reply));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(mut receiver) = subscription.await else {
                return;
            };
            loop {
                let state = receiver.borrow_and_update().clone();
                if this
                    .update(cx, |this, cx| {
                        this.instances
                            .retain(|id, _| state.tools.iter().any(|tool| tool.id == *id));
                        if let Some(id) = state.selected_tool
                            && !this.instances.contains_key(&id)
                        {
                            this.instances.insert(
                                id,
                                InstancePanel {
                                    expanded: true,
                                    panel: None,
                                },
                            );
                            this.focus_requested = Some(id);
                        }
                        this.state = state;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if receiver.changed().await.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            tools: Vec::new(),
            instances: HashMap::new(),
            state: PipelineState::default(),
            sender,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            focus_requested: None,
            add_menu: None,
            drag_tool: None,
            drop_slot: None,
            busy_overlay: cx.new(|cx| BusyOverlay::new(true, cx)),
        }
    }

    /// Register a tool kind and its panel factory with the sidebar.
    pub fn register(&mut self, tool: ToolRegistration) {
        self.tools.push(tool);
    }

    /// Reveal the sidebar and dismiss its add-tool menu.
    pub fn reveal(&mut self, cx: &mut Context<Self>) {
        self.add_menu = None;
        cx.emit(ToolSelected);
        cx.notify();
    }

    /// Request a new tool instance and reveal the sidebar.
    pub fn add_tool(&mut self, kind: &BackendToolType, cx: &mut Context<Self>) {
        let (reply, result) = tokio::sync::oneshot::channel();
        let _ = self.sender.send(PipelineCommand::AddTool {
            kind: kind.clone(),
            reply,
        });
        cx.spawn(async move |_, _| {
            let _ = result.await;
        })
        .detach();
        self.reveal(cx);
    }
    /// Return the current ordered tool summaries.
    fn pipeline_tools(&self) -> Vec<ToolSummary> {
        self.state.tools.clone()
    }
    /// Request removal of a tool and return focus to the sidebar.
    fn remove_tool(&self, id: ToolId, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.sender.send(PipelineCommand::RemoveTool(id));
        self.focus_handle.focus(window, cx);
        cx.notify();
    }
    /// Select a tool and toggle its controls in the sidebar.
    fn toggle_expanded(&mut self, id: ToolId, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.sender.send(PipelineCommand::SelectTool(Some(id)));
        let tool = self.instances.entry(id).or_default();
        tool.expanded = !tool.expanded;
        if tool.expanded {
            self.focus_requested = Some(id);
        } else {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Prompt for a pipeline archive to save or load.
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
                    let (reply, result) = tokio::sync::oneshot::channel();
                    let command = if save {
                        PipelineCommand::SavePipeline { path, reply }
                    } else {
                        PipelineCommand::LoadPipeline {
                            path,
                            cli: false,
                            reply,
                        }
                    };
                    let _ = this.sender.send(command);
                    cx.spawn(async move |_, _| {
                        if let Ok(Err(error)) = result.await {
                            eprintln!("Pipeline file operation failed: {error}");
                        }
                    })
                    .detach();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Update the candidate insertion slot during a tool drag.
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

    /// Queue the tool move represented by the current drop slot.
    fn commit_drop(&mut self, cx: &mut Context<Self>) {
        let dragged = self.drag_tool.take();
        let slot = self.drop_slot.take();
        if let (Some(id), Some(slot)) = (dragged, slot)
            && let Some(before) = reorder_target(&self.pipeline_tools(), id, slot)
        {
            let _ = self.sender.send(PipelineCommand::MoveTool { id, before });
        }
        cx.notify();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep tool card layout and bindings together"
    )]
    /// Build the row for the sidebar view.
    fn render_row(
        &self,
        row: usize,
        count: usize,
        id: ToolId,
        label: String,
        enabled: bool,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        let Some(tool) = self.instances.get(&id) else {
            return div().into_any_element();
        };
        let dragged = self.drag_tool == Some(id);
        let payload = DraggedTool {
            id,
            label: label.clone(),
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
                    .id(("pipeline-tool", id.0))
                    .flex()
                    .flex_none()
                    .flex_col()
                    .rounded_md()
                    .border_1()
                    .border_color(if self.state.selected_tool == Some(id) {
                        theme::ACCENT
                    } else {
                        theme::BORDER_MUTED
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _, _, _| {
                            let _ = this.sender.send(PipelineCommand::SelectTool(Some(id)));
                        }),
                    )
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
                                    .id(("tool-drag-handle", id.0))
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
                                            this.drag_tool = Some(dragged.id);
                                            this.drop_slot = Some(row);
                                            cx.notify();
                                        });
                                        cx.new(|_| ToolDragPreview(dragged.clone()))
                                    }),
                            )
                            .child(
                                Button::new(("tool-expand", id.0))
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
                                            .child(label),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.toggle_expanded(id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("tool-remove", id.0))
                                    .variant(ButtonVariant::Ghost)
                                    .icon(
                                        Icon::x().size(16.).color(theme::TEXT_MUTED),
                                        "Remove tool",
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_tool(id, window, cx);
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

    /// Build the choices offered by the add-tool picker.
    fn available_tools(&self) -> Vec<ToolChoice> {
        self.tools
            .iter()
            .map(|tool| ToolChoice {
                kind: tool.kind.clone(),
                label: tool.label,
            })
            .collect()
    }

    /// Open or close the add-tool picker.
    fn toggle_add_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_menu.take().is_some() {
            self.focus_handle.focus(window, cx);
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
                    this.focus_handle.focus(window, cx);
                    cx.notify();
                }
            },
        )
        .detach();
        menu.read(cx).focus_handle(cx).focus(window, cx);
        self.add_menu = Some(menu);
        cx.notify();
    }

    /// Build the add menu for the sidebar view.
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

impl Render for ToolSidebar {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep sidebar layout and bindings together"
    )]
    /// Build the sidebar view and its interaction handlers.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.drag_tool.is_some() && !cx.has_active_drag() {
            self.drag_tool = None;
            self.drop_slot = None;
        }
        let pipeline = self.pipeline_tools();
        let mut rows = Vec::new();
        let mut occurrences: HashMap<BackendToolType, usize> = HashMap::new();
        for (row, proxy) in pipeline.iter().enumerate() {
            let Some(definition) = self.tools.iter().find(|tool| tool.kind == proxy.kind) else {
                continue;
            };
            let ordinal = occurrences.entry(proxy.kind.clone()).or_default();
            *ordinal = ordinal.saturating_add(1);
            let repeated = pipeline
                .iter()
                .filter(|tool| tool.kind == proxy.kind)
                .count()
                > 1;
            let label = if repeated {
                format!("{} {}", definition.label, ordinal)
            } else {
                definition.label.to_string()
            };
            let tool = self.instances.entry(proxy.id).or_default();
            if tool.expanded {
                if tool.panel.is_none() {
                    tool.panel = Some((definition.factory)(proxy.id, window, cx));
                }
                if self.focus_requested == Some(proxy.id) {
                    if let Some(panel) = &tool.panel {
                        panel.focus.focus(window, cx);
                    }
                    self.scroll.scroll_to_top_of_item(row);
                    self.focus_requested = None;
                }
            }
            rows.push(self.render_row(row, pipeline.len(), proxy.id, label, proxy.is_enabled, cx));
        }
        div()
            .id("pipeline-sidebar")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus_handle)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.add_menu.take().is_some() {
                    this.focus_handle.focus(window, cx);
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
                                this.focus_handle.focus(window, cx);
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

/// Build the visual handle used to drag a tool row.
fn drag_grip() -> impl IntoElement {
    div().flex().gap(px(2.5)).children((0..2).map(|_| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.5))
            .children((0..3).map(|_| div().size(px(1.5)).rounded_full().bg(theme::TEXT_MUTED)))
    }))
}

/// Build an insertion target between sidebar tool rows.
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

/// Resolve a visible insertion slot to the stable ID immediately after it.
#[expect(
    clippy::option_option,
    reason = "Distinguish an ignored drop from a move to the end of the stack"
)]
fn reorder_target(tools: &[ToolSummary], id: ToolId, slot: usize) -> Option<Option<ToolId>> {
    let from = tools.iter().position(|tool| tool.id == id)?;
    if slot > tools.len() || slot == from || slot == from.saturating_add(1) {
        return None;
    }
    Some(tools.get(slot).map(|tool| tool.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Build a pipeline state fixture for sidebar ordering tests.
    fn pipeline() -> Vec<ToolSummary> {
        [2, 5, 9]
            .into_iter()
            .map(|id| ToolSummary {
                id: ToolId(id),
                kind: BackendToolType::Contrast,
                is_enabled: id != 5,
            })
            .collect()
    }
    #[test]
    /// Verify that sidebar reordering follows backend order and includes disabled tools.
    fn reorder_uses_backend_positions_and_includes_disabled_stack_items() {
        assert_eq!(
            reorder_target(&pipeline(), ToolId(9), 0),
            Some(Some(ToolId(2)))
        );
        assert_eq!(reorder_target(&pipeline(), ToolId(2), 3), Some(None));
        assert_eq!(
            reorder_target(&pipeline(), ToolId(9), 1),
            Some(Some(ToolId(5)))
        );
    }
    #[test]
    /// Verify that adjacent and invalid drop positions produce no tool move.
    fn adjacent_and_invalid_drops_do_not_reorder() {
        assert_eq!(reorder_target(&pipeline(), ToolId(5), 1), None);
        assert_eq!(reorder_target(&pipeline(), ToolId(5), 2), None);
        assert_eq!(reorder_target(&pipeline(), ToolId(5), 8), None);
        assert_eq!(reorder_target(&pipeline(), ToolId(30), 0), None);
        assert_eq!(reorder_target(&[], ToolId(5), 0), None);
    }
}
