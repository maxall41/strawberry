use image::RgbaImage;
use std::path::PathBuf;
use std::sync::Arc;
use strawberry_processing::local_maxima::ThresholdType;
use strawberry_processing::segmentation::{FeatureConfig, ForestConfig, InferenceConfig};
use strawberry_processing::threshold::AutoThresholdMethod;

use super::{
    AnnotationKind, BackendToolType, BrushRadius, ClassId, DatasetImportMode, ObjectSizeFilterMode,
    RgbColor, SegmentationOutputMode, StrokeId, ToolState,
};

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
    SetThresholdValue(f32),
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
    SelectLayer(usize),
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
    /// 0-based index of the stage about to run (display code adds 1).
    pub stage: usize,
    /// Number of stages this run will execute in total.
    pub total: usize,
    /// Display name
    pub name: String,
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
        reply: CommandResult,
    },
    SaveOutput {
        path: PathBuf,
        reply: CommandResult,
    },
    HandleClick(u64, u64),
    RequestOverlays,
    InteractionStart(u64, u64),
    InteractionMove(u64, u64),
    InteractionEnd,
}
