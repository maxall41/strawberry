use image::RgbaImage;
use std::path::PathBuf;
use std::sync::Arc;
use strawberry_processing::local_maxima::ThresholdType;

use super::{BackendToolType, ToolState};

#[derive(Debug, Clone)]
pub struct OutputFrame {
    pub load_generation: u64,
    pub image: Arc<RgbaImage>,
}

#[derive(Debug, Clone)]
pub enum CompositorMessage {
    ToggleVisibility(bool, u16),
}

#[derive(Debug, Clone)]
pub enum DecomposerMessage {
    ToggleVisibility(bool, u16),
}

#[derive(Debug, Clone)]
pub enum ContrastMessage {
    SetMinMax(f32, f32),
}

#[derive(Debug, Clone)]
pub enum GaussianBlurMessage {
    SetSigma(f32),
}

#[derive(Debug, Clone)]
pub enum LocalMaximaMessage {
    SetParameters(u32, ThresholdType, bool), // min_distance, threshold, exclude_border
}

#[derive(Debug, Clone)]
pub enum MorphologyMessage {
    SetOperation(strawberry_processing::morphology::MorphologyOperation),
    SetElementSize(u32),
}

#[derive(Debug, Clone)]
pub enum ManagerMessage {
    ReorderTool(u16, u16),
    SavePipeline(PathBuf),
    LoadPipeline(PathBuf),
}

#[derive(Debug, Clone)]
pub enum RollingBallMessage {
    SetBallRadius(u32),
}

#[derive(Debug, Clone)]
pub enum ThresholdMessage {
    SetThresholdValue(f32),
}

#[derive(Debug, Clone)]
pub enum WatershedMessage {
    SetParameters(f32, u32), // sigma, min_distance
}

#[derive(Debug, Clone)]
pub enum SobelMessage {
    SetThreshold(f32),
    ToggleThreshold(bool),
}

#[derive(Debug, Clone)]
pub enum FilterMessage {
    ConfigureSizeFilter(u16), // Min size
}

#[derive(Debug, Clone)]
pub enum NerveTracerMessage {
    CommitCurrentPathSegments(),
    DeleteUncomittedPathSegment(usize),
    DeletePathSegmentInCommitedPath(usize, usize),
    DeletePath(usize),
    SaveTraces(PathBuf),
    LoadTraces(PathBuf),
    SetSelectedImageIndex(usize),
}

// Note: counter module types need to be forward-declared since they're in the backend crate
// We use a placeholder type here that will be resolved when the backend crate depends on this
#[derive(Debug, Clone)]
pub enum CountMessage {
    NewCountType(u16),
    DeleteCountType(u16),
    ToggleCountTypeVisibility(u16),
    SetMode(CounterMode),
    SetActiveCountType(u16),
    SetActiveTool(Option<CounterTool>),
    ToggleFilter(u16, bool),
    FilterMessage(FilterMessage),
    SaveCounts(PathBuf),
    LoadCounts(PathBuf),
    CopyToManual,
    ReviewNavigatePrevious,
    ReviewNavigateNext,
    DeleteCurrentCount,
    DeletionLasso(Vec<(u64, u64)>),
}

// Import counter types from state module
use crate::{CounterMode, CounterTool};

#[derive(Debug, Clone)]
pub enum PipelineObjectMessage {
    DecomposerMessage(DecomposerMessage),
    CompositorMessage(CompositorMessage),
    ContrastMessage(ContrastMessage),
    GaussianBlurMessage(GaussianBlurMessage),
    LocalMaximaMessage(LocalMaximaMessage),
    MorphologyMessage(MorphologyMessage),
    RollingBallMessage(RollingBallMessage),
    SobelMessage(SobelMessage),
    ThresholdMessage(ThresholdMessage),
    WatershedMessage(WatershedMessage),
    ToggleActive(bool),
    ManagerMessage(ManagerMessage),
    CountMessage(CountMessage),
    NerveTracerMessage(NerveTracerMessage),
}

/// Severity of a `UserFeedback` toast; drives the toast's progress bar color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackLevel {
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct PipelineProgress {
    /// 0-based index of the stage about to run (display code adds 1).
    pub stage: usize,
    /// Number of stages this run will execute in total.
    pub total: usize,
    /// Display name
    pub name: String,
}

#[derive(Debug, Clone)]
pub enum FrontendMessage {
    ReturnState(ToolState),
    ReturnBackendTools(Vec<u16>, Vec<flume::Sender<PipelineCommand>>),
    UserFeedback(String, FeedbackLevel),
    ReturnOverlays(crate::overlay::OverlayCollection),
    SnapToPosition(f32, f32),
    /// Sent just before each stage of a pipeline run executes.
    PipelineProgress(PipelineProgress),
    PipelineFinished,
}

impl FrontendMessage {
    pub fn error(message: impl Into<String>) -> Self {
        Self::UserFeedback(message.into(), FeedbackLevel::Error)
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self::UserFeedback(message.into(), FeedbackLevel::Success)
    }
}

/// Commands that can be sent to the pipeline
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// Load input images into the pipeline
    Load(Vec<PathBuf>),
    /// Message to pipeline object
    PipelineObject(PipelineObjectMessage),
    /// Message to frontend
    FrontendMessage(FrontendMessage),
    /// Request state from pipeline object
    RequestState(),
    /// Discover
    DiscoverBackend(BackendToolType, flume::Sender<Self>),
    /// Disconnect frontend tool
    DisconnectFrontend(BackendToolType),
    /// Handle click at x and y image pixels
    HandleClick(u64, u64),
    /// Register click receiver
    RegisterClickReceiver(flume::Sender<Self>),
    /// Command used by CLI to avoid discovery negoitation
    CLILoadPipeline(PathBuf),
    /// Save output image to file
    SaveOutput(PathBuf),
    /// Request overlay updates from all tools
    RequestOverlays,
    /// Interaction start (mouse down) at image coordinates
    InteractionStart(u64, u64),
    /// Interaction move (mouse move while dragging) at image coordinates
    InteractionMove(u64, u64),
    /// Interaction end (mouse up)
    InteractionEnd,
}
