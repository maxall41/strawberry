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
    Manager,
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
