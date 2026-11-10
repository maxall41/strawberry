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
    pub fn take(&mut self) -> Option<usize> {
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
    pub fn validate(&self) -> Option<ValidReconcilliationTicket> {
        if let Some(index) = self.index {
            Some(ValidReconcilliationTicket { index: index })
        } else {
            None
        }
    }
}

impl ValidReconcilliationTicket {
    pub fn take(&self) -> usize {
        self.index
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendToolType {
    Compositor,
    Counter,
    Contrast,
    GaussianBlur,
    Grayscale,
    LocalMaxima,
    Manager,
    Morphology,
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
