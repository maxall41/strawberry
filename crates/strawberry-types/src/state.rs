use crate::{BgraColor, RgbColor};
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::collections::{BTreeMap, HashMap, HashSet, btree_map};
use std::path::PathBuf;
use std::{
    fmt::{self, Display, Formatter},
    num::ParseIntError,
    ops::Deref,
    sync::Arc,
};
use strawberry_processing::segmentation::{FeatureConfig, ForestConfig, InferenceConfig};
use strawberry_processing::{local_maxima::ThresholdType, morphology::MorphologyOperation};

// Counter-specific types
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountData {
    pub position: (u64, u64), // X, Y
    pub size: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CounterMode {
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CounterTool {
    #[default]
    Place,
    Delete,
    LassoDelete,
    Review,
}

// Individual tool states
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositorState {
    pub is_enabled: bool,
    pub enabled_layers: Vec<bool>,
    /// Don't include runtime layer metadata in persisted state
    #[serde(skip)]
    pub real_layers_len: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContrastState {
    pub min: f32,
    pub max: f32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GaussianBlurState {
    pub sigma: f32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DifferenceOfGaussiansState {
    pub small_sigma: f32,
    pub large_sigma: f32,
    pub is_enabled: bool,
}

impl Default for DifferenceOfGaussiansState {
    fn default() -> Self {
        Self {
            small_sigma: 1.0,
            large_sigma: 2.0,
            is_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrayscaleState {
    pub is_enabled: bool,
}

type PathSegment = Vec<(u64, u64)>;
type Path = Vec<PathSegment>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NerveTracerState {
    pub is_enabled: bool,
    pub paths: Vec<Path>,
    pub current_path_segments: Vec<PathSegment>,
    pub first_point: Option<(u64, u64)>,
    pub selected_image_index: usize,
    /// Don't include runtime image count metadata in persisted state
    #[serde(skip)]
    pub total_images: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalMaximaState {
    pub min_distance: u32,
    pub threshold: ThresholdType,
    pub exclude_border: bool,
    pub is_enabled: bool,
}

impl Default for LocalMaximaState {
    fn default() -> Self {
        Self {
            min_distance: 5,
            threshold: ThresholdType::Absolute(0.0),
            exclude_border: true,
            is_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MorphologyState {
    pub operation: MorphologyOperation,
    pub element_size: u32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectSizeFilterMode {
    #[default]
    RemoveSmallerThan,
    RemoveLargerThan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSizeFilterState {
    pub is_enabled: bool,
    pub mode: ObjectSizeFilterMode,
    pub size: u32,
}

impl Default for ObjectSizeFilterState {
    fn default() -> Self {
        Self {
            is_enabled: false,
            mode: ObjectSizeFilterMode::default(),
            size: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollingBallState {
    pub ball_radius: u32,
    pub is_enabled: bool,
}

impl Default for RollingBallState {
    fn default() -> Self {
        Self {
            ball_radius: 50,
            is_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SobelState {
    pub threshold_value: f32,
    pub use_threshold: bool,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ThresholdState {
    pub threshold_value: f32,
    pub is_enabled: bool,
}

// TOML table keys are strings, including numeric count-type IDs.
fn deserialize_count_type_map<'de, D, V>(
    deserializer: D,
) -> std::result::Result<HashMap<u16, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    let entries = HashMap::<serde_value::Value, V>::deserialize(deserializer)?;
    let mut result = HashMap::with_capacity(entries.len());
    for (key, value) in entries {
        let id = match key {
            serde_value::Value::String(text) => text.parse().map_err(D::Error::custom)?,
            numeric => u16::deserialize(numeric).map_err(D::Error::custom)?,
        };
        if result.insert(id, value).is_some() {
            return Err(D::Error::custom("Duplicate count type ID"));
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterState {
    pub is_enabled: bool,
    #[serde(deserialize_with = "deserialize_count_type_map")]
    pub manual_counts: HashMap<u16, Vec<CountData>>,
    pub auto_counts: Vec<CountData>,
    pub mode: CounterMode,
    pub active_tool: Option<CounterTool>,
    pub active_count_type: Option<u16>,
    #[serde(deserialize_with = "deserialize_count_type_map")]
    pub count_type_visibility: HashMap<u16, bool>,
    pub review_current_index: usize,
    #[serde(deserialize_with = "deserialize_count_type_map")]
    pub count_type_names: HashMap<u16, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecomposerState {
    pub is_enabled: bool,
    pub enabled_layers: Vec<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WatershedState {
    pub sigma: f32,
    pub min_distance: u32,
    pub is_enabled: bool,
}

/// A brush radius validated when reading messages or saved pipeline settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct BrushRadius(u32);

impl BrushRadius {
    pub const MIN: u32 = 1;
    pub const MAX: u32 = 128;

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for BrushRadius {
    fn default() -> Self {
        Self(1)
    }
}

impl TryFrom<u32> for BrushRadius {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!(
                "Brush radius must be in {}..={}",
                Self::MIN,
                Self::MAX
            ))
        }
    }
}

impl From<BrushRadius> for u32 {
    fn from(radius: BrushRadius) -> Self {
        radius.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClassId(u64);

impl ClassId {
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Display for ClassId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<ClassId> for String {
    fn from(id: ClassId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<String> for ClassId {
    type Error = ParseIntError;
    fn try_from(id: String) -> Result<Self, Self::Error> {
        id.parse().map(Self)
    }
}

pub type StrokeId = usize;
/// Index of an input image retained by the segmentation tool.
pub type SampleId = usize;
pub type AnnotationsBySample = HashMap<SampleId, BTreeMap<StrokeId, AnnotationStroke>>;
pub const MAX_CLASSES: usize = 256;

/// Ordered by ID for stable class presentation and model class mappings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SegmentationClasses(BTreeMap<ClassId, SegmentationClass>);

impl SegmentationClasses {
    /// # Errors
    /// Returns an error if class count, names, or name uniqueness are invalid.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (2..=MAX_CLASSES).contains(&self.len()),
            "Define between 2 and {MAX_CLASSES} classes"
        );
        let mut names = HashSet::new();
        for class in self.values() {
            ensure!(!class.name.trim().is_empty(), "Class names cannot be empty");
            ensure!(
                names.insert(class.name.trim()),
                "Duplicate class name: {}",
                class.name
            );
        }
        Ok(())
    }

    fn check_name(&self, id: ClassId, name: &str) -> Result<()> {
        ensure!(!name.is_empty(), "Class names cannot be empty");
        ensure!(
            self.iter()
                .all(|(&other, class)| other == id || class.name != name),
            "Duplicate class name: {name}"
        );
        Ok(())
    }

    /// # Errors
    /// Returns an error for a duplicate ID, invalid name, or class limit violation.
    pub fn insert(&mut self, id: ClassId, mut class: SegmentationClass) -> Result<()> {
        ensure!(!self.contains_key(&id), "Class ID already exists");
        ensure!(self.len() < MAX_CLASSES, "Maximum of {MAX_CLASSES} classes");
        class.name = class.name.trim().into();
        self.check_name(id, &class.name)?;
        self.0.insert(id, class);
        Ok(())
    }

    /// # Errors
    /// Returns an error if the class is unknown or the name is empty or duplicated.
    pub fn rename(&mut self, id: ClassId, name: &str) -> Result<()> {
        let name = name.trim();
        self.check_name(id, name)?;
        name.clone_into(
            &mut self
                .0
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("Unknown class"))?
                .name,
        );
        Ok(())
    }

    /// # Errors
    /// Returns an error if the class is unknown or fewer than two classes would remain.
    pub fn remove(&mut self, id: ClassId) -> Result<()> {
        ensure!(self.len() > 2, "Keep at least two classes");
        ensure!(self.0.remove(&id).is_some(), "Unknown class");
        Ok(())
    }

    /// # Errors
    /// Returns an error if the class is unknown.
    pub fn set_color(&mut self, id: ClassId, color: RgbColor) -> Result<()> {
        self.0
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("Unknown class"))?
            .color = color;
        Ok(())
    }

    pub fn annotations_mut(&mut self, id: ClassId) -> Option<&mut AnnotationsBySample> {
        self.0.get_mut(&id).map(|class| &mut class.annotations)
    }

    #[must_use]
    pub fn without_annotations(&self) -> Self {
        Self(
            self.iter()
                .map(|(&id, class)| {
                    (
                        id,
                        SegmentationClass {
                            name: class.name.clone(),
                            color: class.color,
                            annotations: HashMap::new(),
                        },
                    )
                })
                .collect(),
        )
    }

    /// # Errors
    /// Returns an error if no unused name can be generated.
    pub fn unused_name(&self) -> Result<String> {
        let start = self.len().saturating_add(1);
        let end = start.saturating_add(self.len());
        (start..=end)
            .map(|number| format!("Class {number}"))
            .find(|name| self.values().all(|class| class.name != *name))
            .ok_or_else(|| anyhow::anyhow!("No unused class name"))
    }
}

impl TryFrom<BTreeMap<ClassId, SegmentationClass>> for SegmentationClasses {
    type Error = anyhow::Error;

    fn try_from(mut classes: BTreeMap<ClassId, SegmentationClass>) -> Result<Self> {
        for class in classes.values_mut() {
            class.name = class.name.trim().into();
        }
        let classes = Self(classes);
        classes.validate()?;
        Ok(classes)
    }
}

impl<'de> Deserialize<'de> for SegmentationClasses {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        BTreeMap::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

impl Deref for SegmentationClasses {
    type Target = BTreeMap<ClassId, SegmentationClass>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for SegmentationClasses {
    type Item = (ClassId, SegmentationClass);
    type IntoIter = btree_map::IntoIter<ClassId, SegmentationClass>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a SegmentationClasses {
    type Item = (&'a ClassId, &'a SegmentationClass);
    type IntoIter = btree_map::Iter<'a, ClassId, SegmentationClass>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentationClass {
    pub name: String,
    pub color: RgbColor,
    #[serde(skip)]
    pub annotations: AnnotationsBySample,
}
impl SegmentationClass {
    #[must_use]
    pub const fn bgra(&self, alpha: u8) -> BgraColor {
        self.color.with_alpha(alpha).to_bgra()
    }

    pub fn annotation_count(&self) -> usize {
        self.annotations.values().map(BTreeMap::len).sum()
    }
}
pub const CLASS_PALETTE: [RgbColor; 8] = [
    RgbColor::new(230, 70, 80),
    RgbColor::new(40, 220, 100),
    RgbColor::new(75, 145, 245),
    RgbColor::new(245, 190, 55),
    RgbColor::new(185, 110, 235),
    RgbColor::new(45, 205, 210),
    RgbColor::new(245, 135, 65),
    RgbColor::new(230, 120, 175),
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotationKind {
    Paint(ClassId),
    Eraser,
}
impl Default for AnnotationKind {
    fn default() -> Self {
        Self::Paint(ClassId::new(1))
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentationOutputMode {
    #[default]
    ClassMask,
    ClassColors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationStroke {
    pub visible: bool,
    pub radius: BrushRadius,
    pub points: Arc<Vec<(u32, u32)>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetImportMode {
    Replace,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetOperation {
    Saving,
    Loading,
    Merging,
}

impl DatasetOperation {
    #[must_use]
    pub const fn is_importing(self) -> bool {
        matches!(self, Self::Loading | Self::Merging)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Processing, visibility, and runtime job flags vary independently"
)]
pub struct AutomaticSegmentationState {
    pub is_enabled: bool,
    pub training: bool,
    pub show_overlay: bool,
    pub overlay_opacity: u8,
    pub use_threshold: bool,
    pub threshold_value: f32,
    pub classes: SegmentationClasses,
    pub next_class_id: u64,
    pub hidden_classes: Vec<ClassId>,
    pub preview_selected: bool,
    pub output_mode: SegmentationOutputMode,
    pub output_class: ClassId,
    pub active_class: ClassId,
    pub brush: AnnotationKind,
    pub brush_radius: BrushRadius,
    pub selected_layer: usize,
    #[serde(skip)]
    pub total_layers: usize,
    pub features: FeatureConfig,
    pub forest: ForestConfig,
    pub inference: InferenceConfig,
    #[serde(skip)]
    pub model_path: Option<PathBuf>,
    #[serde(skip)]
    pub dataset_samples: usize,
    #[serde(skip)]
    pub dataset_path: Option<PathBuf>,
    #[serde(skip)]
    pub dataset_dirty: bool,
    #[serde(skip)]
    pub dataset_operation: Option<DatasetOperation>,
    #[serde(skip)]
    pub erasers: AnnotationsBySample,
    #[serde(skip)]
    pub current_sample: Option<SampleId>,
    #[serde(skip)]
    pub selected_stroke: Option<StrokeId>,
    #[serde(skip)]
    pub busy: bool,
    #[serde(skip)]
    pub model_ready: bool,
    #[serde(skip)]
    pub oob_error: Option<f64>,
    /// Invalidates pipeline output only when the trained or loaded model changes.
    #[serde(skip)]
    pub model_revision: u64,
}

impl AutomaticSegmentationState {
    pub fn annotation_count(&self) -> usize {
        self.classes
            .values()
            .map(SegmentationClass::annotation_count)
            .chain(self.erasers.values().map(BTreeMap::len))
            .sum()
    }

    pub fn sample_annotations(
        &self,
        sample: SampleId,
    ) -> impl Iterator<Item = (AnnotationKind, StrokeId, &AnnotationStroke)> {
        self.classes
            .iter()
            .flat_map(move |(&id, class)| {
                class
                    .annotations
                    .get(&sample)
                    .into_iter()
                    .flat_map(move |strokes| {
                        strokes.iter().map(move |(&stroke, annotation)| {
                            (AnnotationKind::Paint(id), stroke, annotation)
                        })
                    })
            })
            .chain(self.erasers.get(&sample).into_iter().flat_map(|strokes| {
                strokes
                    .iter()
                    .map(|(&stroke, annotation)| (AnnotationKind::Eraser, stroke, annotation))
            }))
    }

    pub fn current_annotations(
        &self,
    ) -> impl Iterator<Item = (AnnotationKind, StrokeId, &AnnotationStroke)> {
        self.current_sample
            .into_iter()
            .flat_map(|sample| self.sample_annotations(sample))
    }

    #[must_use]
    pub fn ordered_annotations(
        &self,
        sample: SampleId,
    ) -> BTreeMap<StrokeId, (AnnotationKind, &AnnotationStroke)> {
        self.sample_annotations(sample)
            .map(|(kind, id, stroke)| (id, (kind, stroke)))
            .collect()
    }

    #[must_use]
    pub fn stroke(&self, sample: SampleId, id: StrokeId) -> Option<&AnnotationStroke> {
        self.erasers
            .get(&sample)
            .and_then(|strokes| strokes.get(&id))
            .or_else(|| {
                self.classes
                    .values()
                    .find_map(|class| class.annotations.get(&sample)?.get(&id))
            })
    }

    pub fn stroke_mut(&mut self, sample: SampleId, id: StrokeId) -> Option<&mut AnnotationStroke> {
        self.erasers
            .get_mut(&sample)
            .and_then(|strokes| strokes.get_mut(&id))
            .or_else(|| {
                self.classes
                    .0
                    .values_mut()
                    .find_map(|class| class.annotations.get_mut(&sample)?.get_mut(&id))
            })
    }

    pub fn remove_stroke(&mut self, sample: SampleId, id: StrokeId) {
        if let Some(strokes) = self.erasers.get_mut(&sample) {
            strokes.remove(&id);
        }
        for class in self.classes.0.values_mut() {
            if let Some(strokes) = class.annotations.get_mut(&sample) {
                strokes.remove(&id);
            }
        }
    }

    pub fn clear_sample_annotations(&mut self, sample: SampleId) {
        self.erasers.remove(&sample);
        for class in self.classes.0.values_mut() {
            class.annotations.remove(&sample);
        }
    }
}

impl Default for AutomaticSegmentationState {
    fn default() -> Self {
        Self {
            is_enabled: false,
            training: true,
            show_overlay: false,
            overlay_opacity: 30,
            use_threshold: false,
            threshold_value: 0.5,
            classes: SegmentationClasses(BTreeMap::from([
                (
                    ClassId::new(0),
                    SegmentationClass {
                        name: "Background".into(),
                        color: CLASS_PALETTE[0],
                        annotations: HashMap::new(),
                    },
                ),
                (
                    ClassId::new(1),
                    SegmentationClass {
                        name: "Foreground".into(),
                        color: CLASS_PALETTE[1],
                        annotations: HashMap::new(),
                    },
                ),
            ])),
            next_class_id: 2,
            hidden_classes: vec![],
            preview_selected: false,
            output_mode: SegmentationOutputMode::ClassMask,
            output_class: ClassId::new(1),
            active_class: ClassId::new(1),
            brush: AnnotationKind::default(),
            brush_radius: BrushRadius::default(),
            selected_layer: 0,
            total_layers: 0,
            features: FeatureConfig::default(),
            forest: ForestConfig::default(),
            inference: InferenceConfig::default(),
            model_path: None,
            dataset_samples: 0,
            dataset_path: None,
            dataset_dirty: false,
            dataset_operation: None,
            erasers: HashMap::new(),
            current_sample: None,
            selected_stroke: None,
            busy: false,
            model_ready: false,
            oob_error: None,
            model_revision: 0,
        }
    }
}

/// Aggregated tool state enum
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolState {
    AutomaticSegmentationState(crate::AutomaticSegmentationState),
    CompositorState(CompositorState),
    ContrastState(ContrastState),
    DifferenceOfGaussiansState(DifferenceOfGaussiansState),
    GaussianBlurState(GaussianBlurState),
    GrayscaleState(GrayscaleState),
    LocalMaximaState(LocalMaximaState),
    MorphologyState(MorphologyState),
    ObjectSizeFilterState(ObjectSizeFilterState),
    RollingBallState(RollingBallState),
    SobelState(SobelState),
    ThresholdState(ThresholdState),
    CounterState(CounterState),
    DecomposerState(DecomposerState),
    WatershedState(WatershedState),
    NerveTracerState(NerveTracerState),
}

/// Object-safe loading into detached candidate instances.
pub trait PersistedState {
    fn load_persisted_fields(&mut self, fields: serde_value::Value) -> anyhow::Result<()>;
}
impl<T: Clone + serde::de::DeserializeOwned> PersistedState for T {
    fn load_persisted_fields(&mut self, fields: serde_value::Value) -> anyhow::Result<()> {
        let mut merged = self.clone();
        T::deserialize_in_place(fields, &mut merged)?;
        *self = merged;
        Ok(())
    }
}

impl ToolState {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::ContrastState(s) => ensure!(
                s.min.is_finite() && s.max.is_finite() && s.min <= s.max,
                "Contrast minimum must not exceed maximum"
            ),
            Self::GaussianBlurState(s) => ensure!(
                s.sigma.is_finite() && s.sigma > 0.,
                "Sigma must be positive"
            ),
            Self::DifferenceOfGaussiansState(s) => ensure!(
                s.small_sigma.is_finite()
                    && s.small_sigma > 0.
                    && s.large_sigma.is_finite()
                    && s.large_sigma > s.small_sigma,
                "Invalid Gaussian sigmas"
            ),
            Self::LocalMaximaState(s) => {
                if let ThresholdType::Absolute(value) | ThresholdType::Relative(value) = s.threshold
                {
                    ensure!(
                        value.is_finite() && (0.0..=1.0).contains(&value),
                        "Invalid maxima threshold"
                    );
                }
            }
            Self::MorphologyState(s) => {
                ensure!(s.element_size > 0, "Element size must be positive")
            }
            Self::RollingBallState(s) => ensure!(s.ball_radius > 0, "Ball radius must be positive"),
            Self::SobelState(s) => ensure!(
                s.threshold_value.is_finite() && (0.0..=1.0).contains(&s.threshold_value),
                "Invalid Sobel threshold"
            ),
            Self::ThresholdState(s) => ensure!(
                s.threshold_value.is_finite() && (0.0..=1.0).contains(&s.threshold_value),
                "Invalid threshold"
            ),
            Self::WatershedState(s) => ensure!(
                s.sigma.is_finite() && s.sigma > 0.,
                "Sigma must be positive"
            ),
            _ => {}
        }
        Ok(())
    }
}
