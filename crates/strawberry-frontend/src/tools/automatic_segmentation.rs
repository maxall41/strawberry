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
use gpui::{App, Context, Entity, FocusHandle, Focusable, Window, div, prelude::*};
use std::sync::{Arc, RwLock};
use strawberry_types::*;

#[derive(Clone, Copy)]
enum ControlKind {
    BrushRadius,
    Trees,
    Depth,
    MaxParallelTiles,
}

impl ControlKind {
    const fn id(self) -> &'static str {
        match self {
            Self::BrushRadius => "segmentation-brush-radius",
            Self::Trees => "segmentation-trees",
            Self::Depth => "segmentation-depth",
            Self::MaxParallelTiles => "segmentation-max-parallel-tiles",
        }
    }
}

#[derive(Default)]
struct Controls {
    brush_radius: Option<Entity<ValueControl>>,
    trees: Option<Entity<ValueControl>>,
    depth: Option<Entity<ValueControl>>,
    input_scale: Option<Entity<ValueControl>>,
    max_parallel_tiles: Option<Entity<ValueControl>>,
}

impl Controls {
    fn slot(&mut self, kind: ControlKind) -> &mut Option<Entity<ValueControl>> {
        match kind {
            ControlKind::BrushRadius => &mut self.brush_radius,
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
    show_forest: bool,
    annotations_scroll: gpui::ScrollHandle,
}
impl Focusable for AutomaticSegmentationView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.base.focus_handle.clone()
    }
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
            show_forest: false,
            annotations_scroll: gpui::ScrollHandle::new(),
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
    fn annotations(&self, state: &AutomaticSegmentationState, cx: &Context<Self>) -> gpui::Div {
        let mut list = tool_panel::list("segmentation-annotations", &self.annotations_scroll);
        if state.annotations.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("No annotations"),
            );
        }
        for (index, stroke) in state.annotations.iter().enumerate() {
            let (name, color) = match stroke.kind {
                AnnotationKind::Foreground => ("Foreground", theme::SUCCESS),
                AnnotationKind::Background => ("Background", theme::DANGER),
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
                            format!("{name} {}", index.saturating_add(1)),
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
        tool_panel::section("Training annotations").child(list)
    }
    fn feature_controls(&self, s: &AutomaticSegmentationState, cx: &Context<Self>) -> gpui::Div {
        let mut section = tool_panel::section("Features");
        for (label, enabled) in [
            ("Gaussian blur", s.features.gaussian),
            ("Sobel", s.features.sobel),
            ("Difference of Gaussians", s.features.dog),
        ] {
            let mut features = s.features.clone();
            match label {
                "Gaussian blur" => features.gaussian = !enabled,
                "Sobel" => features.sobel = !enabled,
                _ => features.dog = !enabled,
            }
            section = section.child(
                ToggleButton::new(label.into(), enabled, move |this: &mut Self, _, _, _| {
                    this.send(AutomaticSegmentationMessage::SetFeatures(features.clone()))
                })
                .with_no_margin()
                .render(cx),
            );
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

    fn input_scale_control(&mut self, value: f32, cx: &mut Context<Self>) -> Entity<ValueControl> {
        if let Some(control) = &self.controls.input_scale {
            return control.clone();
        }
        let control = cx.new(|cx| {
            ValueControl::new(
                format!("{value:.2}"),
                value,
                ValueControlConfig {
                    id: "segmentation-input-scale".into(),
                    label: "Input scale multiplier".into(),
                    min: 0.01,
                    max: 1.0,
                    step: 0.05,
                },
                cx,
            )
        });
        cx.subscribe(&control, |this, _, event: &ValueControlEvent, _| {
            let mut forest = this
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .forest
                .clone();
            forest.input_scale = event.value;
            this.send(AutomaticSegmentationMessage::SetForest(forest));
        })
        .detach();
        self.controls.input_scale = Some(control.clone());
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
        for (kind, value) in [
            (ControlKind::BrushRadius, state.brush_radius.get()),
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
        if let Some(control) = &self.controls.input_scale {
            control.update(cx, |control, cx| {
                if control.value != state.forest.input_scale {
                    control.set_value(state.forest.input_scale);
                    cx.notify();
                }
            });
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
                ("Foreground", AnnotationKind::Foreground),
                ("Background", AnnotationKind::Background),
                ("Eraser", AnnotationKind::Eraser),
            ] {
                let selected = s.brush == kind;
                let message = AutomaticSegmentationMessage::SetBrush(kind);
                brushes = brushes.child(
                    self.button(
                        gpui::ElementId::Name(format!("segmentation-brush-{label}").into()),
                        label,
                        message,
                        cx,
                    )
                    .variant(ButtonVariant::Ghost)
                    .compact()
                    .px_1()
                    .selected(selected)
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
            panel = panel
                .child(
                    tool_panel::section("Annotate")
                        .child(brushes)
                        .child(radius)
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    self.button(
                                        "segmentation-undo",
                                        "Undo stroke",
                                        AutomaticSegmentationMessage::UndoStroke,
                                        cx,
                                    )
                                    .compact()
                                    .flex_1()
                                    .min_w_0(),
                                )
                                .child(
                                    self.button(
                                        "segmentation-clear",
                                        "Clear image",
                                        AutomaticSegmentationMessage::ClearAnnotations,
                                        cx,
                                    )
                                    .compact()
                                    .flex_1()
                                    .min_w_0(),
                                ),
                        ),
                )
                .child(self.annotations(&s, cx))
                .child(self.feature_controls(&s, cx));
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
                    .child(self.input_scale_control(s.forest.input_scale, cx));
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
                                || s.training_annotation_count == 0
                                || s.features.count() == 0,
                        )
                        .on_click(cx.listener(|this, _, _, _| {
                            this.send(AutomaticSegmentationMessage::TrainModel)
                        })),
                ),
            );
        } else {
            panel = panel.child(tool_panel::section("Inference settings").child(
                self.integer_control(
                    ControlKind::MaxParallelTiles,
                    "Maximum parallel tiles",
                    u32::from(s.inference.max_parallel_tiles),
                    1,
                    256,
                    cx,
                ),
            ));
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
