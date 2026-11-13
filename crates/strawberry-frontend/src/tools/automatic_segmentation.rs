use crate::gui::tool_panel;
use crate::{
    gui::{
        Button, ButtonVariant, ToggleButton,
        icon::Icon,
        theme,
        value_control::{ValueControl, ValueControlConfig, ValueControlEvent},
    },
    tools::{FrontendMessageHandler, spawn_message_processor},
};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Rgba as GpuiRgba, Window, div, prelude::*,
};
use std::sync::{Arc, RwLock};
use strawberry_processing::segmentation::FeatureConfig;
use strawberry_types::*;

#[derive(Clone, Copy)]
enum SigmaKind {
    Gaussian,
    DogSmall,
    DogLarge,
}

impl SigmaKind {
    const fn id(self) -> &'static str {
        match self {
            Self::Gaussian => "segmentation-gaussian-sigma",
            Self::DogSmall => "segmentation-dog-small-sigma",
            Self::DogLarge => "segmentation-dog-large-sigma",
        }
    }

    fn parameters(self, features: &FeatureConfig) -> (f32, f32, f32) {
        match self {
            Self::Gaussian => (features.gaussian_sigma, 0.1, 64.0),
            Self::DogSmall => (
                features.dog_small_sigma,
                0.1,
                (features.dog_large_sigma - 0.1).max(0.1),
            ),
            Self::DogLarge => (
                features.dog_large_sigma,
                (features.dog_small_sigma + 0.1).min(64.0),
                64.0,
            ),
        }
    }
}

#[derive(Clone, Copy)]
enum ControlKind {
    BrushRadius,
    OverlayOpacity,
    Trees,
    Depth,
    MaxParallelTiles,
}

#[derive(Clone, Copy)]
enum FloatControlKind {
    InputScale,
    Threshold,
}

impl ControlKind {
    const fn id(self) -> &'static str {
        match self {
            Self::BrushRadius => "segmentation-brush-radius",
            Self::OverlayOpacity => "segmentation-overlay-opacity",
            Self::Trees => "segmentation-trees",
            Self::Depth => "segmentation-depth",
            Self::MaxParallelTiles => "segmentation-max-parallel-tiles",
        }
    }
}

#[derive(Default)]
struct Controls {
    brush_radius: Option<Entity<ValueControl>>,
    overlay_opacity: Option<Entity<ValueControl>>,
    trees: Option<Entity<ValueControl>>,
    depth: Option<Entity<ValueControl>>,
    input_scale: Option<Entity<ValueControl>>,
    threshold: Option<Entity<ValueControl>>,
    max_parallel_tiles: Option<Entity<ValueControl>>,
    gaussian_sigma: Option<Entity<ValueControl>>,
    dog_small_sigma: Option<Entity<ValueControl>>,
    dog_large_sigma: Option<Entity<ValueControl>>,
}

impl Controls {
    fn float_slot(&mut self, kind: FloatControlKind) -> &mut Option<Entity<ValueControl>> {
        match kind {
            FloatControlKind::InputScale => &mut self.input_scale,
            FloatControlKind::Threshold => &mut self.threshold,
        }
    }

    fn sigma_slot(&mut self, kind: SigmaKind) -> &mut Option<Entity<ValueControl>> {
        match kind {
            SigmaKind::Gaussian => &mut self.gaussian_sigma,
            SigmaKind::DogSmall => &mut self.dog_small_sigma,
            SigmaKind::DogLarge => &mut self.dog_large_sigma,
        }
    }

    fn slot(&mut self, kind: ControlKind) -> &mut Option<Entity<ValueControl>> {
        match kind {
            ControlKind::BrushRadius => &mut self.brush_radius,
            ControlKind::OverlayOpacity => &mut self.overlay_opacity,
            ControlKind::Trees => &mut self.trees,
            ControlKind::Depth => &mut self.depth,
            ControlKind::MaxParallelTiles => &mut self.max_parallel_tiles,
        }
    }
}

pub struct AutomaticSegmentationView {
    pub state: Arc<RwLock<AutomaticSegmentationState>>,
    pub base: super::base::ToolBase,
    controls: Controls,
    show_features: bool,
    show_gaussian_settings: bool,
    show_dog_settings: bool,
    show_forest: bool,
    show_annotations: bool,
    annotations_scroll: gpui::ScrollHandle,
    classes_scroll: gpui::ScrollHandle,
    class_edit: Option<(ClassId, bool, String)>,
    class_edit_select_all: bool,
}
impl Focusable for AutomaticSegmentationView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
}
fn class_color(class: &SegmentationClass) -> GpuiRgba {
    class.color.into()
}
impl AutomaticSegmentationView {
    pub fn new(
        _: tokio::sync::broadcast::Receiver<PipelineCommand>,
        sender: tokio::sync::broadcast::Sender<PipelineCommand>,
        cx: &App,
    ) -> Self {
        Self {
            state: Arc::new(RwLock::new(AutomaticSegmentationState::default())),
            base: super::base::ToolBase::new(
                sender,
                BackendToolType::AutomaticSegmentation,
                cx.focus_handle(),
            ),
            controls: Controls::default(),
            show_features: false,
            show_gaussian_settings: false,
            show_dog_settings: false,
            show_forest: false,
            show_annotations: false,
            annotations_scroll: gpui::ScrollHandle::new(),
            classes_scroll: gpui::ScrollHandle::new(),
            class_edit: None,
            class_edit_select_all: false,
        }
    }
    fn send(&self, message: AutomaticSegmentationMessage) {
        if let Err(error) = self.base.send_to_backend(PipelineCommand::PipelineObject(
            PipelineObjectMessage::AutomaticSegmentationMessage(message),
        )) {
            eprintln!("{error}");
        }
    }
    fn button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        message: AutomaticSegmentationMessage,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .label(label)
            .on_click(cx.listener(move |this, _, _, _| this.send(message.clone())))
    }
    fn file_button(
        &self,
        id: &'static str,
        label: &'static str,
        save: bool,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .compact()
            .flex_1()
            .min_w_0()
            .label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.file(save, window, cx)))
    }
    fn commit_class_edit(&mut self) {
        let Some((id, color, draft)) = &self.class_edit else {
            return;
        };
        let message = if *color {
            let text = draft.trim().trim_start_matches('#');
            if text.len() != 6 {
                return;
            }
            let Ok(value) = u32::from_str_radix(text, 16) else {
                return;
            };
            AutomaticSegmentationMessage::SetClassColor(*id, RgbColor::from_hex(value))
        } else {
            if draft.trim().is_empty() {
                return;
            }
            AutomaticSegmentationMessage::RenameClass(*id, draft.clone())
        };
        self.send(message);
        self.class_edit = None;
    }
    fn class_edit_field(&self, draft: &str, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("class-editor")
            .track_focus(&self.base.focus_handle)
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .h(gpui::px(28.))
            .px_2()
            .rounded_sm()
            .bg(theme::SURFACE_DEEP)
            .border_1()
            .border_color(theme::BORDER)
            .focus(|style| style.border_color(theme::ACCENT))
            .cursor(gpui::CursorStyle::IBeam)
            .text_sm()
            .child(
                div()
                    .truncate()
                    .when(self.class_edit_select_all, |el| el.bg(theme::ACCENT))
                    .child(draft.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .w(gpui::px(1.))
                    .h(gpui::px(14.))
                    .bg(theme::TEXT),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.base.focus_handle.focus(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                let Some((_, _, draft)) = &mut this.class_edit else {
                    return;
                };
                match event.keystroke.key.as_str() {
                    "escape" => {
                        this.class_edit = None;
                    }
                    "enter" => {
                        this.commit_class_edit();
                    }
                    "a" if event.keystroke.modifiers.platform => {
                        this.class_edit_select_all = true;
                    }
                    "v" if event.keystroke.modifiers.platform => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            if this.class_edit_select_all {
                                draft.clear();
                            }
                            this.class_edit_select_all = false;
                            draft.push_str(text.lines().next().unwrap_or_default());
                        }
                    }
                    "backspace" => {
                        if event.keystroke.modifiers.platform || this.class_edit_select_all {
                            draft.clear();
                        } else {
                            draft.pop();
                        }
                        this.class_edit_select_all = false;
                    }
                    _ if event.keystroke.modifiers.platform
                        || event.keystroke.modifiers.control => {}
                    _ => {
                        if let Some(text) = &event.keystroke.key_char {
                            if !text.chars().any(char::is_control) {
                                if this.class_edit_select_all {
                                    draft.clear();
                                }
                                this.class_edit_select_all = false;
                                draft.push_str(text);
                            }
                        }
                    }
                }
                cx.stop_propagation();
                cx.notify();
            }))
    }
    fn classes(&self, state: &AutomaticSegmentationState, cx: &Context<Self>) -> gpui::Div {
        let mut list = tool_panel::list_for_tool(
            "segmentation-classes",
            &self.classes_scroll,
            &self.base.focus_handle,
        );
        for (&id, class) in &state.classes {
            let selected = state.active_class == id;
            let name = class.name.clone();
            let color = class.color;
            let count = class.annotation_count();
            let editing = self
                .class_edit
                .as_ref()
                .filter(|(editing_id, _, _)| *editing_id == id);
            let renaming = editing.is_some_and(|(_, color, _)| !color);
            let mut row = tool_panel::row()
                .id(gpui::ElementId::Name(format!("class-row-{id}").into()))
                .w_full()
                .role(gpui::Role::RadioButton)
                .aria_label(format!("Select {name}"))
                .aria_toggled(if selected {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .focusable()
                .cursor_pointer()
                .border_color(theme::BORDER_MUTED)
                .bg(if selected {
                    theme::SURFACE_ALT
                } else {
                    theme::SURFACE_DEEP
                })
                .hover(|style| style.bg(theme::SURFACE_ALT))
                .focus_visible(|style| style.bg(theme::CONTROL))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if renaming {
                        return;
                    }
                    this.send(AutomaticSegmentationMessage::SetBrush(
                        AnnotationKind::Paint(id),
                    ));
                    this.class_edit = None;
                    cx.notify();
                }))
                .child(tool_panel::radio_indicator(selected, theme::ACCENT));
            if let Some((_, false, draft)) = editing {
                row = row
                    .child(
                        self.class_edit_field(draft, cx)
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM),
                    )
                    .child(
                        Button::new("save-class-name")
                            .compact()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.commit_class_edit();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("cancel-class-name")
                            .variant(ButtonVariant::Ghost)
                            .icon(Icon::x(), "Cancel rename")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.class_edit = None;
                                cx.notify();
                            })),
                    );
            } else {
                row = row
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("class-name-{id}").into()))
                            .role(gpui::Role::Button)
                            .aria_label(format!("Select {name}"))
                            .focusable()
                            .cursor_pointer()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .text_xs()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(name.clone())
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    if event.click_count() != 2 {
                                        return;
                                    }
                                    cx.stop_propagation();
                                    this.class_edit = Some((id, false, name.clone()));
                                    this.class_edit_select_all = true;
                                    this.base.focus_handle.focus(window, cx);
                                    cx.notify();
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .min_w(gpui::px(18.))
                            .text_right()
                            .text_color(if selected {
                                theme::TEXT
                            } else {
                                theme::TEXT_MUTED
                            })
                            .child(count.to_string()),
                    )
                    .child(
                        Button::new(gpui::ElementId::Name(format!("class-color-{id}").into()))
                            .variant(ButtonVariant::Ghost)
                            .compact()
                            .p_0()
                            .flex_none()
                            .size(gpui::px(24.))
                            .min_h(gpui::px(24.))
                            .aria_label(format!("Change {} color", class.name))
                            .tooltip_text("Change color")
                            .child(
                                div()
                                    .size(gpui::px(10.))
                                    .rounded_full()
                                    .bg(class_color(class)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.class_edit = Some((id, true, format!("{:06X}", color.hex())));
                                this.class_edit_select_all = true;
                                this.base.focus_handle.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(gpui::ElementId::Name(format!("class-visible-{id}").into()))
                            .variant(ButtonVariant::Ghost)
                            .icon(
                                if state.hidden_classes.contains(&id) {
                                    Icon::eye_closed()
                                } else {
                                    Icon::eye_open()
                                },
                                "Toggle class visibility",
                            )
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.send(AutomaticSegmentationMessage::ToggleClassVisibility(id))
                            })),
                    )
                    .child(
                        Button::new(gpui::ElementId::Name(format!("delete-class-{id}").into()))
                            .variant(ButtonVariant::Delete)
                            .tooltip_text("Delete class")
                            .aria_label("Delete class")
                            .disabled(state.classes.len() <= 2)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.class_edit = None;
                                this.send(AutomaticSegmentationMessage::DeleteClass(id));
                                cx.notify();
                            })),
                    );
            }
            let mut entry = div().flex().flex_col().flex_none().min_w_0().child(row);
            if let Some((_, true, draft)) = editing {
                let mut palette = div().flex().flex_wrap().gap_1();
                for (index, color) in CLASS_PALETTE.iter().copied().enumerate() {
                    palette = palette.child(
                        Button::new(gpui::ElementId::Name(
                            format!("palette-{id}-{index}").into(),
                        ))
                        .variant(ButtonVariant::Ghost)
                        .p_0()
                        .size(gpui::px(24.))
                        .min_h(gpui::px(24.))
                        .aria_label(format!("Color #{:06X}", color.hex()))
                        .child(
                            div()
                                .size(gpui::px(14.))
                                .rounded_full()
                                .bg(GpuiRgba::from(color)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(AutomaticSegmentationMessage::SetClassColor(id, color));
                            this.class_edit = None;
                            cx.notify();
                        })),
                    );
                }
                entry = entry.child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap_2()
                        .p_2()
                        .bg(theme::SURFACE_DEEP)
                        .child(palette)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .min_w_0()
                                .gap_2()
                                .child(div().text_xs().text_color(theme::TEXT_MUTED).child("Hex"))
                                .child(self.class_edit_field(draft, cx))
                                .child(
                                    Button::new("save-class-color")
                                        .compact()
                                        .label("Save")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.commit_class_edit();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("cancel-class-color")
                                        .variant(ButtonVariant::Ghost)
                                        .icon(Icon::x(), "Cancel color edit")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.class_edit = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
            }
            list = list.child(entry);
        }
        let header = div()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .child("Classes");
        let add = Button::new("add-class")
            .compact()
            .w_full()
            .child(Icon::plus())
            .label("Add class")
            .disabled(state.classes.len() >= 256)
            .on_click(cx.listener(|this, _, _, cx| {
                this.send(AutomaticSegmentationMessage::AddClass);
                this.class_edit = None;
                cx.notify();
            }));
        if state.training {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .min_w_0()
                .child(header)
                .child(list)
                .child(add)
        } else {
            tool_panel::section_with_header(header)
                .child(list)
                .child(add)
        }
    }
    fn annotations(&self, state: &AutomaticSegmentationState, cx: &Context<Self>) -> gpui::Div {
        let mut list = tool_panel::list("segmentation-annotations", &self.annotations_scroll);
        if state.current_annotations().next().is_none() {
            list = list.child(
                div()
                    .p_3()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("No annotations"),
            );
        }
        for (ordinal, (kind, index, stroke)) in state.current_annotations().enumerate() {
            let (name, color) = match kind {
                AnnotationKind::Paint(id) => state
                    .classes
                    .get(&id)
                    .map(|c| (c.name.as_str(), class_color(c)))
                    .unwrap_or(("Unknown", theme::TEXT_MUTED)),
                AnnotationKind::Eraser => ("Eraser", theme::TEXT_MUTED),
            };
            list = list.child(
                tool_panel::row()
                    .bg(if state.selected_stroke == Some(index) {
                        theme::SURFACE_ALT
                    } else {
                        theme::SURFACE_DEEP
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(gpui::px(6.))
                            .rounded_full()
                            .bg(color),
                    )
                    .child(
                        self.button(
                            ("segmentation-stroke", index),
                            format!("{name} {}", ordinal.saturating_add(1)),
                            AutomaticSegmentationMessage::SelectStroke(index),
                            cx,
                        )
                        .variant(ButtonVariant::Ghost)
                        .compact()
                        .flex_1()
                        .min_w_0()
                        .justify_start(),
                    )
                    .child(
                        Button::new(("segmentation-visible", index))
                            .variant(ButtonVariant::Ghost)
                            .icon(
                                if stroke.visible {
                                    Icon::eye_open()
                                } else {
                                    Icon::eye_closed()
                                },
                                "Toggle annotation visibility",
                            )
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.send(AutomaticSegmentationMessage::ToggleStrokeVisibility(
                                    index,
                                ))
                            })),
                    )
                    .child(
                        Button::new(("segmentation-delete", index))
                            .variant(ButtonVariant::Delete)
                            .aria_label("Delete annotation")
                            .tooltip_text("Delete annotation")
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.send(AutomaticSegmentationMessage::DeleteStroke(index))
                            })),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Button::new("segmentation-strokes-disclosure")
                            .variant(ButtonVariant::Ghost)
                            .compact()
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .px_0()
                            .child(Icon::disclosure(self.show_annotations))
                            .label(format!("Strokes ({})", state.current_annotations().count()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_annotations = !this.show_annotations;
                                cx.notify();
                            })),
                    )
                    .child(
                        self.button(
                            "segmentation-undo",
                            "Undo",
                            AutomaticSegmentationMessage::UndoStroke,
                            cx,
                        )
                        .variant(ButtonVariant::Ghost)
                        .compact()
                        .disabled(state.current_annotations().next().is_none()),
                    )
                    .child(
                        self.button(
                            "segmentation-clear",
                            "Clear",
                            AutomaticSegmentationMessage::ClearAnnotations,
                            cx,
                        )
                        .variant(ButtonVariant::Ghost)
                        .compact()
                        .disabled(state.current_annotations().next().is_none()),
                    ),
            )
            .when(self.show_annotations, |el| el.child(list))
    }
    fn feature_controls(
        &mut self,
        s: &AutomaticSegmentationState,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let disclosure_width = gpui::px(28.);
        let mut section = tool_panel::section_with_header(
            Button::new("segmentation-feature-settings")
                .variant(ButtonVariant::Ghost)
                .w_full()
                .justify_start()
                .px_0()
                .gap_1()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(
                    div()
                        .w(disclosure_width)
                        .flex_none()
                        .flex()
                        .justify_center()
                        .child(Icon::disclosure(self.show_features)),
                )
                .label("Features")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_features = !this.show_features;
                    cx.notify();
                })),
        );
        if !self.show_features {
            return section;
        }
        for (label, enabled) in [
            ("Gaussian blur", s.features.gaussian),
            ("Sobel", s.features.sobel),
            ("Difference of Gaussians", s.features.dog),
            ("Hessian", s.features.hessian),
        ] {
            let mut features = s.features.clone();
            match label {
                "Gaussian blur" => features.gaussian = !enabled,
                "Sobel" => features.sobel = !enabled,
                "Difference of Gaussians" => features.dog = !enabled,
                _ => features.hessian = !enabled,
            }
            let expanded = match label {
                "Gaussian blur" => Some(self.show_gaussian_settings),
                "Difference of Gaussians" => Some(self.show_dog_settings),
                _ => None,
            };
            let mut disclosure = div().size(disclosure_width).flex_none();
            if let Some(expanded) = expanded {
                disclosure = disclosure.child(
                    Button::new(gpui::ElementId::Name(
                        format!("segmentation-feature-{label}").into(),
                    ))
                    .variant(ButtonVariant::Ghost)
                    .icon(Icon::disclosure(expanded), format!("{label} settings"))
                    .w(disclosure_width)
                    .min_w(disclosure_width)
                    .aria_expanded(expanded)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match label {
                            "Gaussian blur" => {
                                this.show_gaussian_settings = !this.show_gaussian_settings
                            }
                            _ => this.show_dog_settings = !this.show_dog_settings,
                        }
                        cx.notify();
                    })),
                );
            }
            let row = div()
                .flex()
                .items_center()
                .px(gpui::px(1.))
                .gap_1()
                .child(disclosure)
                .child(
                    div().flex_1().min_w_0().child(
                        ToggleButton::new(
                            label.into(),
                            enabled,
                            move |this: &mut Self, _, _, _| {
                                this.send(AutomaticSegmentationMessage::SetFeatures(
                                    features.clone(),
                                ))
                            },
                        )
                        .with_no_margin()
                        .render(cx),
                    ),
                );
            let mut feature = div().flex().flex_col().gap_2().child(row);
            if expanded == Some(true) {
                let mut settings = div().flex().flex_col().gap_2().pl_8();
                match label {
                    "Gaussian blur" => {
                        settings = settings.child(self.sigma_control(
                            SigmaKind::Gaussian,
                            "Gaussian sigma (px)",
                            &s.features,
                            cx,
                        ));
                    }
                    "Difference of Gaussians" => {
                        settings = settings
                            .child(self.sigma_control(
                                SigmaKind::DogSmall,
                                "Small sigma (px)",
                                &s.features,
                                cx,
                            ))
                            .child(self.sigma_control(
                                SigmaKind::DogLarge,
                                "Large sigma (px)",
                                &s.features,
                                cx,
                            ));
                    }
                    _ => {}
                }
                feature = feature.child(settings);
            }
            section = section.child(feature);
        }
        let mut channels = tool_panel::segmented();
        for (rgb, label) in [(false, "Luminance"), (true, "RGB")] {
            let mut features = s.features.clone();
            features.rgb = rgb;
            channels = channels.child(
                self.button(
                    gpui::ElementId::Name(format!("segmentation-channel-{label}").into()),
                    label,
                    AutomaticSegmentationMessage::SetFeatures(features),
                    cx,
                )
                .variant(ButtonVariant::Ghost)
                .compact()
                .selected(s.features.rgb == rgb)
                .flex_1()
                .min_w_0(),
            );
        }
        section
            .child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("Channels"),
            )
            .child(channels)
    }
    fn file(&self, save: bool, window: &Window, cx: &Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let dialog = rfd::FileDialog::new().add_filter("Segmentation model", &["sbm"]);
            let path = if save {
                dialog.set_file_name("model.sbm").save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(path) = path {
                if !save {
                    let has_annotations = this
                        .read_with(cx, |this, _| {
                            this.state
                                .read()
                                .map(|s| s.annotation_count() > 0)
                                .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if has_annotations
                        && rfd::MessageDialog::new()
                            .set_title("Load model?")
                            .set_description(
                                "Loading a model replaces classes and clears training annotations.",
                            )
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show()
                            != rfd::MessageDialogResult::Ok
                    {
                        return;
                    }
                }
                let _ = this.update(cx, |this, _| {
                    this.send(if save {
                        AutomaticSegmentationMessage::SaveModel(path)
                    } else {
                        AutomaticSegmentationMessage::LoadModel(path)
                    });
                });
            }
        })
        .detach();
    }

    fn float_control(
        &mut self,
        kind: FloatControlKind,
        value: f32,
        cx: &mut Context<Self>,
    ) -> Entity<ValueControl> {
        if let Some(control) = self.controls.float_slot(kind) {
            return control.clone();
        }
        let (id, label, min, step) = match kind {
            FloatControlKind::InputScale => (
                "segmentation-input-scale",
                "Input scale multiplier",
                0.01,
                0.05,
            ),
            FloatControlKind::Threshold => ("segmentation-threshold", "Threshold", 0.0, 0.01),
        };
        let control = cx.new(|cx| {
            ValueControl::new(
                format!("{value:.2}"),
                value,
                ValueControlConfig {
                    id: id.into(),
                    label: label.into(),
                    min,
                    max: 1.0,
                    step,
                },
                cx,
            )
        });
        cx.subscribe(
            &control,
            move |this, _, event: &ValueControlEvent, _| match kind {
                FloatControlKind::InputScale => {
                    let mut forest = this
                        .state
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .forest
                        .clone();
                    forest.input_scale = event.value;
                    this.send(AutomaticSegmentationMessage::SetForest(forest));
                }
                FloatControlKind::Threshold => {
                    this.send(AutomaticSegmentationMessage::SetThreshold(event.value))
                }
            },
        )
        .detach();
        *self.controls.float_slot(kind) = Some(control.clone());
        control
    }

    fn sigma_control(
        &mut self,
        kind: SigmaKind,
        label: &str,
        features: &FeatureConfig,
        cx: &mut Context<Self>,
    ) -> Entity<ValueControl> {
        if let Some(control) = self.controls.sigma_slot(kind) {
            return control.clone();
        }
        let (value, min, max) = kind.parameters(features);
        let control = cx.new(|cx| {
            ValueControl::new(
                format!("{value:.2}"),
                value,
                ValueControlConfig {
                    id: kind.id().into(),
                    label: label.into(),
                    min,
                    max,
                    step: 0.1,
                },
                cx,
            )
        });
        cx.subscribe(&control, move |this, _, event: &ValueControlEvent, _| {
            let mut features = this
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .features
                .clone();
            match kind {
                SigmaKind::Gaussian => features.gaussian_sigma = event.value,
                SigmaKind::DogSmall => features.dog_small_sigma = event.value,
                SigmaKind::DogLarge => features.dog_large_sigma = event.value,
            }
            this.send(AutomaticSegmentationMessage::SetFeatures(features));
        })
        .detach();
        *self.controls.sigma_slot(kind) = Some(control.clone());
        control
    }

    fn integer_control(
        &mut self,
        kind: ControlKind,
        label: &str,
        value: u32,
        min: u32,
        max: u32,
        cx: &mut Context<Self>,
    ) -> Entity<ValueControl> {
        if let Some(control) = self.controls.slot(kind) {
            return control.clone();
        }
        let control = cx.new(|cx| {
            ValueControl::new_integer(
                value,
                ValueControlConfig {
                    id: kind.id().into(),
                    label: label.into(),
                    min: num_traits::cast(min).unwrap_or(0.),
                    max: num_traits::cast(max).unwrap_or(0.),
                    step: 1.,
                },
                cx,
            )
        });
        cx.subscribe(&control, move |this, _, event: &ValueControlEvent, _| {
            let Some(integer) = num_traits::ToPrimitive::to_u32(&event.value) else {
                return;
            };
            let mut forest = this
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .forest
                .clone();
            match kind {
                ControlKind::OverlayOpacity => {
                    if let Ok(opacity) = u8::try_from(integer) {
                        this.send(AutomaticSegmentationMessage::SetOverlayOpacity(opacity));
                    }
                }
                ControlKind::BrushRadius => {
                    if let Ok(radius) = BrushRadius::try_from(integer) {
                        this.send(AutomaticSegmentationMessage::SetBrushRadius(radius));
                    }
                }
                ControlKind::Trees => {
                    if let Ok(trees) = u16::try_from(integer) {
                        forest.trees = trees;
                        this.send(AutomaticSegmentationMessage::SetForest(forest));
                    }
                }
                ControlKind::Depth => {
                    if let Ok(depth) = u16::try_from(integer) {
                        forest.depth = depth;
                        this.send(AutomaticSegmentationMessage::SetForest(forest));
                    }
                }
                ControlKind::MaxParallelTiles => {
                    if let Ok(limit) = u16::try_from(integer) {
                        let mut inference = this
                            .state
                            .read()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .inference
                            .clone();
                        inference.max_parallel_tiles = limit;
                        this.send(AutomaticSegmentationMessage::SetInference(inference));
                    }
                }
            }
        })
        .detach();
        *self.controls.slot(kind) = Some(control.clone());
        control
    }
}
impl FrontendMessageHandler for AutomaticSegmentationView {
    type State = AutomaticSegmentationState;
    fn state_arc(&self) -> Arc<RwLock<Self::State>> {
        self.state.clone()
    }
    fn base(&self) -> &super::ToolBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut super::ToolBase {
        &mut self.base
    }
    fn extract_state(state: ToolState) -> Option<Self::State> {
        if let ToolState::AutomaticSegmentationState(state) = state {
            Some(state)
        } else {
            None
        }
    }
    fn on_state_changed(&mut self, state: &Self::State, cx: &mut Context<Self>) {
        for kind in [
            SigmaKind::Gaussian,
            SigmaKind::DogSmall,
            SigmaKind::DogLarge,
        ] {
            if let Some(control) = self.controls.sigma_slot(kind) {
                let (value, min, max) = kind.parameters(&state.features);
                control.update(cx, |control, cx| {
                    control.config.min = min;
                    control.config.max = max;
                    control.set_value(value);
                    cx.notify();
                });
            }
        }
        for (kind, value) in [
            (ControlKind::BrushRadius, state.brush_radius.get()),
            (
                ControlKind::OverlayOpacity,
                u32::from(state.overlay_opacity),
            ),
            (ControlKind::Trees, u32::from(state.forest.trees)),
            (ControlKind::Depth, u32::from(state.forest.depth)),
            (
                ControlKind::MaxParallelTiles,
                u32::from(state.inference.max_parallel_tiles),
            ),
        ] {
            if let Some(control) = self.controls.slot(kind) {
                control.update(cx, |control, cx| {
                    let value = num_traits::cast(value).unwrap_or(0.);
                    if control.value != value {
                        control.set_value(value);
                        cx.notify();
                    }
                });
            }
        }
        for (kind, value) in [
            (FloatControlKind::InputScale, state.forest.input_scale),
            (FloatControlKind::Threshold, state.threshold_value),
        ] {
            if let Some(control) = self.controls.float_slot(kind) {
                control.update(cx, |control, cx| {
                    if control.value != value {
                        control.set_value(value);
                        cx.notify();
                    }
                });
            }
        }
    }
}
impl ToolFrontend for AutomaticSegmentationView {
    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.base.connect() {
            spawn_message_processor(self, window, cx);
        }
    }
}
impl Render for AutomaticSegmentationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let enabled = s.is_enabled;
        let mut modes = tool_panel::segmented();
        for (training, label) in [(true, "Training"), (false, "Inference")] {
            modes = modes.child(
                self.button(
                    gpui::ElementId::Name(format!("segmentation-mode-{label}").into()),
                    label,
                    AutomaticSegmentationMessage::SetTraining(training),
                    cx,
                )
                .variant(ButtonVariant::Ghost)
                .selected(s.training == training)
                .flex_1()
                .min_w_0(),
            );
        }
        let mut panel = tool_panel::panel()
            .child(
                ToggleButton::new(
                    "Enabled".into(),
                    enabled,
                    move |this: &mut Self, _, _, _| {
                        if let Err(error) =
                            this.base.send_to_backend(PipelineCommand::PipelineObject(
                                PipelineObjectMessage::ToggleActive(!enabled),
                            ))
                        {
                            eprintln!("{error}");
                        }
                    },
                )
                .with_no_margin()
                .render(cx),
            )
            .child(modes);
        if s.total_layers > 1 {
            panel = panel.child(
                tool_panel::section("Input layer").child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children((0..s.total_layers).map(|i| {
                            self.button(
                                ("segmentation-layer", i),
                                format!("{}", i.saturating_add(1)),
                                AutomaticSegmentationMessage::SelectLayer(i),
                                cx,
                            )
                            .compact()
                            .selected(i == s.selected_layer)
                        })),
                ),
            );
        }
        if s.training {
            let mut brushes = tool_panel::segmented();
            for (label, kind) in [
                ("Paint", AnnotationKind::Paint(s.active_class)),
                ("Erase", AnnotationKind::Eraser),
            ] {
                brushes = brushes.child(
                    self.button(
                        gpui::ElementId::Name(format!("segmentation-brush-{label}").into()),
                        label,
                        AutomaticSegmentationMessage::SetBrush(kind),
                        cx,
                    )
                    .variant(ButtonVariant::Ghost)
                    .compact()
                    .selected(s.brush == kind)
                    .flex_1()
                    .min_w_0(),
                );
            }
            let radius = self.integer_control(
                ControlKind::BrushRadius,
                "Brush radius (px)",
                s.brush_radius.get(),
                BrushRadius::MIN,
                BrushRadius::MAX,
                cx,
            );
            panel = panel.child(
                tool_panel::section("Annotate")
                    .child(brushes)
                    .child(self.classes(&s, cx))
                    .child(radius)
                    .child(div().h(gpui::px(1.)).bg(theme::BORDER_MUTED))
                    .child(self.annotations(&s, cx)),
            );

            let mut preview = tool_panel::section("Preview").child(
                ToggleButton::new(
                    "Prediction overlay".into(),
                    s.show_overlay,
                    move |this: &mut Self, _, _, _| {
                        this.send(AutomaticSegmentationMessage::SetShowOverlay(
                            !s.show_overlay,
                        ));
                    },
                )
                .with_no_margin()
                .render(cx),
            );
            let mut scope = tool_panel::segmented();
            for (selected, label) in [(false, "All classes"), (true, "Selected class")] {
                scope = scope.child(
                    self.button(
                        gpui::ElementId::Name(format!("preview-{selected}").into()),
                        label,
                        AutomaticSegmentationMessage::SetPreviewSelected(selected),
                        cx,
                    )
                    .variant(ButtonVariant::Ghost)
                    .compact()
                    .selected(s.preview_selected == selected)
                    .flex_1()
                    .min_w_0(),
                );
            }
            preview = preview.child(scope);
            if s.show_overlay {
                preview = preview.child(self.integer_control(
                    ControlKind::OverlayOpacity,
                    "Opacity (%)",
                    u32::from(s.overlay_opacity),
                    0,
                    100,
                    cx,
                ));
            }
            panel = panel.child(preview).child(self.feature_controls(&s, cx));
            let mut classifier = tool_panel::section("Classifier").child(
                Button::new("segmentation-forest-settings")
                    .variant(ButtonVariant::Ghost)
                    .compact()
                    .justify_start()
                    .child(Icon::disclosure(self.show_forest))
                    .label("Forest settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_forest = !this.show_forest;
                        cx.notify();
                    })),
            );
            if self.show_forest {
                classifier = classifier
                    .child(self.integer_control(
                        ControlKind::Trees,
                        "Trees",
                        u32::from(s.forest.trees),
                        1,
                        4096,
                        cx,
                    ))
                    .child(self.integer_control(
                        ControlKind::Depth,
                        "Maximum depth",
                        u32::from(s.forest.depth),
                        1,
                        256,
                        cx,
                    ))
                    .child(self.float_control(
                        FloatControlKind::InputScale,
                        s.forest.input_scale,
                        cx,
                    ));
                for (label, enabled) in [
                    ("Balance classes", s.forest.balance_classes),
                    ("Auto-context", s.forest.autocontext),
                ] {
                    classifier = classifier.child(
                        ToggleButton::new(
                            label.into(),
                            enabled,
                            move |this: &mut Self, _, _, _| {
                                let mut forest = this
                                    .state
                                    .read()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .forest
                                    .clone();
                                match label {
                                    "Auto-context" => forest.autocontext = !forest.autocontext,
                                    _ => forest.balance_classes = !forest.balance_classes,
                                }
                                this.send(AutomaticSegmentationMessage::SetForest(forest));
                            },
                        )
                        .with_no_margin()
                        .render(cx),
                    );
                }
            }
            panel = panel.child(
                classifier.child(
                    Button::new("segmentation-train")
                        .variant(ButtonVariant::Success)
                        .large()
                        .w_full()
                        .label(if s.busy { "Training…" } else { "Train model" })
                        .disabled(
                            s.busy
                                || !s.is_enabled
                                || s.classes
                                    .values()
                                    .any(|class| class.annotation_count() == 0),
                        )
                        .on_click(cx.listener(|this, _, _, _| {
                            this.send(AutomaticSegmentationMessage::TrainModel)
                        })),
                ),
            );
        } else {
            panel = panel.child(self.classes(&s, cx));
            let mut inference =
                tool_panel::section("Inference settings").child(self.integer_control(
                    ControlKind::MaxParallelTiles,
                    "Maximum parallel tiles",
                    u32::from(s.inference.max_parallel_tiles),
                    1,
                    256,
                    cx,
                ));
            let mut outputs = tool_panel::segmented();
            for (mode, label) in [
                (SegmentationOutputMode::ClassMask, "Class mask"),
                (SegmentationOutputMode::ClassColors, "Class colors"),
            ] {
                outputs = outputs.child(
                    self.button(
                        gpui::ElementId::Name(format!("output-{label}").into()),
                        label,
                        AutomaticSegmentationMessage::SetOutputMode(mode),
                        cx,
                    )
                    .variant(ButtonVariant::Ghost)
                    .selected(s.output_mode == mode)
                    .flex_1()
                    .min_w_0(),
                );
            }
            inference = inference.child(outputs);
            if s.output_mode == SegmentationOutputMode::ClassMask {
                for (&id, class) in &s.classes {
                    inference = inference.child(
                        self.button(
                            gpui::ElementId::Name(format!("output-class-{}", id).into()),
                            class.name.clone(),
                            AutomaticSegmentationMessage::SetOutputClass(id),
                            cx,
                        )
                        .compact()
                        .variant(ButtonVariant::Ghost)
                        .selected(s.output_class == id)
                        .justify_start(),
                    );
                }
                inference = inference.child(
                    ToggleButton::new(
                        "Thresholding".into(),
                        s.use_threshold,
                        move |this: &mut Self, _, _, _| {
                            this.send(AutomaticSegmentationMessage::ToggleThreshold(
                                !s.use_threshold,
                            ));
                        },
                    )
                    .with_no_margin()
                    .render(cx),
                );
                if s.use_threshold {
                    inference = inference.child(self.float_control(
                        FloatControlKind::Threshold,
                        s.threshold_value,
                        cx,
                    ));
                }
            }
            panel = panel.child(inference);
        }
        let mut model =
            tool_panel::section("Model").child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(gpui::px(6.0)).rounded_full().flex_none().bg(
                        if s.model_ready {
                            theme::SUCCESS
                        } else {
                            theme::TEXT_MUTED
                        },
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(if s.model_ready { "Ready" } else { "Not loaded" }),
                    ),
            );
        if let Some(error) = s.oob_error {
            model = model.child(
                div()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child(format!("OOB Error: {:.2}%", error * 100.0)),
            );
        }
        if let Some(name) = s.model_path.as_ref().and_then(|p| p.file_name()) {
            model = model.child(
                div()
                    .min_w_0()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .truncate()
                    .child(name.to_string_lossy().into_owned()),
            );
        }
        panel = panel.child(
            model.child(
                tool_panel::file_actions()
                    .child(self.file_button("segmentation-load-model", "Load model", false, cx))
                    .child(
                        self.file_button("segmentation-save-model", "Save model", true, cx)
                            .disabled(!s.model_ready),
                    ),
            ),
        );
        panel
    }
}
