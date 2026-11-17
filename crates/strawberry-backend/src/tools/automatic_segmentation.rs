use super::{base::ToolBase, cache::ToolCache};
use crate::segmentation::{
    AnnotationMask, TrainingSample, append_stroke_points,
    dataset::{Dataset, DatasetSample},
    render_class_mask, render_class_prediction, train,
};
use crate::tools::base::ToolEvents;
use crate::utils::file_io::write_artifact;
use anyhow::{Result, ensure};
use async_trait::async_trait;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    path::PathBuf,
    sync::Arc,
};
use strawberry_processing::{
    GpuImageProcessor,
    segmentation::{FeatureExtractor, SegmentationModel},
};
use strawberry_types::{
    AnnotationKind, AnnotationStroke, AutomaticSegmentationMessage, AutomaticSegmentationState,
    BackendToolType, BgraColor, CLASS_PALETTE, ClassId, DatasetImportMode, DatasetOperation,
    FrontendMessage, MAX_CLASSES, PipelineObjectMessage, RgbaColor, SampleId, SegmentationClass,
    SegmentationClasses, SegmentationOutputMode, StrokeId, ToolAssets, ToolBackend, ToolInput,
    ToolState,
    overlay::{CircleStyle, OverlayElement, ToolOverlay},
};

const MODEL_ASSET: &str = "model.sbm";

#[derive(Clone)]
struct Sample {
    image: Arc<RgbaImage>,
}

struct DatasetJob {
    receiver: flume::Receiver<Result<DatasetResult>>,
    revision: u64,
    path: PathBuf,
}

struct TrainingJob {
    receiver: flume::Receiver<Result<(SegmentationModel, Vec<ClassId>)>>,
    generation: u64,
}

enum DatasetResult {
    Saved,
    Imported(Box<PreparedDataset>),
}

struct PreparedDataset {
    state: AutomaticSegmentationState,
    samples: Vec<Sample>,
    next_stroke_id: StrokeId,
    model_class_ids: Option<Vec<ClassId>>,
    mode: DatasetImportMode,
}

impl PreparedDataset {
    fn new(
        mut state: AutomaticSegmentationState,
        mut samples: Vec<Sample>,
        mut next_stroke_id: StrokeId,
        model_class_ids: &[ClassId],
        dataset: Dataset,
        mode: DatasetImportMode,
    ) -> Result<Self> {
        dataset.classes.validate()?;
        let mut class_mapping = HashMap::new();
        let model_class_ids = match mode {
            DatasetImportMode::Replace => {
                let model_mapping =
                    remap_model_classes(&state.classes, &dataset.classes, model_class_ids);
                let mut classes = BTreeMap::new();
                state.erasers.clear();
                samples.clear();
                next_stroke_id = 0;
                for (source, mut class) in dataset.classes {
                    let destination = ClassId::new(u64::try_from(classes.len())?);
                    class_mapping.insert(source, destination);
                    class.annotations.clear();
                    classes.insert(destination, class);
                }
                state.classes = classes.try_into()?;
                state.next_class_id = u64::try_from(state.classes.len())?;
                model_mapping.and_then(|ids| {
                    ids.into_iter()
                        .map(|id| class_mapping.get(&id).copied())
                        .collect()
                })
            }
            DatasetImportMode::Merge => {
                let mut names: HashMap<_, _> = state
                    .classes
                    .iter()
                    .map(|(&id, class)| (class.name.clone(), id))
                    .collect();
                for (id, mut class) in dataset.classes {
                    let name = class.name.clone();
                    let destination = if let Some(&destination) = names.get(&name) {
                        destination
                    } else {
                        ensure!(
                            state.classes.len() < MAX_CLASSES,
                            "Merged dataset would exceed {MAX_CLASSES} classes"
                        );
                        let destination = ClassId::new(state.next_class_id);
                        state.next_class_id = state
                            .next_class_id
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("Class ID exhausted"))?;
                        class.annotations.clear();
                        state.classes.insert(destination, class)?;
                        names.insert(name, destination);
                        destination
                    };
                    class_mapping.insert(id, destination);
                }
                Some(model_class_ids.to_vec())
            }
        };
        samples.try_reserve(dataset.samples.len())?;
        for sample in dataset.samples {
            let index = samples.len();
            for (kind, stroke) in sample.annotations {
                next_stroke_id = next_stroke_id
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Stroke ID exhausted"))?;
                let annotations = match kind {
                    AnnotationKind::Paint(id) => {
                        let destination = class_mapping
                            .get(&id)
                            .ok_or_else(|| anyhow::anyhow!("Unknown imported class"))?;
                        state
                            .classes
                            .annotations_mut(*destination)
                            .ok_or_else(|| anyhow::anyhow!("Unknown destination class"))?
                    }
                    AnnotationKind::Eraser => &mut state.erasers,
                };
                annotations
                    .entry(index)
                    .or_default()
                    .insert(next_stroke_id, stroke);
            }
            samples.push(Sample {
                image: sample.image,
            });
        }
        Ok(Self {
            state,
            samples,
            next_stroke_id,
            model_class_ids,
            mode,
        })
    }
}

fn dataset_snapshot(state: &AutomaticSegmentationState, samples: &[Sample]) -> Dataset {
    let classes = state.classes.without_annotations();
    let samples = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| {
            let annotations = state
                .ordered_annotations(index)
                .into_values()
                .map(|(kind, stroke)| (kind, stroke.clone()))
                .collect::<Vec<_>>();
            (!annotations.is_empty()).then(|| DatasetSample {
                image: sample.image.clone(),
                annotations,
            })
        })
        .collect();
    Dataset { classes, samples }
}

fn remap_model_classes(
    current: &SegmentationClasses,
    incoming: &SegmentationClasses,
    ids: &[ClassId],
) -> Option<Vec<ClassId>> {
    ids.iter()
        .map(|id| {
            let name = &current.get(id)?.name;
            incoming
                .iter()
                .find(|(_, class)| class.name == *name)
                .map(|(&id, _)| id)
        })
        .collect()
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
        document.classes.validate()?;
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
    dataset_revision: u64,
    dataset_job: Option<DatasetJob>,
    // Annotation/settings edits do not invalidate a model trained from their earlier snapshot.
    // Deleting a class or loading a model invalidates pending training results.
    training_generation: u64,
    training_job: Option<TrainingJob>,
    next_stroke_id: StrokeId,
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
}
impl AutomaticSegmentation {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(output: ToolEvents, gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        let state = AutomaticSegmentationState::default();
        let base = ToolBase::new(output);
        Ok(Self {
            dataset_revision: 0,
            dataset_job: None,
            training_generation: 0,
            next_stroke_id: 0,
            cache: ToolCache::new(&state),
            state,
            base,
            extractor: Arc::new(FeatureExtractor::new(gpu).await?),
            model: None,
            model_class_ids: Vec::new(),
            inference_cache: None,
            samples: vec![],
            input: None,
            current: None,
            stroke: None,
            annotation_masks: AnnotationMask::default(),
            overlays_dirty: false,
            training_job: None,
        })
    }
    fn importing_dataset(&self) -> bool {
        self.state
            .dataset_operation
            .is_some_and(DatasetOperation::is_importing)
    }

    fn reset_dataset_status(&mut self) {
        self.dataset_revision = self.dataset_revision.wrapping_add(1);
        self.state.dataset_samples = 0;
        self.state.dataset_path = None;
        self.state.dataset_dirty = false;
        self.dataset_job = None;
        self.state.dataset_operation = None;
    }

    fn dataset_edited(&mut self) {
        self.dataset_revision = self.dataset_revision.wrapping_add(1);
        self.state.dataset_dirty = true;
        self.state.dataset_samples = (0..self.samples.len())
            .filter(|&sample| self.state.sample_annotations(sample).next().is_some())
            .count();
    }

    fn start_dataset_save(&mut self, path: PathBuf) -> Result<()> {
        ensure!(
            self.dataset_job.is_none(),
            "A dataset operation is already running"
        );
        ensure!(
            self.stroke.is_none(),
            "Finish the current stroke before saving"
        );
        let dataset = dataset_snapshot(&self.state, &self.samples);
        ensure!(
            !dataset.samples.is_empty(),
            "Annotate an image before saving a dataset"
        );
        let (tx, receiver) = flume::bounded(1);
        self.dataset_job = Some(DatasetJob {
            receiver,
            revision: self.dataset_revision,
            path: path.clone(),
        });
        self.state.dataset_operation = Some(DatasetOperation::Saving);
        tokio::task::spawn_blocking(move || {
            let result = dataset.save(&path).map(|()| DatasetResult::Saved);
            let _ = tx.send(result);
        });
        Ok(())
    }

    fn start_dataset_import(&mut self, path: PathBuf, mode: DatasetImportMode) -> Result<()> {
        ensure!(
            self.dataset_job.is_none(),
            "A dataset operation is already running"
        );
        ensure!(
            self.training_job.is_none(),
            "Wait for training to finish before importing a dataset"
        );
        ensure!(
            self.stroke.is_none(),
            "Finish the current stroke before importing"
        );
        let state = self.state.clone();
        let samples = if mode == DatasetImportMode::Merge {
            self.samples.clone()
        } else {
            Vec::new()
        };
        let existing = (mode == DatasetImportMode::Merge)
            .then(|| dataset_snapshot(&self.state, &self.samples));
        let next_stroke_id = self.next_stroke_id;
        let model_class_ids = self.model_class_ids.clone();
        let (tx, receiver) = flume::bounded(1);
        self.dataset_job = Some(DatasetJob {
            receiver,
            revision: self.dataset_revision,
            path: path.clone(),
        });
        self.state.dataset_operation = Some(match mode {
            DatasetImportMode::Replace => DatasetOperation::Loading,
            DatasetImportMode::Merge => DatasetOperation::Merging,
        });
        tokio::task::spawn_blocking(move || {
            let result = Dataset::load(&path, existing.as_ref())
                .and_then(|dataset| {
                    PreparedDataset::new(
                        state,
                        samples,
                        next_stroke_id,
                        &model_class_ids,
                        dataset,
                        mode,
                    )
                })
                .map(|prepared| DatasetResult::Imported(Box::new(prepared)));
            let _ = tx.send(result);
        });
        Ok(())
    }

    fn finish_dataset_job(&mut self) {
        let Some(job) = self.dataset_job.as_ref() else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(flume::TryRecvError::Empty) => return,
            Err(flume::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("Dataset worker stopped unexpectedly"))
            }
        };
        let Some(job) = self.dataset_job.take() else {
            return;
        };
        self.state.dataset_operation = None;
        match result {
            Ok(DatasetResult::Saved) => {
                self.state.dataset_path = Some(job.path);
                if job.revision == self.dataset_revision {
                    self.state.dataset_dirty = false;
                }
                self.dataset_feedback("Dataset saved");
            }
            Ok(DatasetResult::Imported(prepared)) => {
                if job.revision != self.dataset_revision {
                    self.error(&anyhow::anyhow!(
                        "Dataset changed during import; nothing was imported"
                    ));
                    return;
                }
                let replace = prepared.mode == DatasetImportMode::Replace;
                self.state.classes = prepared.state.classes;
                self.state.erasers = prepared.state.erasers;
                self.state.next_class_id = prepared.state.next_class_id;
                self.samples = prepared.samples;
                self.next_stroke_id = prepared.next_stroke_id;
                self.training_generation = self.training_generation.wrapping_add(1);
                if replace {
                    self.current = None;
                    self.stroke = None;
                    self.state.selected_stroke = None;
                    self.annotation_masks.clear();
                    self.state.hidden_classes.clear();
                    let Some(fallback) = self.state.classes.keys().nth(1).copied() else {
                        self.error(&anyhow::anyhow!("Missing foreground class"));
                        return;
                    };
                    self.state.active_class = fallback;
                    self.state.output_class = fallback;
                    self.state.brush = AnnotationKind::Paint(fallback);
                    if let Some(ids) = prepared.model_class_ids {
                        self.model_class_ids = ids;
                        self.inference_cache = None;
                        self.state.model_revision = self.state.model_revision.wrapping_add(1);
                    } else {
                        self.invalidate_model();
                    }
                }
                self.dataset_edited();
                if replace {
                    self.state.dataset_path = Some(job.path);
                    self.state.dataset_dirty = false;
                }
                self.rebuild_mask();
                self.cache.invalidate();
                self.changed();
                self.dataset_feedback(if replace {
                    "Dataset loaded"
                } else {
                    "Dataset merged"
                });
            }
            Err(error) => self.error(&error),
        }
    }

    fn dataset_feedback(&self, message: &str) {
        self.base.events.emit(FrontendMessage::success(message));
    }

    fn class(&self, id: ClassId) -> Result<&SegmentationClass> {
        self.state
            .classes
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("Unknown class"))
    }
    fn install_classes(&mut self, classes: SegmentationClasses) -> Result<()> {
        let active = *classes
            .keys()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Missing foreground class"))?;
        self.state.next_class_id = self.state.next_class_id.max(
            classes
                .keys()
                .next_back()
                .copied()
                .map_or(0, ClassId::get)
                .saturating_add(1),
        );
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
        self.reset_dataset_status();
        Ok(())
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
    fn render_prediction(&self, planes: &[image::GrayImage]) -> Result<RgbaImage> {
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
            .map(|id| self.class(*id).map(|class| class.bgra(255)))
            .collect::<Result<_>>()?;
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
        let colors = match self
            .model_class_ids
            .iter()
            .map(|id| {
                let class = self.class(*id)?;
                let hidden = self.state.hidden_classes.contains(id)
                    || (self.state.preview_selected && self.state.active_class != *id);
                Ok(class.bgra(if hidden { 0 } else { 255 }))
            })
            .collect::<Result<Vec<_>>>()
        {
            Ok(colors) => colors,
            Err(error) => {
                self.error(&error);
                return;
            }
        };
        if cached
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.colors == colors)
        {
            return;
        }
        let image = match render_class_prediction(&cached.output, &colors) {
            Ok(image) => Arc::new(image),
            Err(error) => {
                self.error(&error);
                return;
            }
        };
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
                strokes
                    .into_values()
                    .filter(|&(kind, stroke)| annotation_visible(&self.state, kind, stroke))
                    .map(|(kind, stroke)| (stroke, annotation_color(kind, &self.state.classes))),
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
    fn error(&self, error: &anyhow::Error) {
        eprintln!("Segmentation: {error:#}");
        self.base
            .events
            .emit(FrontendMessage::error(format!("Segmentation: {error:#}")));
    }
    fn start_training(&mut self) -> Result<()> {
        ensure!(self.training_job.is_none(), "Training is already running");
        ensure!(!self.importing_dataset(), "A dataset import is running");
        self.state.features.validate()?;
        self.state.forest.validate()?;
        self.state.classes.validate()?;
        let classes: Vec<_> = self.state.classes.keys().copied().collect();
        let samples: Vec<_> = self
            .samples
            .iter()
            .enumerate()
            .filter(|(index, _)| self.state.sample_annotations(*index).next().is_some())
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
        let (tx, rx) = flume::bounded(1);
        self.training_job = Some(TrainingJob {
            receiver: rx,
            generation: self.training_generation,
        });
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
    #[expect(
        clippy::too_many_lines,
        reason = "Keep exhaustive tool command dispatch in one match"
    )]
    fn handle(&mut self, message: AutomaticSegmentationMessage) -> Result<()> {
        ensure!(
            !self.importing_dataset()
                || matches!(
                    &message,
                    AutomaticSegmentationMessage::SetShowOverlay(_)
                        | AutomaticSegmentationMessage::SetOverlayOpacity(_)
                        | AutomaticSegmentationMessage::SetPreviewSelected(_)
                        | AutomaticSegmentationMessage::SetOutputMode(_)
                        | AutomaticSegmentationMessage::SetOutputClass(_)
                        | AutomaticSegmentationMessage::ToggleThreshold(_)
                        | AutomaticSegmentationMessage::SetThreshold(_)
                        | AutomaticSegmentationMessage::SetInference(_)
                        | AutomaticSegmentationMessage::SaveModel(_)
                ),
            "A dataset import is running"
        );
        let changes_dataset = matches!(
            &message,
            AutomaticSegmentationMessage::AddClass
                | AutomaticSegmentationMessage::RenameClass(..)
                | AutomaticSegmentationMessage::SetClassColor(..)
                | AutomaticSegmentationMessage::DeleteClass(..)
                | AutomaticSegmentationMessage::ToggleStrokeVisibility(..)
                | AutomaticSegmentationMessage::DeleteStroke(..)
                | AutomaticSegmentationMessage::UndoStroke
                | AutomaticSegmentationMessage::ClearAnnotations
        );
        match message {
            AutomaticSegmentationMessage::AddClass => {
                let id = self.state.next_class_id;
                let next_class_id = id
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Class ID exhausted"))?;
                let id = ClassId::new(id);
                let name = self.state.classes.unused_name()?;
                let palette_len = u64::try_from(CLASS_PALETTE.len())?;
                let index = usize::try_from(id.get().checked_rem(palette_len).unwrap_or_default())?;
                let color = *CLASS_PALETTE
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("Empty class palette"))?;
                self.state.classes.insert(
                    id,
                    SegmentationClass {
                        name,
                        color,
                        annotations: HashMap::new(),
                    },
                )?;
                self.state.next_class_id = next_class_id;
                self.state.brush = AnnotationKind::Paint(id);
                self.state.active_class = id;
                self.edited();
            }
            AutomaticSegmentationMessage::RenameClass(id, name) => {
                self.state.classes.rename(id, &name)?;
            }
            AutomaticSegmentationMessage::SetClassColor(id, color) => {
                self.state.classes.set_color(id, color)?;
                self.rebuild_mask();
            }
            AutomaticSegmentationMessage::DeleteClass(id) => {
                self.state.classes.remove(id)?;
                self.state.hidden_classes.retain(|&c| c != id);
                let fallback = *self
                    .state
                    .classes
                    .keys()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Missing remaining class"))?;
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
                self.class(id)?;
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
                self.class(id)?;
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
                self.state.use_threshold = value;
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
                    self.class(id)?;
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
                if let Some(sample) = self.current
                    && let Some((&id, _)) = self.state.ordered_annotations(sample).last_key_value()
                {
                    self.state.remove_stroke(sample, id);
                }
                self.edited();
            }
            AutomaticSegmentationMessage::ClearAnnotations => {
                if let Some(sample) = self.current {
                    self.state.clear_sample_annotations(sample);
                }
                self.edited();
            }
            AutomaticSegmentationMessage::SaveDataset(path) => self.start_dataset_save(path)?,
            AutomaticSegmentationMessage::LoadDataset(path, mode) => {
                self.start_dataset_import(path, mode)?;
            }
            AutomaticSegmentationMessage::TrainModel => self.start_training()?,
            AutomaticSegmentationMessage::SaveModel(path) => {
                let bytes = self.model_bytes()?;
                write_artifact(&path, |file| {
                    file.write_all(&bytes)?;
                    Ok(())
                })?;
                self.state.model_path = Some(path);
            }
            AutomaticSegmentationMessage::LoadModel(path) => {
                let document = ModelDocument::load(&std::fs::read(&path)?)?;
                self.install_classes(document.classes)?;
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
        if changes_dataset {
            self.dataset_edited();
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
                stroke.stroke.points.get(appended).unwrap_or_default(),
                stroke.stroke.radius,
                annotation_color(stroke.kind, &self.state.classes),
            );
        }
    }
    fn finish_training(&mut self) {
        let result = self
            .training_job
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(flume::TryRecvError::Empty) => None,
                Err(flume::TryRecvError::Disconnected) => {
                    Some(Err(anyhow::anyhow!("Training worker stopped unexpectedly")))
                }
            });
        if let Some(result) = result {
            let Some(job) = self.training_job.take() else {
                return;
            };
            self.state.busy = false;
            match result {
                _ if job.generation != self.training_generation || !self.state.is_enabled => {}
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

    fn receive_commands(&mut self, message: ToolInput) -> Result<()> {
        match message {
            ToolInput::Message(PipelineObjectMessage::AutomaticSegmentationMessage(message)) => {
                if let Err(error) = self.handle(message) {
                    self.error(&error);
                }
            }
            ToolInput::InteractionStart(x, y)
                if self.state.is_enabled && self.state.training && !self.importing_dataset() =>
            {
                if self.ensure_sample().is_some() {
                    self.next_stroke_id = self
                        .next_stroke_id
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("Stroke ID exhausted"))?;
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
            ToolInput::InteractionMove(x, y) if self.stroke.is_some() => {
                self.add_point(x, y);
            }
            ToolInput::InteractionEnd => {
                if let Some(stroke) = self.stroke.take()
                    && !stroke.stroke.points.is_empty()
                {
                    if let Some(sample) = self.current {
                        match stroke.kind {
                            AnnotationKind::Paint(id) => {
                                if let Some(annotations) = self.state.classes.annotations_mut(id) {
                                    annotations
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
                    self.dataset_edited();
                    self.edited();
                }
            }
            _ => {}
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
    fn load_fields(
        &mut self,
        fields: serde_value::Value,
        assets: &ToolAssets,
        cli: bool,
    ) -> Result<()> {
        let mut state = self.state.clone();
        AutomaticSegmentationState::deserialize_in_place(fields, &mut state)?;
        state.features.validate()?;
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
        state.classes.validate()?;
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
                .map_or(0, ClassId::get)
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
        state.dataset_samples = 0;
        state.dataset_path = None;
        state.dataset_dirty = false;
        state.dataset_operation = None;
        state.model_revision = self.state.model_revision.wrapping_add(1);

        {
            self.state = state;
            self.model_class_ids = model
                .as_ref()
                .map_or_else(Vec::new, |document| document.class_ids.clone());
            self.model = model.map(|document| document.model);
            self.reset_dataset_status();
            self.samples.clear();
            self.current = None;
            self.input = None;
            self.annotation_masks.clear();
            self.inference_cache = None;
            self.training_job = None;
            self.stroke = None;
            self.training_generation = self.training_generation.wrapping_add(1);
            self.cache.invalidate();
            self.changed();
        }
        Ok(())
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
            *layer = Arc::new(self.render_prediction(&output)?);
        }

        Ok(())
    }
    fn handle_input(&mut self, input: ToolInput) -> Result<()> {
        self.receive_commands(input)
    }
    fn take_overlay_changes(&mut self) -> bool {
        std::mem::take(&mut self.overlays_dirty)
    }
    fn cancel_interaction(&mut self) {
        if self.stroke.take().is_some() {
            self.rebuild_mask();
            self.changed();
        }
    }
    fn poll_background(&mut self) -> Result<()> {
        // Disabling or leaving training mode cancels the transient stroke.
        if (!self.state.is_enabled || !self.state.training) && self.stroke.take().is_some() {
            self.rebuild_mask();
            self.changed();
        }
        self.finish_training();
        self.finish_dataset_job();
        Ok(())
    }
    fn get_overlays(&self, id: strawberry_types::ToolId) -> Option<ToolOverlay> {
        if !self.state.is_enabled || !self.state.training {
            return None;
        }
        let mut elements = Vec::new();
        if self.state.show_overlay
            && let Some(cached) = self.inference_cache.as_ref().filter(|cached| {
                self.input
                    .as_ref()
                    .zip(self.model.as_ref())
                    .is_some_and(|(image, model)| {
                        cached.matches(self.state.selected_layer, image, model)
                    })
            })
            && let Some(overlay) = &cached.overlay
        {
            elements.push(OverlayElement::ImageMask {
                x: 0,
                y: 0,
                opacity: f32::from(self.state.overlay_opacity) / 100.,
                image: Some(overlay.image.clone()),
            });
        }
        elements.extend(self.annotation_masks.iter().map(|(&(x, y), image)| {
            OverlayElement::ImageMask {
                x,
                y,
                opacity: 1.,
                image: Some(image.clone()),
            }
        }));
        if let Some(sample) = self.current
            && let Some(index) = self.state.selected_stroke
            && let Some(stroke) = self.state.stroke(sample, index)
            && let Some(&(x, y)) = stroke.points.first()
        {
            elements.push(OverlayElement::Circle {
                x: num_traits::cast::<u32, f32>(x).unwrap_or(0.0) + 0.5,
                y: num_traits::cast::<u32, f32>(y).unwrap_or(0.0) + 0.5,
                radius: num_traits::cast(stroke.radius.get()).unwrap_or(1.0),
                color: RgbaColor::WHITE,
                style: CircleStyle::Selected { width: 1.0 },
            });
        }
        Some(ToolOverlay {
            tool_id: id,
            z_order: 50,
            elements,
        })
    }
}
