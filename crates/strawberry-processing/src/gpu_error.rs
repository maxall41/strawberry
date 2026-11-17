use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

/// The GPU stage or error category that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuErrorKind {
    Validation,
    OutOfMemory,
    Internal,
    DeviceLost,
    Polling,
    Readback,
}

/// A GPU failure and the operation that encountered it.
#[derive(Debug, Clone)]
pub struct GpuError {
    pub kind: GpuErrorKind,
    pub operation: String,
    pub diagnostic: String,
}

impl GpuError {
    /// Construct a GPU error with its category, operation, and diagnostic.
    pub(crate) fn new(
        kind: GpuErrorKind,
        operation: impl Into<String>,
        diagnostic: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation: operation.into(),
            diagnostic: diagnostic.into(),
        }
    }

    /// Translate a wgpu error into the corresponding operation-specific GPU error.
    pub(crate) fn from_wgpu(operation: &str, error: &wgpu::Error) -> Self {
        let kind = match error {
            wgpu::Error::Validation { .. } => GpuErrorKind::Validation,
            wgpu::Error::OutOfMemory { .. } => GpuErrorKind::OutOfMemory,
            wgpu::Error::Internal { .. } => GpuErrorKind::Internal,
        };
        Self::new(kind, operation, error.to_string())
    }
}

impl fmt::Display for GpuError {
    /// Format the failed operation, error category, and diagnostic for display.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({:?}): {}",
            self.operation, self.kind, self.diagnostic
        )
    }
}

impl Error for GpuError {}

struct DeviceLoss {
    error: GpuError,
    pending_notification: bool,
}

pub struct GpuErrors {
    lost: Arc<Mutex<Option<DeviceLoss>>>,
    receiver: flume::Receiver<GpuError>,
}

impl GpuErrors {
    /// Install callbacks that collect uncaptured errors and remember device loss.
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let (sender, receiver) = flume::unbounded();
        let lost = Arc::new(Mutex::new(None));
        device.on_uncaptured_error(Arc::new(move |error| {
            // An unbounded channel never waits for consumer capacity. During shutdown
            // the receiver may already be gone; the callback must not panic then.
            let _ = sender.send(GpuError::from_wgpu("Uncaptured GPU error", &error));
        }));
        let device_lost = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            let mut state = device_lost.lock().unwrap_or_else(PoisonError::into_inner);
            if state.is_none() {
                *state = Some(DeviceLoss {
                    error: GpuError::new(
                        GpuErrorKind::DeviceLost,
                        "GPU device lost; restart Strawberry to use the GPU again",
                        format!("{reason:?}: {message}"),
                    ),
                    pending_notification: true,
                });
            }
        });
        Self { lost, receiver }
    }

    /// Return any recorded device-loss error and consume its pending notification.
    pub(crate) fn check_device_availability(&self, operation: &str) -> anyhow::Result<()> {
        let lost = {
            let mut state = self.lost.lock().unwrap_or_else(PoisonError::into_inner);
            state.as_mut().map(|loss| {
                // This operation returns the failure to its caller; an idle device-loss
                // notification would report the same failure again.
                loss.pending_notification = false;
                loss.error.clone()
            })
        };
        if let Some(mut error) = lost {
            error.operation = format!("{operation}: {}", error.operation);
            return Err(error.into());
        }
        Ok(())
    }

    /// Retrieve the next uncaptured error or unreported device-loss notification.
    pub(crate) fn next(&self) -> Option<GpuError> {
        if let Ok(error) = self.receiver.try_recv() {
            return Some(error);
        }
        let mut state = self.lost.lock().unwrap_or_else(PoisonError::into_inner);
        let notification = state
            .as_mut()
            .filter(|loss| loss.pending_notification)
            .map(|loss| {
                loss.pending_notification = false;
                loss.error.clone()
            });
        drop(state);
        notification
    }
}
