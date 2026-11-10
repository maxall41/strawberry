use super::{base::ToolBase, cache::ToolCache};
use anyhow::{Result, ensure};
use async_trait::async_trait;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use strawberry_processing::{
    GpuImageProcessor,
    segmentation::{FeatureExtractor, SegmentationModel, write_artifact},
};
use strawberry_types::{
    overlay::{CircleStyle, OverlayElement, ToolOverlay},
    *,
};

#[derive(Clone, Serialize, Deserialize)]
struct Sample {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    strokes: Vec<AnnotationStroke>,
}
impl Sample {
    fn image(&self) -> Result<RgbaImage> {
        RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or_else(|| anyhow::anyhow!("Invalid training image dimensions"))
    }
}
/// Replay paint in order. Visibility affects display only, never training labels.
fn rasterize<'a>(
    width: u32,
    height: u32,
    strokes: impl Iterator<Item = &'a AnnotationStroke>,
) -> BTreeMap<(u32, u32), u8> {
    let mut labels = BTreeMap::new();
    for stroke in strokes {
        for &(x, y) in &stroke.points {
            let radius = i64::from(stroke.radius.min(128));
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dy * dy > radius * radius {
                        continue;
                    }
                    let px = i64::from(x) + dx;
                    let py = i64::from(y) + dy;
                    if px < 0 || py < 0 || px >= i64::from(width) || py >= i64::from(height) {
                        continue;
                    }
                    if let (Ok(px), Ok(py)) = (u32::try_from(px), u32::try_from(py)) {
                        if stroke.erase {
                            labels.remove(&(py, px));
                        } else {
                            labels.insert((py, px), u8::from(stroke.foreground));
                        }
                    }
                }
            }
        }
    }
    labels
}

const MASK_TILE_SIZE: u32 = 256;
type MaskTiles = BTreeMap<(u32, u32), Arc<RgbaImage>>;

fn paint_mask(
    tiles: &mut MaskTiles,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    radius: u32,
    color: [u8; 4],
) {
    let radius = i64::from(radius.min(128));
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let (Ok(x), Ok(y)) = (
                u32::try_from(i64::from(x) + dx),
                u32::try_from(i64::from(y) + dy),
            ) else {
                continue;
            };
            if x >= width || y >= height {
                continue;
            }
            let origin = (
                x / MASK_TILE_SIZE * MASK_TILE_SIZE,
                y / MASK_TILE_SIZE * MASK_TILE_SIZE,
            );
            if color[3] == 0 && !tiles.contains_key(&origin) {
                continue;
            }
            let tile = tiles.entry(origin).or_insert_with(|| {
                Arc::new(RgbaImage::new(
                    (width - origin.0).min(MASK_TILE_SIZE),
                    (height - origin.1).min(MASK_TILE_SIZE),
                ))
            });
            Arc::make_mut(tile).put_pixel(x - origin.0, y - origin.1, image::Rgba(color));
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Dataset {
    version: u32,
    samples: Vec<Sample>,
    features: strawberry_processing::segmentation::FeatureConfig,
    forest: strawberry_processing::segmentation::ForestConfig,
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
    loaded_path: Option<std::path::PathBuf>,
    samples: Vec<Sample>,
    input: Option<Arc<RgbaImage>>,
    current: Option<usize>,
    stroke: Option<AnnotationStroke>,
    annotation_masks: MaskTiles,
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
            loaded_path: None,
            samples: vec![],
            input: None,
            current: None,
            stroke: None,
            annotation_masks: BTreeMap::new(),
            job: None,
        }
    }
    fn rebuild_mask(&mut self) {
        let mut tiles = MaskTiles::new();
        if let Some(sample) = self.active().and_then(|i| self.samples.get(i)) {
            for stroke in sample.strokes.iter().filter(|s| s.visible) {
                let color = if stroke.erase {
                    [0; 4]
                } else if stroke.foreground {
                    [100, 220, 40, 150]
                } else {
                    [80, 70, 230, 150]
                };
                for &(x, y) in &stroke.points {
                    paint_mask(
                        &mut tiles,
                        sample.width,
                        sample.height,
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
        let _ = self.commands.send(PipelineCommand::RequestOverlays);
        self.state.revision = self.state.revision.wrapping_add(1);
        self.state.samples = self
            .samples
            .iter()
            .map(|s| AnnotationSummary {
                width: s.width,
                height: s.height,
                strokes: s
                    .strokes
                    .iter()
                    .map(|stroke| AnnotationStrokeSummary {
                        foreground: stroke.foreground,
                        erase: stroke.erase,
                        visible: stroke.visible,
                        point_count: stroke.points.len(),
                    })
                    .collect(),
            })
            .collect();
    }
    fn edited(&mut self) {
        self.training_revision = self.training_revision.wrapping_add(1);
        self.changed();
    }
    fn active(&self) -> Option<usize> {
        self.state.preview.or(self.current)
    }
    fn ensure_sample(&mut self) -> Option<usize> {
        if let Some(index) = self.active() {
            return Some(index);
        }
        let image = self.input.as_ref()?;
        self.samples.push(Sample {
            width: image.width(),
            height: image.height(),
            pixels: image.as_raw().clone(),
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
                    let labels = rasterize(sample.width, sample.height, sample.strokes.iter());
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
                    let image = sample.image()?;
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
        use AutomaticSegmentationMessage as M;
        let refresh_mask = matches!(
            &message,
            M::Preview(_)
                | M::SelectStroke(..)
                | M::Visibility(..)
                | M::DeleteStroke(..)
                | M::Undo
                | M::Clear
                | M::LoadDataset(_)
                | M::Layer(_)
                | M::Training(_)
        );
        match message {
            M::Training(value) => {
                self.state.training = value;
                self.state.preview = None;
                self.stroke = None;
            }
            M::Foreground(value) => {
                self.state.foreground = value;
                self.state.eraser = false;
            }
            M::Eraser(value) => self.state.eraser = value,
            M::BrushRadius(value) => self.state.brush_radius = value.clamp(1, 128),
            M::Layer(value) => {
                self.state.preview = None;
                self.state.selected_stroke = None;
                self.state.selected_layer = value;
                self.input = None;
                self.current = None;
                self.stroke = None;
            }
            M::Features(value) => {
                if value.count() > 0 {
                    value.validate()?;
                }
                self.state.features = value;
                self.edited();
            }
            M::Forest(value) => {
                self.state.forest = value;
                self.edited();
            }
            M::Preview(index) => {
                ensure!(
                    index.is_none_or(|i| i < self.samples.len()),
                    "Unknown training image"
                );
                self.state.preview = index;
                self.state.selected_stroke = None;
                self.stroke = None;
            }
            M::SelectStroke(image, stroke) => {
                ensure!(
                    self.samples
                        .get(image)
                        .is_some_and(|s| stroke < s.strokes.len()),
                    "Unknown stroke"
                );
                self.state.preview = Some(image);
                self.state.selected_stroke = Some((image, stroke));
            }
            M::Visibility(image, stroke) => {
                if let Some(stroke) = self
                    .samples
                    .get_mut(image)
                    .and_then(|s| s.strokes.get_mut(stroke))
                {
                    stroke.visible = !stroke.visible;
                }
            }
            M::DeleteStroke(image, stroke) => {
                if let Some(sample) = self.samples.get_mut(image) {
                    ensure!(stroke < sample.strokes.len(), "Unknown annotation");
                    sample.strokes.remove(stroke);
                }
                self.edited();
            }
            M::Undo => {
                if let Some(sample) = self.active().and_then(|i| self.samples.get_mut(i)) {
                    sample.strokes.pop();
                }
                self.edited();
            }
            M::Clear => {
                if let Some(sample) = self.active().and_then(|i| self.samples.get_mut(i)) {
                    sample.strokes.clear();
                }
                self.edited();
            }
            M::Train => self.start_training()?,
            M::SaveModel(path) => {
                self.model
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Train or load a model first"))?
                    .save(&path)?;
                self.state.model_path = Some(path.clone());
                self.loaded_path = Some(path);
            }
            M::LoadModel(path) => {
                let model = SegmentationModel::load(&path)?;
                self.training_revision = self.training_revision.wrapping_add(1);
                self.state.features = model.features.clone();
                self.model = Some(Arc::new(model));
                self.state.model_ready = true;
                self.state.model_path = Some(path.clone());
                self.loaded_path = Some(path);
            }
            M::SaveDataset(path) => write_artifact(
                &path,
                &serde_json::to_vec(&Dataset {
                    version: 1,
                    samples: self.samples.clone(),
                    features: self.state.features.clone(),
                    forest: self.state.forest.clone(),
                })?,
            )?,
            M::LoadDataset(path) => {
                let data: Dataset = serde_json::from_slice(&std::fs::read(path)?)?;
                ensure!(data.version == 1, "Unsupported dataset version");
                if data.features.count() > 0 {
                    data.features.validate()?;
                }
                for sample in &data.samples {
                    sample.image()?;
                    ensure!(
                        sample.strokes.iter().all(|s| s.radius > 0
                            && s.radius <= 128
                            && s.points
                                .iter()
                                .all(|&(x, y)| x < sample.width && y < sample.height)),
                        "Invalid annotation"
                    );
                }
                self.samples = data.samples;
                self.state.features = data.features;
                self.state.forest = data.forest;
                self.current = None;
                self.state.preview = None;
                self.edited();
            }
        }
        if refresh_mask {
            self.rebuild_mask();
        }
        self.changed();
        Ok(())
    }
    fn add_point(&mut self, x: u64, y: u64) {
        let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
            return;
        };
        let Some(index) = self.active() else {
            return;
        };
        let Some(sample) = self.samples.get(index) else {
            return;
        };
        if x >= sample.width || y >= sample.height {
            return;
        }
        let (width, height) = (sample.width, sample.height);
        let _ = self.commands.send(PipelineCommand::RequestOverlays);
        if let Some(stroke) = &mut self.stroke {
            let first_new = stroke.points.len();
            if let Some(&(a, b)) = stroke.points.last() {
                let dx = i64::from(x) - i64::from(a);
                let dy = i64::from(y) - i64::from(b);
                let steps = dx.unsigned_abs().max(dy.unsigned_abs()).max(1);
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
            let color = if stroke.erase {
                [0; 4]
            } else if stroke.foreground {
                [100, 220, 40, 150]
            } else {
                [80, 70, 230, 150]
            };
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
}
#[async_trait]
impl ToolBackend for AutomaticSegmentation {
    crate::impl_tool_backend_basics!(AutomaticSegmentation);
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
            self.current = None;
            self.stroke = None;
            if self.state.preview.is_some() {
                self.rebuild_mask();
            }
        }
        if self.state.training {
            if let Some(sample) = self.state.preview.and_then(|i| self.samples.get(i)) {
                if let Some(layer) = images.get_mut(self.state.selected_layer) {
                    *layer = Arc::new(sample.image()?);
                }
            }
            return Ok(());
        }
        if self.state.model_path != self.loaded_path {
            if let Some(path) = &self.state.model_path {
                let model = SegmentationModel::load(path)?;
                self.model = Some(Arc::new(model));
                self.loaded_path = Some(path.clone());
                self.state.model_ready = true;
            }
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
        if let Some(result) = self.job.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.job = None;
            self.state.busy = false;
            match result {
                Ok(_) if self.job_revision != self.training_revision || !self.state.is_enabled => {}
                Ok(model) => {
                    self.model = Some(Arc::new(model));
                    self.state.model_ready = true;
                    self.state.model_path = None;
                    self.loaded_path = None;
                    self.changed();
                }
                Err(error) => self.error(&error),
            }
        }
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
                            foreground: self.state.foreground,
                            erase: self.state.eraser,
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
                            if let Some(sample) =
                                self.active().and_then(|i| self.samples.get_mut(i))
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
    fn get_overlays(&self) -> Option<ToolOverlay> {
        if !self.state.is_enabled || !self.state.training {
            return None;
        }
        let sample = self.active().and_then(|i| self.samples.get(i))?;
        let mut elements: Vec<_> = self
            .annotation_masks
            .iter()
            .map(|(&(x, y), image)| OverlayElement::ImageMask {
                x,
                y,
                image: Some(image.clone()),
            })
            .collect();
        if let Some((image, stroke)) = self.state.selected_stroke {
            if Some(image) == self.active() {
                if let Some(stroke) = sample.strokes.get(stroke) {
                    if let Some(&(x, y)) = stroke.points.first() {
                        elements.push(OverlayElement::Circle {
                            x: num_traits::cast::<u32, f32>(x).unwrap_or(0.0) + 0.5,
                            y: num_traits::cast::<u32, f32>(y).unwrap_or(0.0) + 0.5,
                            radius: num_traits::cast(stroke.radius).unwrap_or(1.0),
                            color: [255, 255, 255, 255],
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
