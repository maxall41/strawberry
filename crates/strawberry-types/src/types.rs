use core::fmt;

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
}

impl fmt::Display for BackendToolType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
