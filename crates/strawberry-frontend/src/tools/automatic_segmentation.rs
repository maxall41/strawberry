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
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use strawberry_types::{AutomaticSegmentationMessage as M, *};

pub struct AutomaticSegmentationView {
    pub state: Arc<RwLock<AutomaticSegmentationState>>,
    pub base: super::base::ToolBase,
    controls: BTreeMap<String, Entity<ValueControl>>,
    show_forest: bool,
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
            controls: BTreeMap::new(),
            show_forest: false,
        }
    }
    fn send(&self, message: M) {
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
        message: M,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .label(label)
            .on_click(cx.listener(move |this, _, _, _| this.send(message.clone())))
    }
    fn section(title: &'static str) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_3()
            .w_full()
            .min_w_0()
            .p_3()
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::BORDER_MUTED)
            .rounded_lg()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(title),
            )
    }
    fn file_button(
        &self,
        id: &'static str,
        label: &'static str,
        save: bool,
        dataset: bool,
        cx: &Context<Self>,
    ) -> Button {
        Button::new(id)
            .compact()
            .flex_1()
            .min_w_0()
            .label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.file(save, dataset, window, cx)))
    }
    fn annotations(&self, s: &AutomaticSegmentationState, cx: &Context<Self>) -> gpui::Div {
        let count: usize = s.samples.iter().map(|sample| sample.strokes.len()).sum();
        let mut list = div()
            .id("segmentation-annotations")
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .max_h(gpui::px(240.0))
            .overflow_y_scroll()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .border_1()
            .border_color(theme::BORDER)
            .rounded_md()
            .bg(theme::SURFACE_DEEP);
        if count == 0 {
            list = list.child(
                div()
                    .flex_none()
                    .p_3()
                    .text_xs()
                    .text_color(theme::TEXT_MUTED)
                    .child("No annotations"),
            );
        }
        for (i, sample) in s.samples.iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .bg(theme::SURFACE_ALT)
                    .border_b_1()
                    .border_color(theme::BORDER_MUTED)
                    .child(
                        self.button(
                            ("segmentation-image", i),
                            format!("Image {}", i.saturating_add(1)),
                            M::Preview(Some(i)),
                            cx,
                        )
                        .variant(ButtonVariant::Ghost)
                        .compact()
                        .selected(s.preview == Some(i))
                        .flex_1()
                        .min_w_0()
                        .justify_start(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{} × {}", sample.width, sample.height)),
                    ),
            );
            for (j, stroke) in sample.strokes.iter().enumerate() {
                let name = if stroke.erase {
                    "Eraser"
                } else if stroke.foreground {
                    "Foreground"
                } else {
                    "Background"
                };
                let color = if stroke.erase {
                    theme::TEXT_MUTED
                } else if stroke.foreground {
                    theme::SUCCESS
                } else {
                    theme::DANGER
                };
                let selected = s.selected_stroke == Some((i, j)) && s.preview == Some(i);
                let row = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .min_w_0()
                    .border_b_1()
                    .border_color(theme::BORDER_MUTED)
                    .bg(if selected {
                        theme::SURFACE_ALT
                    } else {
                        theme::SURFACE_DEEP
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(gpui::px(6.0))
                            .rounded_full()
                            .bg(color),
                    )
                    .child(
                        Button::new(gpui::ElementId::Name(
                            format!("segmentation-stroke-{i}-{j}").into(),
                        ))
                        .variant(ButtonVariant::Ghost)
                        .compact()
                        .flex_1()
                        .min_w_0()
                        .justify_start()
                        .aria_label(format!("Select {name} annotation {}", j.saturating_add(1)))
                        .child(
                            div().flex().flex_col().items_start().min_w_0().child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .child(format!("{name} {}", j.saturating_add(1))),
                            ),
                        )
                        .on_click(
                            cx.listener(move |this, _, _, _| this.send(M::SelectStroke(i, j))),
                        ),
                    )
                    .child(
                        Button::new(gpui::ElementId::Name(
                            format!("segmentation-visible-{i}-{j}").into(),
                        ))
                        .variant(ButtonVariant::Ghost)
                        .icon(
                            if stroke.visible {
                                Icon::eye_open()
                            } else {
                                Icon::eye_closed()
                            },
                            if stroke.visible {
                                "Hide annotation"
                            } else {
                                "Show annotation"
                            },
                        )
                        .tooltip_text(if stroke.visible {
                            "Hide annotation"
                        } else {
                            "Show annotation"
                        })
                        .on_click(cx.listener(move |this, _, _, _| this.send(M::Visibility(i, j)))),
                    )
                    .child(
                        Button::new(gpui::ElementId::Name(
                            format!("segmentation-delete-{i}-{j}").into(),
                        ))
                        .variant(ButtonVariant::Delete)
                        .aria_label("Delete annotation")
                        .tooltip_text("Delete annotation")
                        .on_click(
                            cx.listener(move |this, _, _, _| this.send(M::DeleteStroke(i, j))),
                        ),
                    );
                list = list.child(row);
            }
        }
        let mut section = Self::section("Training annotations")
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::TEXT_MUTED)
                            .child(format!("{count} annotations")),
                    )
                    .when(s.preview.is_some(), |el| {
                        el.child(
                            self.button(
                                "segmentation-current",
                                "Current image",
                                M::Preview(None),
                                cx,
                            )
                            .compact()
                            .variant(ButtonVariant::Ghost),
                        )
                    }),
            )
            .child(list);
        section = section.child(
            div()
                .flex()
                .gap_2()
                .min_w_0()
                .child(self.file_button("segmentation-load-data", "Load dataset", false, true, cx))
                .child(
                    self.file_button("segmentation-save-data", "Save dataset", true, true, cx)
                        .disabled(count == 0),
                ),
        );
        section
    }
    fn feature_controls(&self, s: &AutomaticSegmentationState, cx: &Context<Self>) -> gpui::Div {
        let mut section = Self::section("Features");
        for (label, enabled) in [
            ("Gaussian blur", !s.features.gaussian.is_empty()),
            ("Sobel", s.features.sobel),
            ("Difference of Gaussians", !s.features.dog.is_empty()),
        ] {
            let mut features = s.features.clone();
            match label {
                "Gaussian blur" => features.gaussian = if enabled { vec![] } else { vec![1.0] },
                "Sobel" => features.sobel = !enabled,
                _ => features.dog = if enabled { vec![] } else { vec![(1.0, 2.0)] },
            }
            section = section.child(
                ToggleButton::new(label.into(), enabled, move |this: &mut Self, _, _, _| {
                    this.send(M::Features(features.clone()))
                })
                .with_no_margin()
                .render(cx),
            );
        }
        let mut channels = div()
            .flex()
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(theme::SURFACE_DEEP);
        for (rgb, label) in [(false, "Luminance"), (true, "RGB")] {
            let mut features = s.features.clone();
            features.rgb = rgb;
            channels = channels.child(
                self.button(
                    gpui::ElementId::Name(format!("segmentation-channel-{label}").into()),
                    label,
                    M::Features(features),
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
    fn file(&self, save: bool, dataset: bool, window: &Window, cx: &Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let dialog = rfd::FileDialog::new().add_filter(
                if dataset {
                    "Segmentation dataset"
                } else {
                    "Segmentation model"
                },
                &[if dataset {
                    "strawberry-training"
                } else {
                    "strawberry-model"
                }],
            );
            let path = if save {
                dialog.save_file()
            } else {
                dialog.pick_file()
            };
            if let Some(path) = path {
                let _ = this.update(cx, |this, _| {
                    this.send(match (save, dataset) {
                        (true, true) => M::SaveDataset(path),
                        (false, true) => M::LoadDataset(path),
                        (true, false) => M::SaveModel(path),
                        (false, false) => M::LoadModel(path),
                    })
                });
            }
        })
        .detach();
    }
    fn number(
        &mut self,
        id: &str,
        label: &str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        cx: &mut Context<Self>,
    ) -> Entity<ValueControl> {
        if let Some(control) = self.controls.get(id) {
            return control.clone();
        }
        let key = id.to_string();
        let control = cx.new(|cx| {
            ValueControl::new(
                value.to_string(),
                value,
                ValueControlConfig {
                    id: key.clone(),
                    label: label.into(),
                    min,
                    max,
                    step,
                },
                cx,
            )
        });
        cx.subscribe(&control, |this, _, event: &ValueControlEvent, _| {
            let state = this
                .state
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let integer = num_traits::ToPrimitive::to_u32(&event.value).unwrap_or(1);
            let mut forest = state.forest;
            match event.id.as_str() {
                "brush" => this.send(M::BrushRadius(integer)),
                "trees" => {
                    forest.trees = u16::try_from(integer).unwrap_or(100);
                    this.send(M::Forest(forest));
                }
                "depth" => {
                    forest.depth = u16::try_from(integer).unwrap_or(20);
                    this.send(M::Forest(forest));
                }
                _ => {}
            }
        })
        .detach();
        self.controls.insert(key, control.clone());
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
        for (id, control) in &self.controls {
            let value = match id.as_str() {
                "brush" => num_traits::cast(state.brush_radius),
                "trees" => Some(f32::from(state.forest.trees)),
                "depth" => Some(f32::from(state.forest.depth)),
                _ => None,
            };
            if let Some(value) = value {
                control.update(cx, |control, cx| {
                    control.value = value;
                    cx.notify();
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
        let mut modes = div()
            .flex()
            .gap_1()
            .p_1()
            .rounded_md()
            .bg(theme::SURFACE_DEEP);
        for (training, label) in [(true, "Training"), (false, "Inference")] {
            modes = modes.child(
                self.button(
                    gpui::ElementId::Name(format!("segmentation-mode-{label}").into()),
                    label,
                    M::Training(training),
                    cx,
                )
                .variant(ButtonVariant::Ghost)
                .selected(s.training == training)
                .flex_1()
                .min_w_0(),
            );
        }
        let mut panel = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap_4()
            .p_4()
            .text_sm()
            .text_color(theme::TEXT)
            .bg(theme::WINDOW_BG)
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
                Self::section("Input layer").child(div().flex().flex_wrap().gap_1().children(
                    (0..s.total_layers).map(|i| {
                        self.button(
                            ("segmentation-layer", i),
                            format!("{}", i.saturating_add(1)),
                            M::Layer(i),
                            cx,
                        )
                        .compact()
                        .selected(i == s.selected_layer)
                    }),
                )),
            );
        }
        if s.training {
            let mut brushes = div()
                .flex()
                .gap_1()
                .p_1()
                .rounded_md()
                .bg(theme::SURFACE_DEEP);
            for (label, selected, message) in [
                ("Foreground", !s.eraser && s.foreground, M::Foreground(true)),
                (
                    "Background",
                    !s.eraser && !s.foreground,
                    M::Foreground(false),
                ),
                ("Eraser", s.eraser, M::Eraser(true)),
            ] {
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
            let radius = self.number(
                "brush",
                "Brush radius (px)",
                num_traits::cast(s.brush_radius).unwrap_or(5.0),
                1.0,
                128.0,
                1.0,
                cx,
            );
            panel = panel
                .child(
                    Self::section("Annotate")
                        .child(brushes)
                        .child(radius)
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    self.button("segmentation-undo", "Undo stroke", M::Undo, cx)
                                        .compact()
                                        .flex_1()
                                        .min_w_0(),
                                )
                                .child(
                                    self.button("segmentation-clear", "Clear image", M::Clear, cx)
                                        .compact()
                                        .flex_1()
                                        .min_w_0(),
                                ),
                        ),
                )
                .child(self.annotations(&s, cx))
                .child(self.feature_controls(&s, cx));
            let mut classifier = Self::section("Classifier").child(
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
                    .child(self.number(
                        "trees",
                        "Trees",
                        f32::from(s.forest.trees),
                        1.0,
                        4096.0,
                        1.0,
                        cx,
                    ))
                    .child(self.number(
                        "depth",
                        "Maximum depth",
                        f32::from(s.forest.depth),
                        1.0,
                        256.0,
                        1.0,
                        cx,
                    ));
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
                                || s.samples.is_empty()
                                || s.features.count() == 0,
                        )
                        .on_click(cx.listener(|this, _, _, _| this.send(M::Train))),
                ),
            );
        }
        let mut model =
            Self::section("Model").child(
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
                div()
                    .flex()
                    .gap_2()
                    .min_w_0()
                    .child(self.file_button(
                        "segmentation-load-model",
                        "Load model",
                        false,
                        false,
                        cx,
                    ))
                    .child(
                        self.file_button("segmentation-save-model", "Save model", true, false, cx)
                            .disabled(!s.model_ready),
                    ),
            ),
        );
        panel
    }
}
