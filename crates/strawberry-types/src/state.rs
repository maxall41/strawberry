use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FilterProxy {
    pub name: String,
    pub is_enabled: bool,
    pub values: Vec<f64>,
}

// Manager-specific types
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProxy {
    pub name: String,
    pub position: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub tools: HashMap<String, ToolState>,
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
pub struct ManagerState {
    pub proxy_tools: Vec<ToolProxy>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MorphologyState {
    pub operation: MorphologyOperation,
    pub element_size: u32,
    pub is_enabled: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CounterState {
    pub is_enabled: bool,
    pub manual_counts: HashMap<u16, Vec<CountData>>,
    pub auto_counts: Vec<CountData>,
    pub mode: CounterMode,
    pub active_tool: Option<CounterTool>,
    pub active_count_type: Option<u16>,
    pub filter_proxys: Vec<FilterProxy>,
    pub count_type_visibility: HashMap<u16, bool>,
    pub review_current_index: usize,
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

/// Aggregated tool state enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolState {
    CompositorState(CompositorState),
    ContrastState(ContrastState),
    GaussianBlurState(GaussianBlurState),
    GrayscaleState(GrayscaleState),
    LocalMaximaState(LocalMaximaState),
    ManagerState(ManagerState),
    MorphologyState(MorphologyState),
    RollingBallState(RollingBallState),
    SobelState(SobelState),
    ThresholdState(ThresholdState),
    CounterState(CounterState),
    DecomposerState(DecomposerState),
    WatershedState(WatershedState),
    NerveTracerState(NerveTracerState),
}

/// A validated state update that takes effect only when invoked.
pub type PreparedStateUpdate<'a> = Box<dyn FnOnce() + 'a>;

/// Object-safe access to a concrete state struct's Serde in-place update.
pub trait PersistedState {
    /// Prepares a state update, retaining fields marked `serde(skip)`. The returned closure must be executed for the state update to take place.
    /// # Errors
    /// Returns an error if the fields cannot be deserialized; leaves state unchanged.
    fn prepare_persisted_fields<'a>(
        &'a mut self,
        fields: serde_value::Value,
    ) -> anyhow::Result<PreparedStateUpdate<'a>>;
}

impl<T: Clone + serde::de::DeserializeOwned> PersistedState for T {
    fn prepare_persisted_fields<'a>(
        &'a mut self,
        fields: serde_value::Value,
    ) -> anyhow::Result<PreparedStateUpdate<'a>> {
        let mut merged = self.clone();
        T::deserialize_in_place(fields, &mut merged)?;
        Ok(Box::new(move || *self = merged))
    }
}
