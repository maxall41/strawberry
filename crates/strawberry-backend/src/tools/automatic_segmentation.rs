use super::{base::ToolBase, cache::ToolCache};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
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

const MASK_TILE_SIZE: u32 = 256;
const TRANSPARENT_BGRA: BgraColor = BgraColor::TRANSPARENT;
// The viewer caches masks by overlay index, so rebuilds must emit tiles in a stable order.
type MaskTiles = BTreeMap<(u32, u32), Arc<RgbaImage>>;

#[derive(Clone)]
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
fn annotation_color(kind: AnnotationKind, classes: &SegmentationClasses) -> BgraColor {
    match kind {
        AnnotationKind::Paint(id) => classes
            .get(&id)
            .map_or(TRANSPARENT_BGRA, |class| class.bgra(150)),
        AnnotationKind::Eraser => TRANSPARENT_BGRA,
    }
}

fn render_class_prediction(planes: &[image::GrayImage], colors: &[BgraColor]) -> RgbaImage {
    let (width, height) = planes[0].dimensions();
    RgbaImage::from_fn(width, height, |x, y| {
        let mut winner = 0;
        for index in 1..planes.len() {
            if planes[index].get_pixel(x, y)[0] > planes[winner].get_pixel(x, y)[0] {
                winner = index;
            }
        }
        colors[winner].into_pixel()
    })
}

/// Visit horizontal spans of a circular dab, clipping once per row.
fn brush_spans(
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    radius: BrushRadius,
    mut visit: impl FnMut(u32, u32, u32),
) {
    let radius = i64::from(radius.get());
    let top = (i64::from(y) - radius).max(0);
    let bottom = (i64::from(y) + radius).min(i64::from(height) - 1);
    for row in top..=bottom {
        let dy = row - i64::from(y);
        let extent = (radius * radius - dy * dy).unsigned_abs().isqrt();
        let extent = i64::try_from(extent).unwrap_or(0);
        let left = (i64::from(x) - extent).max(0);
        let right = (i64::from(x) + extent + 1).min(i64::from(width));
        if let (Ok(row), Ok(left), Ok(right)) = (
            u32::try_from(row),
            u32::try_from(left),
            u32::try_from(right),
        ) {
            visit(row, left, right);
        }
    }
}

/// Replay circular dabs in stroke order so erasers remove earlier labels.
fn rasterize(
    image: &RgbaImage,
    width: u32,
    height: u32,
    sample: SampleId,
    annotations: &AutomaticSegmentationState,
) -> BTreeMap<(u32, u32), u8> {
    let mut labels = BTreeMap::new();
    let classes: Vec<_> = annotations.classes.keys().copied().collect();
    // Map original brush pixels to the feature image while retaining stroke/eraser order.
    let scaled_x = |x| {
        u32::try_from(u64::from(x) * u64::from(width) / u64::from(image.width()))
            .unwrap_or_default()
    };
    let scaled_y = |y| {
        u32::try_from(u64::from(y) * u64::from(height) / u64::from(image.height()))
            .unwrap_or_default()
    };
    for (_, (kind, stroke)) in annotations.ordered_annotations(sample) {
        for &(x, y) in stroke.points.iter() {
            brush_spans(
                image.width(),
                image.height(),
                x,
                y,
                stroke.radius,
                |row, left, right| {
                    if left >= right {
                        return;
                    }
                    let row = scaled_y(row);
                    for column in scaled_x(left)..=scaled_x(right - 1) {
                        match kind {
                            AnnotationKind::Eraser => {
                                labels.remove(&(row, column));
                            }
                            AnnotationKind::Paint(id) => {
                                labels.insert(
                                    (row, column),
                                    classes
                                        .iter()
                                        .position(|&class| class == id)
                                        .expect("validated stroke class")
                                        as u8,
                                );
                            }
                        }
                    }
                },
            );
        }
    }
    labels
}

fn select_training_pixels(
    candidates: &[Vec<(usize, u32, u32)>],
    balance_classes: bool,
) -> Result<HashMap<(usize, u32, u32), u8>> {
    let minority_count = candidates.iter().map(Vec::len).min().unwrap_or(0);
    ensure!(minority_count > 0, "Annotate every class before training");
    let mut selected = HashMap::new();
    for (label, class) in candidates.iter().enumerate() {
        let count = if balance_classes {
            minority_count
        } else {
            class.len()
        };
        for i in 0..count {
            let index = if count == class.len() {
                i
            } else {
                i * class.len() / count
            };
            if let Some(&key) = class.get(index) {
                selected.insert(key, u8::try_from(label)?);
            }
        }
    }
    Ok(selected)
}

fn paint_mask(
    tiles: &mut MaskTiles,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    radius: BrushRadius,
    color: BgraColor,
) {
    brush_spans(width, height, x, y, radius, |row, left, right| {
        let mut column = left;
        while column < right {
            let origin = (
                column / MASK_TILE_SIZE * MASK_TILE_SIZE,
                row / MASK_TILE_SIZE * MASK_TILE_SIZE,
            );
            let end = right.min(origin.0 + MASK_TILE_SIZE);
            if color != TRANSPARENT_BGRA || tiles.contains_key(&origin) {
                let tile = tiles.entry(origin).or_insert_with(|| {
                    Arc::new(RgbaImage::new(
                        (width - origin.0).min(MASK_TILE_SIZE),
                        (height - origin.1).min(MASK_TILE_SIZE),
                    ))
                });
                // Take copy-on-write ownership once per row segment, rather than once per pixel.
                let tile = Arc::make_mut(tile);
                for px in column..end {
                    tile.put_pixel(px - origin.0, row - origin.1, color.into_pixel());
                }
            }
            column = end;
        }
    });
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
    annotation_masks: MaskTiles,
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
            annotation_masks: MaskTiles::new(),
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
            let (width, height) = planes[0].dimensions();
            let plane = self
                .model_class_ids
                .iter()
                .position(|&id| id == self.state.output_class)
                .and_then(|index| planes.get(index));
            return RgbaImage::from_fn(width, height, |x, y| {
                let probability = plane.map_or(0, |plane| plane.get_pixel(x, y)[0]);
                let value = if self.state.use_threshold {
                    if plane.is_some()
                        && f32::from(probability) / 255. >= self.state.threshold_value
                    {
                        255
                    } else {
                        0
                    }
                } else {
                    probability
                };
                RgbColor::new(value, value, value)
                    .with_alpha(255)
                    .to_bgra()
                    .into_pixel()
            });
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
        let mut tiles = MaskTiles::new();
        if let Some((index, sample)) = self
            .current
            .and_then(|index| self.samples.get(index).map(|sample| (index, sample)))
        {
            for (_, (kind, stroke)) in self.state.ordered_annotations(index) {
                if !stroke.visible {
                    continue;
                }
                if let AnnotationKind::Paint(id) = kind {
                    if self.state.hidden_classes.contains(&id)
                        || (self.state.preview_selected && self.state.active_class != id)
                    {
                        continue;
                    }
                }
                let color = annotation_color(kind, &self.state.classes);
                for &(x, y) in stroke.points.iter() {
                    paint_mask(
                        &mut tiles,
                        sample.image.width(),
                        sample.image.height(),
                        x,
                        y,
                        stroke.radius,
                        color,
                    );
                }
            }
        }
        for (key, tile) in &mut tiles {
            if let Some(old) = self.annotation_masks.get(key) {
                if old == tile {
                    *tile = old.clone();
                }
            }
        }
        self.annotation_masks = tiles;
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
        let samples = self.samples.clone();
        let annotations = self.state.clone();
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
                let images = samples
                    .iter()
                    .map(|sample| forest.rescale_input(&sample.image))
                    .collect::<Result<Vec<_>>>()?;
                let mut candidates = vec![Vec::new(); classes.len()];
                for (image_index, (sample, image)) in samples.iter().zip(&images).enumerate() {
                    let labels = rasterize(
                        &sample.image,
                        image.width(),
                        image.height(),
                        image_index,
                        &annotations,
                    );
                    for ((y, x), label) in labels {
                        if let Some(class) = candidates.get_mut(usize::from(label)) {
                            class.push((image_index, x, y));
                        }
                    }
                }
                let selected = select_training_pixels(&candidates, forest.balance_classes)?;
                let model = SegmentationModel::train(
                    features, forest, &extractor, &images, &selected, &inference,
                )
                .await?;
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
            let first_new = stroke.stroke.points.len();
            if let Some(&(a, b)) = stroke.stroke.points.last() {
                let dx = i64::from(x) - i64::from(a);
                let dy = i64::from(y) - i64::from(b);
                let distance = dx.unsigned_abs().max(dy.unsigned_abs());
                if distance == 0 {
                    return;
                }
                // Circular dabs overlap with half-radius spacing, independent of cursor speed.
                let spacing = u64::from(stroke.stroke.radius.get().div_ceil(2));
                let steps = distance.div_ceil(spacing);
                for i in 1..=steps {
                    if let (Ok(px), Ok(py)) = (
                        u32::try_from(
                            i64::from(a)
                                + dx * i64::try_from(i).unwrap_or(0)
                                    / i64::try_from(steps).unwrap_or(1),
                        ),
                        u32::try_from(
                            i64::from(b)
                                + dy * i64::try_from(i).unwrap_or(0)
                                    / i64::try_from(steps).unwrap_or(1),
                        ),
                    ) {
                        Arc::make_mut(&mut stroke.stroke.points).push((px, py));
                    }
                }
            } else {
                Arc::make_mut(&mut stroke.stroke.points).push((x, y));
            }
            if let AnnotationKind::Paint(id) = stroke.kind {
                if self.state.hidden_classes.contains(&id)
                    || (self.state.preview_selected && self.state.active_class != id)
                {
                    return;
                }
            }
            let color = annotation_color(stroke.kind, &self.state.classes);
            for &(x, y) in stroke.stroke.points.iter().skip(first_new) {
                paint_mask(
                    &mut self.annotation_masks,
                    width,
                    height,
                    x,
                    y,
                    stroke.stroke.radius,
                    color,
                );
            }
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
