use std::path::PathBuf;
use image::RgbaImage;
use strawberry_processing::local_maxima::ThresholdType;

use super::*;

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
}

#[derive(Debug, Clone)]
pub enum FrontendMessage {
    ReturnState(ToolState),
    ReturnBackendTools(Vec<u16>, Vec<flume::Sender<PipelineCommand>>),
    UserFeedback(String),
    ReturnOverlays(crate::overlay::OverlayCollection),
    SnapToPosition(f32, f32),
}

/// Commands that can be sent to the pipeline
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// Refresh the pipeline starting from the given tool index
    Refresh(usize),
    /// Load input images into the pipeline
    Load(Vec<PathBuf>),
    /// Return output image from the pipeline
    Output(RgbaImage),
    /// Message to pipeline object
    PipelineObject(PipelineObjectMessage),
    /// Message to frontend
    FrontendMessage(FrontendMessage),
    /// Request state from pipeline object
    RequestState(),
    /// Discover
    DiscoverBackend(BackendToolType, flume::Sender<PipelineCommand>),
    /// Disconnect frontend tool
    DisconnectFrontend(BackendToolType),
    /// Handle click at x and y image pixels
    HandleClick(u64, u64),
    /// Register click receiver
    RegisterClickReceiver(flume::Sender<PipelineCommand>),
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
