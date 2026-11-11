use super::{base::ToolBase, cache::ToolCache};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use image::RgbaImage;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};
use strawberry_processing::{
    GpuImageProcessor,
    segmentation::{FeatureExtractor, SegmentationModel},
};
use strawberry_types::{
    overlay::{CircleStyle, OverlayElement, ToolOverlay},
    *,
};

const MODEL_ASSET: &str = "model.json";
const MASK_TILE_SIZE: u32 = 256;
const FOREGROUND_BGRA: [u8; 4] = [100, 220, 40, 150];
const BACKGROUND_BGRA: [u8; 4] = [80, 70, 230, 150];
const TRANSPARENT_BGRA: [u8; 4] = [0; 4];
type MaskTiles = BTreeMap<(u32, u32), Arc<RgbaImage>>;

#[derive(Clone)]
struct Sample {
    image: Arc<RgbaImage>,
    strokes: Vec<AnnotationStroke>,
}

fn annotation_color(kind: AnnotationKind) -> [u8; 4] {
    match kind {
        AnnotationKind::Foreground => FOREGROUND_BGRA,
        AnnotationKind::Background => BACKGROUND_BGRA,
        AnnotationKind::Eraser => TRANSPARENT_BGRA,
    }
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
fn rasterize(sample: &Sample) -> BTreeMap<(u32, u32), u8> {
    let mut labels = BTreeMap::new();
    for stroke in &sample.strokes {
        for &(x, y) in &stroke.points {
            brush_spans(
                sample.image.width(),
                sample.image.height(),
                x,
                y,
                stroke.radius,
                |row, left, right| {
                    for column in left..right {
                        match stroke.kind {
                            AnnotationKind::Eraser => {
                                labels.remove(&(row, column));
                            }
                            kind => {
                                labels.insert(
                                    (row, column),
                                    u8::from(kind == AnnotationKind::Foreground),
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

fn paint_mask(
    tiles: &mut MaskTiles,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    radius: BrushRadius,
    color: [u8; 4],
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
                    tile.put_pixel(px - origin.0, row - origin.1, image::Rgba(color));
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
    output: Arc<RgbaImage>,
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
    pub cache: ToolCache<AutomaticSegmentationState>,
    commands: tokio::sync::broadcast::Sender<PipelineCommand>,
    training_revision: u64,
    job_revision: u64,
    extractor: Arc<FeatureExtractor>,
    model: Option<Arc<SegmentationModel>>,
    inference_cache: Option<InferenceCache>,
    samples: Vec<Sample>,
    input: Option<Arc<RgbaImage>>,
    current: Option<usize>,
    stroke: Option<AnnotationStroke>,
    annotation_masks: MaskTiles,
    overlays_dirty: bool,
    job: Option<flume::Receiver<Result<SegmentationModel>>>,
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
            training_revision: 0,
            job_revision: 0,
            cache: ToolCache::new(&state),
            state,
            base,
            extractor: Arc::new(FeatureExtractor::new(gpu)),
            model: None,
            inference_cache: None,
            samples: vec![],
            input: None,
            current: None,
            stroke: None,
            annotation_masks: BTreeMap::new(),
            overlays_dirty: false,
            job: None,
        }
    }
    fn rebuild_mask(&mut self) {
        let mut tiles = MaskTiles::new();
        if let Some(sample) = self.current.and_then(|i| self.samples.get(i)) {
            for stroke in sample.strokes.iter().filter(|s| s.visible) {
                let color = annotation_color(stroke.kind);
                for &(x, y) in &stroke.points {
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
        self.state.training_annotation_count =
            self.samples.iter().map(|sample| sample.strokes.len()).sum();
        self.state.annotations = self
            .current
            .and_then(|i| self.samples.get(i))
            .map(|sample| {
                sample
                    .strokes
                    .iter()
                    .map(|stroke| AnnotationStrokeSummary {
                        kind: stroke.kind,
                        visible: stroke.visible,
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    fn edited(&mut self) {
        self.training_revision = self.training_revision.wrapping_add(1);
        self.state.selected_stroke = None;
        self.changed();
    }
    fn ensure_sample(&mut self) -> Option<usize> {
        if let Some(index) = self.current {
            return Some(index);
        }
        let image = self.input.as_ref()?;
        self.samples.push(Sample {
            image: image.clone(),
            strokes: vec![],
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
        let samples = self.samples.clone();
        let features = self.state.features.clone();
        let forest = self.state.forest.clone();
        let extractor = self.extractor.clone();
        self.job_revision = self.training_revision;
        let (tx, rx) = flume::bounded(1);
        self.job = Some(rx);
        self.state.busy = true;
        tokio::task::spawn_blocking(move || {
            let result = futures::executor::block_on(async move {
                let mut candidates = [Vec::new(), Vec::new()];
                for (image_index, sample) in samples.iter().enumerate() {
                    let labels = rasterize(sample);
                    for ((y, x), label) in labels {
                        if let Some(class) = candidates.get_mut(usize::from(label)) {
                            class.push((image_index, x, y));
                        }
                    }
                }
                let count = candidates
                    .iter()
                    .map(Vec::len)
                    .min()
                    .unwrap_or(0)
                    .min(50_000);
                ensure!(
                    count > 0,
                    "Annotate both foreground and background before training"
                );
                let mut selected = BTreeMap::new();
                for (label, class) in candidates.iter().enumerate() {
                    for i in 0..count {
                        let index = i * class.len() / count;
                        if let Some(&key) = class.get(index) {
                            selected.insert(key, u8::try_from(label)?);
                        }
                    }
                }
                let mut rows = Vec::new();
                let mut labels = Vec::new();
                for (index, sample) in samples.iter().enumerate() {
                    let image = &sample.image;
                    for tile in extractor.tiles(&image, &features)? {
                        let x = tile.core_x;
                        let y = tile.core_y;
                        if !selected.keys().any(|&(i, px, py)| {
                            i == index
                                && px >= x
                                && px < x + tile.core_width
                                && py >= y
                                && py < y + tile.core_height
                        }) {
                            continue;
                        }
                        let data = extractor.extract(&image, &features, &tile).await?;
                        for (offset, row) in data.rows.into_iter().enumerate() {
                            let offset = u32::try_from(offset)?;
                            let key = (
                                index,
                                data.x + offset % data.width,
                                data.y + offset / data.width,
                            );
                            if let Some(&label) = selected.get(&key) {
                                rows.push(row);
                                labels.push(label);
                            }
                        }
                    }
                }
                SegmentationModel::train(features, forest, rows, labels)
            });
            let _ = tx.send(result);
        });
        Ok(())
    }
    fn handle(&mut self, message: AutomaticSegmentationMessage) -> Result<()> {
        match message {
            AutomaticSegmentationMessage::SetTraining(value) => {
                self.state.training = value;
                self.stroke = None;
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::SetBrush(kind) => self.state.brush = kind,
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
                if value.count() > 0 {
                    value.validate()?;
                }
                self.state.features = value;
                self.edited();
            }
            AutomaticSegmentationMessage::SetForest(value) => {
                self.state.forest = value;
                self.edited();
            }
            AutomaticSegmentationMessage::SelectStroke(index) => {
                ensure!(
                    self.current
                        .and_then(|i| self.samples.get(i))
                        .is_some_and(|sample| index < sample.strokes.len()),
                    "Unknown stroke"
                );
                self.state.selected_stroke = Some(index);
            }
            AutomaticSegmentationMessage::ToggleStrokeVisibility(index) => {
                if let Some(stroke) = self
                    .current
                    .and_then(|i| self.samples.get_mut(i))
                    .and_then(|sample| sample.strokes.get_mut(index))
                {
                    stroke.visible = !stroke.visible;
                }
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::DeleteStroke(index) => {
                if let Some(sample) = self.current.and_then(|i| self.samples.get_mut(i)) {
                    ensure!(index < sample.strokes.len(), "Unknown annotation");
                    sample.strokes.remove(index);
                }
                self.edited();
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::UndoStroke => {
                if let Some(sample) = self.current.and_then(|i| self.samples.get_mut(i)) {
                    sample.strokes.pop();
                }
                self.edited();
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::ClearAnnotations => {
                if let Some(sample) = self.current.and_then(|i| self.samples.get_mut(i)) {
                    sample.strokes.clear();
                }
                self.edited();
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::TrainModel => self.start_training()?,
            AutomaticSegmentationMessage::SaveModel(path) => {
                self.model
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Train or load a model first"))?
                    .save(&path)?;
                self.state.model_path = Some(path);
            }
            AutomaticSegmentationMessage::LoadModel(path) => {
                let model = SegmentationModel::load(&path)?;
                self.training_revision = self.training_revision.wrapping_add(1);
                self.state.features = model.features.clone();
                self.state.forest = model.parameters.clone();
                self.model = Some(Arc::new(model));
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
            let first_new = stroke.points.len();
            if let Some(&(a, b)) = stroke.points.last() {
                let dx = i64::from(x) - i64::from(a);
                let dy = i64::from(y) - i64::from(b);
                let distance = dx.unsigned_abs().max(dy.unsigned_abs());
                if distance == 0 {
                    return;
                }
                // Circular dabs overlap with half-radius spacing, independent of cursor speed.
                let spacing = u64::from(stroke.radius.get().div_ceil(2));
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
                        stroke.points.push((px, py));
                    }
                }
            } else {
                stroke.points.push((x, y));
            }
            let color = annotation_color(stroke.kind);
            for &(x, y) in stroke.points.iter().skip(first_new) {
                paint_mask(
                    &mut self.annotation_masks,
                    width,
                    height,
                    x,
                    y,
                    stroke.radius,
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
                Ok(_) if self.job_revision != self.training_revision || !self.state.is_enabled => {}
                Ok(model) => {
                    self.model = Some(Arc::new(model));
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
                        self.stroke = Some(AnnotationStroke {
                            visible: true,
                            kind: self.state.brush,
                            radius: self.state.brush_radius,
                            points: vec![],
                        });
                        self.add_point(x, y);
                    }
                }
                PipelineCommand::InteractionMove(x, y) if self.stroke.is_some() => {
                    self.add_point(x, y)
                }
                PipelineCommand::InteractionEnd => {
                    if let Some(stroke) = self.stroke.take() {
                        if !stroke.points.is_empty() {
                            if let Some(sample) = self.current.and_then(|i| self.samples.get_mut(i))
                            {
                                sample.strokes.push(stroke);
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
        if let Some(model) = &self.model {
            assets.insert(MODEL_ASSET.into(), model.to_bytes()?);
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
        let model = assets
            .get(MODEL_ASSET)
            .map(|bytes| SegmentationModel::from_bytes(bytes))
            .transpose()?;
        if cli && state.is_enabled {
            ensure!(
                model.is_some(),
                "CLI segmentation requires an embedded model"
            );
            state.training = false;
        }
        state.model_path = None;
        state.model_ready = model.is_some();
        state.busy = false;
        state.model_revision = self.state.model_revision.wrapping_add(1);

        Ok(Some(Box::new(move || {
            self.state = state;
            self.model = model.map(Arc::new);
            self.inference_cache = None;
            self.job = None;
            self.stroke = None;
            self.training_revision = self.training_revision.wrapping_add(1);
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
        if self.state.training {
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
            let output = Arc::new(
                tokio::task::spawn_blocking(move || {
                    futures::executor::block_on(extractor.infer(&worker_image, &worker_model))
                })
                .await??,
            );
            self.inference_cache = Some(InferenceCache {
                layer: self.state.selected_layer,
                input: image,
                model,
                output: output.clone(),
            });
            output
        };
        if let Some(layer) = images.get_mut(self.state.selected_layer) {
            *layer = output;
        }
        Ok(())
    }
    fn receive_messages(&mut self) -> Result<()> {
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
        let sample = self.current.and_then(|i| self.samples.get(i))?;
        let mut elements: Vec<_> = self
            .annotation_masks
            .iter()
            .map(|(&(x, y), image)| OverlayElement::ImageMask {
                x,
                y,
                image: Some(image.clone()),
            })
            .collect();
        if let Some(index) = self.state.selected_stroke {
            if let Some(stroke) = sample.strokes.get(index) {
                if let Some(&(x, y)) = stroke.points.first() {
                    elements.push(OverlayElement::Circle {
                        x: num_traits::cast::<u32, f32>(x).unwrap_or(0.0) + 0.5,
                        y: num_traits::cast::<u32, f32>(y).unwrap_or(0.0) + 0.5,
                        radius: num_traits::cast(stroke.radius.get()).unwrap_or(1.0),
                        color: [255, 255, 255, 255],
                        style: CircleStyle::Selected { width: 1.0 },
                    });
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
