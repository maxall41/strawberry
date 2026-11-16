use core::fmt;

/// Coalesces processing requests at the earliest requested tool index.
#[derive(Default)]
pub struct PendingProcessing {
    from: Option<usize>,
}

impl PendingProcessing {
    /// Schedule processing without skipping any earlier pending work.
    pub fn request_processing(&mut self, from: usize) {
        self.from = Some(self.from.map_or(from, |index| index.min(from)));
    }

    /// Consume the pending request at a processing boundary.
    pub const fn take(&mut self) -> Option<usize> {
        self.from.take()
    }
}

pub struct ReconcilliationTicket {
    pub index: Option<usize>,
}

pub struct ValidReconcilliationTicket {
    index: usize,
}

impl ReconcilliationTicket {
    #[must_use]
    pub fn validate(&self) -> Option<ValidReconcilliationTicket> {
        self.index.map(|index| ValidReconcilliationTicket { index })
    }
}

impl ValidReconcilliationTicket {
    #[must_use]
    pub const fn take(&self) -> usize {
        self.index
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendToolType {
    AutomaticSegmentation,
    Compositor,
    Counter,
    Contrast,
    DifferenceOfGaussians,
    GaussianBlur,
    Grayscale,
    LocalMaxima,
    Morphology,
    ObjectSizeFilter,
    RollingBall,
    Sobel,
    Threshold,
    Watershed,
    Decomposer,
    NerveTracer,
}

impl fmt::Display for BackendToolType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Stable identity within a pipeline session. Runtime IDs are never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSummary {
    pub id: ToolId,
    pub kind: BackendToolType,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineState {
    pub tools: Vec<ToolSummary>,
    pub selected_tool: Option<ToolId>,
}

#[derive(Debug, Clone)]
pub enum ToolInput {
    Message(crate::PipelineObjectMessage),
    HandleClick(u64, u64),
    InteractionStart(u64, u64),
    InteractionMove(u64, u64),
    InteractionEnd,
}
