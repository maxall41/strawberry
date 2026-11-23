use image::RgbaImage;
use std::path::PathBuf;
use std::sync::Arc;
use strawberry_processing::local_maxima::ThresholdType;
use strawberry_processing::segmentation::{FeatureConfig, ForestConfig, InferenceConfig};
use strawberry_processing::threshold::AutoThresholdMethod;

use super::{
    AnnotationKind, AnnotationPlacement, BackendToolType, BrushRadius, ClassId, Coordinates,
    DatasetImportMode, ExportScope, ImageArrangement, ObjectSizeFilterMode, RgbColor,
    SegmentationOutputMode, Shape, StrokeId, ToolState,
};

/// The displayed output plane.
#[derive(Debug, Clone)]
pub struct OutputFrame {
    pub load_generation: u64,
    pub coordinates: Coordinates,
    pub shape: Shape,
    pub image: Arc<RgbaImage>,
    /// False when a failed run left an older result on screen.
    pub is_current: bool,
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
pub enum DifferenceOfGaussiansMessage {
    SetSmallSigma(f32),
    SetLargeSigma(f32),
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
pub enum ObjectSizeFilterMessage {
    SetMode(ObjectSizeFilterMode),
    SetSize(u32),
}

#[derive(Debug, Clone)]
pub enum RollingBallMessage {
    SetBallRadius(u32),
}

#[derive(Debug, Clone)]
pub enum ThresholdMessage {
    SetThresholdValue(u8),
    SetAutoThreshold(bool),
    SetAutoMethod(AutoThresholdMethod),
}

#[derive(Debug, Clone)]
pub enum WatershedMessage {
    SetParameters(f32, u32), // sigma, min_distance
}

#[derive(Debug, Clone)]
pub enum AutomaticSegmentationMessage {
    SetTraining(bool),
    SetShowOverlay(bool),
    SetOverlayOpacity(u8),
    ToggleThreshold(bool),
    SetThreshold(f32),
    AddClass,
    RenameClass(ClassId, String),
    SetClassColor(ClassId, RgbColor),
    DeleteClass(ClassId),
    ToggleClassVisibility(ClassId),
    SetPreviewSelected(bool),
    SetOutputMode(SegmentationOutputMode),
    SetOutputClass(ClassId),
    SetBrush(AnnotationKind),
    SetBrushRadius(BrushRadius),
    SelectChannel(usize),
    SetFeatures(FeatureConfig),
    SetForest(ForestConfig),
    SetInference(InferenceConfig),
    SelectStroke(StrokeId),
    ToggleStrokeVisibility(StrokeId),
    DeleteStroke(StrokeId),
    UndoStroke,
    ClearAnnotations,
    TrainModel,
    SaveModel(PathBuf),
    LoadModel(PathBuf),
    SaveDataset(PathBuf),
    LoadDataset(PathBuf, DatasetImportMode),
}

#[derive(Debug, Clone)]
pub enum SobelMessage {
    SetThreshold(f32),
    ToggleThreshold(bool),
}

#[derive(Debug, Clone)]
pub enum NerveTracerMessage {
    CommitCurrentPathSegments(),
    DeleteUncomittedPathSegment(usize),
    DeletePathSegmentInCommitedPath(usize, usize),
    DeletePath(usize),
    SaveTraces(PathBuf),
    LoadTraces(PathBuf, AnnotationPlacement),
    SelectChannel(usize),
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
    SaveCounts(PathBuf),
    LoadCounts(PathBuf, AnnotationPlacement),
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
    AutomaticSegmentationMessage(AutomaticSegmentationMessage),
    DecomposerMessage(DecomposerMessage),
    CompositorMessage(CompositorMessage),
    ContrastMessage(ContrastMessage),
    DifferenceOfGaussiansMessage(DifferenceOfGaussiansMessage),
    GaussianBlurMessage(GaussianBlurMessage),
    LocalMaximaMessage(LocalMaximaMessage),
    MorphologyMessage(MorphologyMessage),
    ObjectSizeFilterMessage(ObjectSizeFilterMessage),
    RollingBallMessage(RollingBallMessage),
    SobelMessage(SobelMessage),
    ThresholdMessage(ThresholdMessage),
    WatershedMessage(WatershedMessage),
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
    /// 0-based position of the active stage among the stages visited in this pass.
    pub stage: usize,
    /// Number of stages visited in this pass, including restored outputs.
    pub total: usize,
    /// Display name
    pub name: String,
    /// Plane counts when the stage reports progress within its own work.
    pub planes: Option<PlaneProgress>,
}

#[derive(Debug, Clone, Copy)]
pub struct PlaneProgress {
    /// Number of planes the stage has finished processing.
    pub completed: usize,
    /// Number of planes the stage processes in this run.
    pub total: usize,
}

#[derive(Debug, Clone)]
pub enum FrontendMessage {
    ToolEvent {
        id: super::ToolId,
        event: Box<Self>,
    },
    UserFeedback(String, FeedbackLevel),
    ReturnOverlays(crate::overlay::OverlayCollection),
    SnapToPosition(f32, f32),
    /// Sent when a stage starts or reports new progress.
    PipelineProgress(PipelineProgress),
    PipelineFinished,
}

impl FrontendMessage {
    /// Create an error feedback message.
    pub fn error(message: impl Into<String>) -> Self {
        Self::UserFeedback(message.into(), FeedbackLevel::Error)
    }

    /// Create a success feedback message.
    pub fn success(message: impl Into<String>) -> Self {
        Self::UserFeedback(message.into(), FeedbackLevel::Success)
    }
}

pub type CommandSender = tokio::sync::mpsc::UnboundedSender<PipelineCommand>;
pub type CommandResult = tokio::sync::oneshot::Sender<Result<(), String>>;

/// All frontend and CLI mutations share this ordered queue.
#[derive(Debug)]
pub enum PipelineCommand {
    AddTool {
        kind: BackendToolType,
        reply: tokio::sync::oneshot::Sender<Result<super::ToolId, String>>,
    },
    RemoveTool(super::ToolId),
    SetEnabled {
        id: super::ToolId,
        enabled: bool,
    },
    SelectTool(Option<super::ToolId>),
    MoveTool {
        id: super::ToolId,
        before: Option<super::ToolId>,
    },
    ToolMessage {
        id: super::ToolId,
        message: PipelineObjectMessage,
    },
    SubscribePipeline(
        tokio::sync::oneshot::Sender<tokio::sync::watch::Receiver<super::PipelineState>>,
    ),
    SubscribeTool {
        id: super::ToolId,
        reply: tokio::sync::oneshot::Sender<Option<tokio::sync::watch::Receiver<ToolState>>>,
    },
    LoadPipeline {
        path: PathBuf,
        cli: bool,
        reply: CommandResult,
    },
    SavePipeline {
        path: PathBuf,
        reply: CommandResult,
    },
    Load {
        files: Vec<PathBuf>,
        arrangement: ImageArrangement,
        reply: CommandResult,
    },
    /// Show the plane at `coordinates` of the output produced by `load_generation`; requests
    /// made for an earlier load are ignored.
    SelectView {
        coordinates: Coordinates,
        load_generation: u64,
    },
    SaveOutput {
        path: PathBuf,
        scope: ExportScope,
        reply: CommandResult,
    },
    HandleClick(u64, u64),
    RequestOverlays,
    InteractionStart(u64, u64),
    InteractionMove(u64, u64),
    InteractionEnd,
}
