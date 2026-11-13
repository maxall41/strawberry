use super::{base::ToolBase, cache::ToolCache};
use crate::segmentation::{
    AnnotationMask, TrainingSample, append_stroke_points, render_class_mask,
    render_class_prediction, train,
};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use strawberry_processing::{
    GpuImageProcessor,
    segmentation::{FeatureExtractor, SegmentationModel, write_artifact},
};
use strawberry_types::{
    overlay::{CircleStyle, OverlayElement, ToolOverlay},
    *,
};

const MODEL_ASSET: &str = "model.json";

struct Sample {
    image: Arc<RgbaImage>,
}

struct PendingStroke {
    id: StrokeId,
    kind: AnnotationKind,
    stroke: AnnotationStroke,
}

#[derive(Serialize, Deserialize)]
struct ModelDocument {
    version: u32,
    classes: SegmentationClasses,
    class_ids: Vec<ClassId>,
    model: Arc<SegmentationModel>,
}
impl ModelDocument {
    const VERSION: u32 = 1;

    fn load(bytes: &[u8]) -> Result<Self> {
        let document: Self = serde_json::from_slice(bytes)?;
        ensure!(
            document.version == Self::VERSION,
            "Unsupported segmentation model version"
        );
        document.model.validate()?;
        validate_classes(&document.classes)?;
        let ids: HashSet<_> = document.class_ids.iter().copied().collect();
        ensure!(
            document.class_ids.len() == document.model.class_count
                && ids.len() == document.class_ids.len()
                && ids.iter().all(|id| document.classes.contains_key(id)),
            "Model class mapping mismatch"
        );
        Ok(document)
    }
}
fn validate_classes(classes: &SegmentationClasses) -> Result<()> {
    ensure!(
        (2..=256).contains(&classes.len()),
        "Define between 2 and 256 classes"
    );
    ensure!(
        classes.values().all(|class| !class.name.trim().is_empty()),
        "Class names cannot be empty"
    );
    Ok(())
}
fn annotation_visible(
    state: &AutomaticSegmentationState,
    kind: AnnotationKind,
    stroke: &AnnotationStroke,
) -> bool {
    stroke.visible
        && match kind {
            AnnotationKind::Paint(id) => {
                !state.hidden_classes.contains(&id)
                    && (!state.preview_selected || state.active_class == id)
            }
            AnnotationKind::Eraser => true,
        }
}

fn annotation_color(kind: AnnotationKind, classes: &SegmentationClasses) -> BgraColor {
    match kind {
        AnnotationKind::Paint(id) => classes
            .get(&id)
            .map_or(BgraColor::TRANSPARENT, |class| class.bgra(150)),
        AnnotationKind::Eraser => BgraColor::TRANSPARENT,
    }
}

/// Retain the expensive prediction independently of the pipeline's enabled/bypass cache.
/// Keep the source and model alive so pointer identity cannot match a recycled allocation.
struct InferenceCache {
    layer: usize,
    input: Arc<RgbaImage>,
    model: Arc<SegmentationModel>,
    output: Arc<Vec<image::GrayImage>>,
    overlay: Option<PredictionOverlay>,
}

struct PredictionOverlay {
    colors: Vec<BgraColor>,
    image: Arc<RgbaImage>,
}

impl InferenceCache {
    fn matches(
        &self,
        layer: usize,
        input: &Arc<RgbaImage>,
        model: &Arc<SegmentationModel>,
    ) -> bool {
        self.layer == layer && Arc::ptr_eq(&self.input, input) && Arc::ptr_eq(&self.model, model)
    }
}

pub struct AutomaticSegmentation {
    pub state: AutomaticSegmentationState,
    pub base: ToolBase,
    /// Pipeline output/bypass cache and frontend state synchronization, used by the backend macros.
    pub cache: ToolCache<AutomaticSegmentationState>,
    commands: tokio::sync::broadcast::Sender<PipelineCommand>,
    // Annotation/settings edits do not invalidate a model trained from their earlier snapshot.
    // Deleting a class or loading a model invalidates pending training results.
    training_generation: u64,
    next_stroke_id: StrokeId,
    job_generation: u64,
    extractor: Arc<FeatureExtractor>,
    model: Option<Arc<SegmentationModel>>,
    model_class_ids: Vec<ClassId>,
    inference_cache: Option<InferenceCache>,
    samples: Vec<Sample>,
    input: Option<Arc<RgbaImage>>,
    current: Option<SampleId>,
    stroke: Option<PendingStroke>,
    annotation_masks: AnnotationMask,
    overlays_dirty: bool,
    job: Option<flume::Receiver<Result<(SegmentationModel, Vec<ClassId>)>>>,
}
impl AutomaticSegmentation {
    pub fn new(
        commands: tokio::sync::broadcast::Sender<PipelineCommand>,
        output: tokio::sync::broadcast::Sender<PipelineCommand>,
        gpu: Arc<GpuImageProcessor>,
    ) -> Self {
        let state = AutomaticSegmentationState::default();
        let base = ToolBase::new(output);
        let _ = commands.send(PipelineCommand::RegisterClickReceiver(base.sender.clone()));
        Self {
            commands,
            training_generation: 0,
            next_stroke_id: 0,
            job_generation: 0,
            cache: ToolCache::new(&state),
            state,
            base,
            extractor: Arc::new(FeatureExtractor::new(gpu.clone())),
            model: None,
            model_class_ids: Vec::new(),
            inference_cache: None,
            samples: vec![],
            input: None,
            current: None,
            stroke: None,
            annotation_masks: AnnotationMask::default(),
            overlays_dirty: false,
            job: None,
        }
    }
    fn class_mut(&mut self, id: ClassId) -> Result<&mut SegmentationClass> {
        self.state
            .classes
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Unknown class"))
    }
    fn install_classes(&mut self, classes: SegmentationClasses) {
        self.state.next_class_id = self.state.next_class_id.max(
            classes
                .keys()
                .next_back()
                .copied()
                .map(ClassId::get)
                .unwrap_or(0)
                .saturating_add(1),
        );
        let active = *classes.keys().nth(1).expect("validated class count");
        self.state.output_class = active;
        self.state.brush = AnnotationKind::Paint(active);
        self.state.active_class = active;
        self.state.classes = classes;
        self.state.hidden_classes.clear();
        self.samples.clear();
        self.state.erasers.clear();
        self.current = None;
        self.stroke = None;
        self.annotation_masks.clear();
        self.state.selected_stroke = None;
        self.inference_cache = None;
    }
    fn model_bytes(&self) -> Result<Vec<u8>> {
        let model = self
            .model
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Train or load a model first"))?;
        Ok(serde_json::to_vec(&ModelDocument {
            version: ModelDocument::VERSION,
            classes: self.state.classes.clone(),
            class_ids: self.model_class_ids.clone(),
            model: Arc::clone(model),
        })?)
    }
    fn render_prediction(&self, planes: &[image::GrayImage]) -> RgbaImage {
        if self.state.output_mode == SegmentationOutputMode::ClassMask {
            let plane = self
                .model_class_ids
                .iter()
                .position(|&id| id == self.state.output_class);
            let threshold = self
                .state
                .use_threshold
                .then_some(self.state.threshold_value);
            return render_class_mask(planes, plane, threshold);
        }
        let colors: Vec<_> = self
            .model_class_ids
            .iter()
            .map(|id| self.state.classes[id].bgra(255))
            .collect();
        render_class_prediction(planes, &colors)
    }
    fn refresh_prediction_overlay(&mut self) {
        if !self.state.is_enabled || !self.state.training || !self.state.show_overlay {
            return;
        }
        let Some(cached) = self.inference_cache.as_ref().filter(|cached| {
            self.input
                .as_ref()
                .zip(self.model.as_ref())
                .is_some_and(|(image, model)| {
                    cached.matches(self.state.selected_layer, image, model)
                })
        }) else {
            return;
        };
        let colors: Vec<_> = self
            .model_class_ids
            .iter()
            .map(|id| {
                let class = &self.state.classes[id];
                let hidden = self.state.hidden_classes.contains(id)
                    || (self.state.preview_selected && self.state.active_class != *id);
                class.bgra(if hidden { 0 } else { 255 })
            })
            .collect();
        if cached
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.colors == colors)
        {
            return;
        }
        let image = Arc::new(render_class_prediction(&cached.output, &colors));
        if let Some(cached) = &mut self.inference_cache {
            cached.overlay = Some(PredictionOverlay { colors, image });
        }
    }
    fn rebuild_mask(&mut self) {
        if let Some((index, sample)) = self
            .current
            .and_then(|index| self.samples.get(index).map(|sample| (index, sample)))
        {
            let strokes = self.state.ordered_annotations(index);
            self.annotation_masks.rebuild(
                sample.image.dimensions(),
                strokes.into_values().filter_map(|(kind, stroke)| {
                    annotation_visible(&self.state, kind, stroke)
                        .then(|| (stroke, annotation_color(kind, &self.state.classes)))
                }),
            );
        } else {
            self.annotation_masks.clear();
        }
    }
    fn changed(&mut self) {
        self.overlays_dirty = true;
        self.state.current_sample = self.current;
        self.refresh_prediction_overlay();
    }
    fn edited(&mut self) {
        // Training inputs can change while the last trained model remains usable.
        self.stroke = None;
        self.state.selected_stroke = None;
        self.rebuild_mask();
        self.changed();
    }
    fn invalidate_model(&mut self) {
        self.training_generation = self.training_generation.wrapping_add(1);
        self.model = None;
        self.model_class_ids.clear();
        self.inference_cache = None;
        self.state.model_ready = false;
        self.state.model_path = None;
        self.state.oob_error = None;
        self.state.model_revision = self.state.model_revision.wrapping_add(1);
    }
    fn ensure_sample(&mut self) -> Option<usize> {
        if let Some(index) = self.current {
            return Some(index);
        }
        let image = self.input.as_ref()?;
        self.samples.push(Sample {
            image: image.clone(),
        });
        let index = self.samples.len().checked_sub(1)?;
        self.current = Some(index);
        Some(index)
    }
    fn error(&mut self, error: &anyhow::Error) {
        let _ = self
            .base
            .global_sender
            .send(PipelineCommand::FrontendMessage(FrontendMessage::error(
                format!("Segmentation: {error}"),
            )));
    }
    fn start_training(&mut self) -> Result<()> {
        ensure!(self.job.is_none(), "Training is already running");
        self.state.features.validate()?;
        self.state.forest.validate()?;
        validate_classes(&self.state.classes)?;
        let classes: Vec<_> = self.state.classes.keys().copied().collect();
        let samples: Vec<_> = self
            .samples
            .iter()
            .enumerate()
            .map(|(index, sample)| TrainingSample {
                image: sample.image.clone(),
                annotations: self
                    .state
                    .ordered_annotations(index)
                    .into_values()
                    .map(|(kind, stroke)| (kind, stroke.clone()))
                    .collect(),
            })
            .collect();
        let features = self.state.features.clone();
        let forest = self.state.forest.clone();
        let inference = self.state.inference.clone();
        let extractor = self.extractor.clone();
        self.job_generation = self.training_generation;
        let (tx, rx) = flume::bounded(1);
        self.job = Some(rx);
        self.state.busy = true;
        tokio::task::spawn_blocking(move || {
            let result = futures::executor::block_on(async move {
                let model =
                    train(&samples, &classes, features, forest, &inference, &extractor).await?;
                Ok((model, classes))
            });
            let _ = tx.send(result);
        });
        Ok(())
    }
    fn handle(&mut self, message: AutomaticSegmentationMessage) -> Result<()> {
        match message {
            AutomaticSegmentationMessage::AddClass => {
                ensure!(self.state.classes.len() < 256, "Maximum of 256 classes");
                let id = self.state.next_class_id;
                self.state.next_class_id = id
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Class ID exhausted"))?;
                let id = ClassId::new(id);
                let number = self.state.classes.len() + 1;
                self.state.classes.insert(
                    id,
                    SegmentationClass {
                        name: format!("Class {number}"),
                        color: CLASS_PALETTE[id.get() as usize % CLASS_PALETTE.len()],
                        annotations: HashMap::new(),
                    },
                );
                self.state.brush = AnnotationKind::Paint(id);
                self.state.active_class = id;
                self.edited();
            }
            AutomaticSegmentationMessage::RenameClass(id, name) => {
                let name = name.trim().to_string();
                ensure!(!name.is_empty(), "Class names cannot be empty");
                self.class_mut(id)?.name = name;
            }
            AutomaticSegmentationMessage::SetClassColor(id, color) => {
                self.class_mut(id)?.color = color;
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::DeleteClass(id) => {
                ensure!(self.state.classes.len() > 2, "Keep at least two classes");
                self.class_mut(id)?;
                self.state.classes.remove(&id);
                self.state.hidden_classes.retain(|&c| c != id);
                let fallback = *self
                    .state
                    .classes
                    .keys()
                    .next()
                    .expect("validated class count");
                if self.state.brush == AnnotationKind::Paint(id) {
                    self.state.brush = AnnotationKind::Paint(fallback);
                }
                if self.state.active_class == id {
                    self.state.active_class = fallback;
                }
                if self.state.output_class == id {
                    self.state.output_class = fallback;
                }
                self.invalidate_model();
                self.edited();
            }
            AutomaticSegmentationMessage::ToggleClassVisibility(id) => {
                self.class_mut(id)?;
                if self.state.hidden_classes.contains(&id) {
                    self.state.hidden_classes.retain(|&c| c != id);
                } else {
                    self.state.hidden_classes.push(id);
                }
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::SetPreviewSelected(value) => {
                self.state.preview_selected = value;
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::SetOutputMode(value) => self.state.output_mode = value,
            AutomaticSegmentationMessage::SetOutputClass(id) => {
                self.class_mut(id)?;
                self.state.output_class = id;
            }
            AutomaticSegmentationMessage::SetTraining(value) => {
                self.state.training = value;
                self.stroke = None;
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::SetShowOverlay(value) => self.state.show_overlay = value,
            AutomaticSegmentationMessage::SetOverlayOpacity(value) => {
                self.state.overlay_opacity = value.min(100);
            }
            AutomaticSegmentationMessage::ToggleThreshold(value) => {
                self.state.use_threshold = value
            }
            AutomaticSegmentationMessage::SetThreshold(value) => {
                ensure!(
                    value.is_finite() && (0.0..=1.0).contains(&value),
                    "Threshold must be between 0 and 1"
                );
                self.state.threshold_value = value;
            }
            AutomaticSegmentationMessage::SetBrush(kind) => {
                if let AnnotationKind::Paint(id) = kind {
                    self.class_mut(id)?;
                    self.state.active_class = id;
                }
                self.state.brush = kind;
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::SetBrushRadius(value) => self.state.brush_radius = value,
            AutomaticSegmentationMessage::SelectLayer(value) => {
                if self.state.selected_layer == value {
                    return Ok(());
                }
                self.state.selected_stroke = None;
                self.state.selected_layer = value;
                self.input = None;
                self.current = None;
                self.stroke = None;
                self.annotation_masks.clear();
            }
            AutomaticSegmentationMessage::SetFeatures(value) => {
                value.validate()?;
                self.state.features = value;
                self.edited();
            }
            AutomaticSegmentationMessage::SetForest(value) => {
                value.validate()?;
                self.state.forest = value;
                self.edited();
            }
            AutomaticSegmentationMessage::SetInference(value) => {
                value.validate()?;
                self.state.inference = value;
            }
            AutomaticSegmentationMessage::SelectStroke(index) => {
                ensure!(
                    self.current
                        .and_then(|sample| self.state.stroke(sample, index))
                        .is_some(),
                    "Unknown stroke"
                );
                self.state.selected_stroke = Some(index);
            }
            AutomaticSegmentationMessage::ToggleStrokeVisibility(index) => {
                if let Some(stroke) = self
                    .current
                    .and_then(|sample| self.state.stroke_mut(sample, index))
                {
                    stroke.visible = !stroke.visible;
                }
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::DeleteStroke(index) => {
                if let Some(sample) = self.current {
                    ensure!(
                        self.state.stroke(sample, index).is_some(),
                        "Unknown annotation"
                    );
                    self.state.remove_stroke(sample, index);
                }
                self.edited();
            }
            AutomaticSegmentationMessage::UndoStroke => {
                if let Some(sample) = self.current {
                    if let Some((&id, _)) = self.state.ordered_annotations(sample).last_key_value()
                    {
                        self.state.remove_stroke(sample, id);
                    }
                }
                self.edited();
            }
            AutomaticSegmentationMessage::ClearAnnotations => {
                if let Some(sample) = self.current {
                    self.state.clear_sample_annotations(sample);
                }
                self.edited();
            }
            AutomaticSegmentationMessage::TrainModel => self.start_training()?,
            AutomaticSegmentationMessage::SaveModel(path) => {
                write_artifact(&path, &self.model_bytes()?)?;
                self.state.model_path = Some(path);
            }
            AutomaticSegmentationMessage::LoadModel(path) => {
                let document = ModelDocument::load(&std::fs::read(&path)?)?;
                self.install_classes(document.classes);
                self.model_class_ids = document.class_ids;
                let model = document.model;
                self.training_generation = self.training_generation.wrapping_add(1);
                self.state.features = model.features.clone();
                self.state.forest = model.parameters.clone();
                self.state.oob_error = model.oob_error;
                self.model = Some(model);
                self.state.model_ready = true;
                self.state.model_path = Some(path);
                self.state.model_revision = self.state.model_revision.wrapping_add(1);
            }
        }
        self.changed();
        Ok(())
    }
    fn add_point(&mut self, x: u64, y: u64) {
        let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
            return;
        };
        let Some(index) = self.current else {
            return;
        };
        let Some(sample) = self.samples.get(index) else {
            return;
        };
        if x >= sample.image.width() || y >= sample.image.height() {
            return;
        }
        let (width, height) = sample.image.dimensions();
        self.overlays_dirty = true;
        if let Some(stroke) = &mut self.stroke {
            let appended =
                append_stroke_points(&mut stroke.stroke.points, (x, y), stroke.stroke.radius);
            if !annotation_visible(&self.state, stroke.kind, &stroke.stroke) {
                return;
            }
            self.annotation_masks.paint(
                (width, height),
                &stroke.stroke.points[appended],
                stroke.stroke.radius,
                annotation_color(stroke.kind, &self.state.classes),
            );
        }
    }
    fn finish_training(&mut self) {
        if let Some(result) = self.job.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.job = None;
            self.state.busy = false;
            match result {
                _ if self.job_generation != self.training_generation || !self.state.is_enabled => {}
                Ok((model, class_ids)) => {
                    self.state.oob_error = model.oob_error;
                    self.model = Some(Arc::new(model));
                    self.model_class_ids = class_ids;
                    self.state.model_ready = true;
                    self.state.model_path = None;
                    self.state.model_revision = self.state.model_revision.wrapping_add(1);
                    self.changed();
                }
                Err(error) => self.error(&error),
            }
        }
    }

    fn receive_commands(&mut self) -> Result<()> {
        for message in self.base.collect_commands() {
            match message {
                PipelineCommand::RequestState() => {
                    self.base
                        .send_to_frontend(&PipelineCommand::FrontendMessage(
                            FrontendMessage::ReturnState(self.get_state()),
                        ))?
                }
                PipelineCommand::PipelineObject(PipelineObjectMessage::ToggleActive(value)) => {
                    self.state.is_enabled = value;
                    self.stroke = None;
                    self.rebuild_mask();
                    self.changed();
                }
                PipelineCommand::PipelineObject(
                    PipelineObjectMessage::AutomaticSegmentationMessage(message),
                ) => {
                    if let Err(error) = self.handle(message) {
                        self.error(&error);
                    }
                }
                PipelineCommand::InteractionStart(x, y)
                    if self.state.is_enabled && self.state.training =>
                {
                    if self.ensure_sample().is_some() {
                        self.next_stroke_id += 1;
                        self.stroke = Some(PendingStroke {
                            id: self.next_stroke_id,
                            kind: self.state.brush,
                            stroke: AnnotationStroke {
                                visible: true,
                                radius: self.state.brush_radius,
                                points: Arc::new(Vec::new()),
                            },
                        });
                        self.add_point(x, y);
                    }
                }
                PipelineCommand::InteractionMove(x, y) if self.stroke.is_some() => {
                    self.add_point(x, y)
                }
                PipelineCommand::InteractionEnd => {
                    if let Some(stroke) = self.stroke.take() {
                        if !stroke.stroke.points.is_empty() {
                            if let Some(sample) = self.current {
                                match stroke.kind {
                                    AnnotationKind::Paint(id) => {
                                        if let Some(class) = self.state.classes.get_mut(&id) {
                                            class
                                                .annotations
                                                .entry(sample)
                                                .or_default()
                                                .insert(stroke.id, stroke.stroke);
                                        }
                                    }
                                    AnnotationKind::Eraser => {
                                        self.state
                                            .erasers
                                            .entry(sample)
                                            .or_default()
                                            .insert(stroke.id, stroke.stroke);
                                    }
                                }
                            }
                            self.edited();
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}
#[async_trait]
impl ToolBackend for AutomaticSegmentation {
    crate::impl_tool_backend_basics!(AutomaticSegmentation);
    fn pipeline_assets(&self) -> Result<ToolAssets> {
        let mut assets = ToolAssets::new();
        if self.model.is_some() {
            assets.insert(MODEL_ASSET.into(), self.model_bytes()?);
        }
        Ok(assets)
    }
    fn prepare_loaded_fields<'a>(
        &'a mut self,
        fields: serde_value::Value,
        assets: &ToolAssets,
        cli: bool,
    ) -> Result<Option<PreparedStateUpdate<'a>>> {
        let mut state = self.state.clone();
        AutomaticSegmentationState::deserialize_in_place(fields, &mut state)?;
        state.forest.validate()?;
        state.inference.validate()?;
        state.overlay_opacity = state.overlay_opacity.min(100);
        ensure!(
            state.threshold_value.is_finite() && (0.0..=1.0).contains(&state.threshold_value),
            "Threshold must be between 0 and 1"
        );
        let model = assets
            .get(MODEL_ASSET)
            .map(|bytes| ModelDocument::load(bytes))
            .transpose()?;
        if cli && state.is_enabled {
            ensure!(
                model.is_some(),
                "CLI segmentation requires an embedded model"
            );
            state.training = false;
        }
        validate_classes(&state.classes)?;
        if let AnnotationKind::Paint(id) = state.brush {
            ensure!(state.classes.contains_key(&id), "Unknown brush class");
        }
        ensure!(
            state
                .hidden_classes
                .iter()
                .all(|id| state.classes.contains_key(id)),
            "Unknown hidden class"
        );
        ensure!(
            state.classes.contains_key(&state.active_class),
            "Unknown active class"
        );
        ensure!(
            state.classes.contains_key(&state.output_class),
            "Unknown output class"
        );
        if let Some(document) = &model {
            ensure!(
                document
                    .class_ids
                    .iter()
                    .all(|id| state.classes.contains_key(id)),
                "Pipeline and model class mappings differ"
            );
        }
        state.next_class_id = state.next_class_id.max(
            state
                .classes
                .keys()
                .next_back()
                .copied()
                .map(ClassId::get)
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Class ID exhausted"))?,
        );
        state.erasers.clear();
        state.current_sample = None;
        state.selected_stroke = None;
        state.model_path = None;
        state.model_ready = model.is_some();
        state.oob_error = model.as_ref().and_then(|document| document.model.oob_error);
        state.busy = false;
        state.model_revision = self.state.model_revision.wrapping_add(1);

        Ok(Some(Box::new(move || {
            self.state = state;
            self.model_class_ids = model
                .as_ref()
                .map_or_else(Vec::new, |document| document.class_ids.clone());
            self.model = model.map(|document| document.model);
            self.samples.clear();
            self.current = None;
            self.input = None;
            self.annotation_masks.clear();
            self.inference_cache = None;
            self.job = None;
            self.stroke = None;
            self.training_generation = self.training_generation.wrapping_add(1);
            self.cache.invalidate();
            self.changed();
        })))
    }
    fn iam(&self) -> BackendToolType {
        BackendToolType::AutomaticSegmentation
    }
    fn get_state(&self) -> ToolState {
        ToolState::AutomaticSegmentationState(self.state.clone())
    }
    async fn process(&mut self, images: &mut Vec<Arc<RgbaImage>>) -> Result<()> {
        self.state.total_layers = images.len();
        if self.state.selected_layer >= images.len() {
            self.state.selected_layer = 0;
        }
        let Some(image) = images.get(self.state.selected_layer).cloned() else {
            return Ok(());
        };
        if self
            .input
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, &image))
        {
            self.annotation_masks.clear();
            self.input = Some(image.clone());
            self.current = self
                .samples
                .iter()
                .position(|sample| Arc::ptr_eq(&sample.image, &image));
            self.stroke = None;
            self.state.selected_stroke = None;
            self.rebuild_mask();
            self.changed();
        }
        if self.state.training && !self.state.show_overlay {
            return Ok(());
        }
        let Some(model) = self.model.clone() else {
            return Ok(());
        };
        let output = if let Some(cached) = self
            .inference_cache
            .as_ref()
            .filter(|cached| cached.matches(self.state.selected_layer, &image, &model))
        {
            cached.output.clone()
        } else {
            let extractor = self.extractor.clone();
            let worker_image = image.clone();
            let worker_model = model.clone();
            let inference = self.state.inference.clone();
            let output = Arc::new(
                tokio::task::spawn_blocking(move || {
                    futures::executor::block_on(worker_model.predict(
                        &extractor,
                        &worker_image,
                        &inference,
                    ))
                })
                .await??,
            );
            self.inference_cache = Some(InferenceCache {
                layer: self.state.selected_layer,
                input: image,
                model,
                output: output.clone(),
                overlay: None,
            });
            output
        };
        if self.state.training {
            self.refresh_prediction_overlay();
            self.overlays_dirty = true;
        } else if let Some(layer) = images.get_mut(self.state.selected_layer) {
            *layer = Arc::new(self.render_prediction(&output));
        }

        Ok(())
    }
    fn receive_messages(&mut self) -> Result<()> {
        // The tool manager can disable a tool directly, bypassing ToggleActive.
        if (!self.state.is_enabled || !self.state.training) && self.stroke.take().is_some() {
            self.rebuild_mask();
            self.changed();
        }
        self.finish_training();
        self.receive_commands()?;
        if std::mem::take(&mut self.overlays_dirty) {
            let _ = self.commands.send(PipelineCommand::RequestOverlays);
        }
        Ok(())
    }
    fn get_overlays(&self) -> Option<ToolOverlay> {
        if !self.state.is_enabled || !self.state.training {
            return None;
        }
        let mut elements = Vec::new();
        if self.state.show_overlay {
            if let Some(cached) = self.inference_cache.as_ref().filter(|cached| {
                self.input
                    .as_ref()
                    .zip(self.model.as_ref())
                    .is_some_and(|(image, model)| {
                        cached.matches(self.state.selected_layer, image, model)
                    })
            }) {
                if let Some(overlay) = &cached.overlay {
                    elements.push(OverlayElement::ImageMask {
                        x: 0,
                        y: 0,
                        opacity: f32::from(self.state.overlay_opacity) / 100.,
                        image: Some(overlay.image.clone()),
                    });
                }
            }
        }
        elements.extend(self.annotation_masks.iter().map(|(&(x, y), image)| {
            OverlayElement::ImageMask {
                x,
                y,
                opacity: 1.,
                image: Some(image.clone()),
            }
        }));
        if let Some(sample) = self.current {
            if let Some(index) = self.state.selected_stroke {
                if let Some(stroke) = self.state.stroke(sample, index) {
                    if let Some(&(x, y)) = stroke.points.first() {
                        elements.push(OverlayElement::Circle {
                            x: num_traits::cast::<u32, f32>(x).unwrap_or(0.0) + 0.5,
                            y: num_traits::cast::<u32, f32>(y).unwrap_or(0.0) + 0.5,
                            radius: num_traits::cast(stroke.radius.get()).unwrap_or(1.0),
                            color: RgbaColor::WHITE,
                            style: CircleStyle::Selected { width: 1.0 },
                        });
                    }
                }
            }
        }
        Some(ToolOverlay {
            tool_id: self.iam(),
            z_order: 50,
            elements,
        })
    }
}
