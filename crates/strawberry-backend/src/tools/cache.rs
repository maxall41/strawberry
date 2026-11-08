use strawberry_types::{
    CompositorState, ContrastState, CounterState, DecomposerState, GaussianBlurState,
    GrayscaleState, LocalMaximaState, MorphologyState, NerveTracerState, RollingBallState,
    SobelState, ThresholdState, WatershedState,
};

/// Which state changes affect processing, independently of frontend presentation.
pub trait ProcessingState: Clone + PartialEq {
    fn is_enabled(&self) -> bool;
    fn processing_differs(&self, other: &Self) -> bool {
        self != other
    }
}

macro_rules! processing_states {
    ($($state:ty),* $(,)?) => {
        $(
            impl ProcessingState for $state {
                fn is_enabled(&self) -> bool { self.is_enabled }
            }
        )*
    };
}
processing_states!(
    ContrastState,
    GaussianBlurState,
    GrayscaleState,
    LocalMaximaState,
    MorphologyState,
    RollingBallState,
    SobelState,
    ThresholdState,
    DecomposerState,
    WatershedState
);

impl ProcessingState for CompositorState {
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled || self.enabled_layers != other.enabled_layers
    }
}
impl ProcessingState for CounterState {
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled
            || self.mode != other.mode
            || self.filter_proxys != other.filter_proxys
    }
}
impl ProcessingState for NerveTracerState {
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled
            || self.selected_image_index != other.selected_image_index
    }
}

/// Frontend delivery never determines whether processing is out of date.
pub struct ToolSync<TState> {
    reconciled: TState,
    pub frontend_latest_state: Option<TState>,
}

impl<TState: ProcessingState> ToolSync<TState> {
    #[must_use]
    pub fn new(state: &TState) -> Self {
        Self {
            reconciled: state.clone(),
            frontend_latest_state: None,
        }
    }

    /// Returns (invalidate this cache, rerun the pipeline).
    fn reconcile(&mut self, state: &TState) -> (bool, bool) {
        let changed = self.reconciled.processing_differs(state);
        let process = changed && (self.reconciled.is_enabled() || state.is_enabled());
        self.acknowledge(state);
        (changed, process)
    }

    pub fn acknowledge(&mut self, state: &TState) {
        if self.reconciled != *state {
            self.reconciled = state.clone();
        }
    }

    #[must_use]
    pub fn needs_frontend_sync(&self, state: &TState) -> bool {
        self.frontend_latest_state.as_ref() != Some(state)
    }
}

pub struct ToolCache<TState> {
    pub cached_output: Option<Vec<std::sync::Arc<image::RgbaImage>>>,
    pub cached_state: Option<TState>,
    pub sync: ToolSync<TState>,
}

impl<TState: ProcessingState> ToolCache<TState> {
    #[must_use]
    pub fn new(state: &TState) -> Self {
        Self {
            cached_output: None,
            cached_state: None,
            sync: ToolSync::new(state),
        }
    }

    pub fn reconcile(&mut self, state: &TState) -> bool {
        let (invalidate, process) = self.sync.reconcile(state);
        if invalidate {
            self.invalidate();
        }
        process
    }

    #[must_use]
    pub fn valid_output(&self, state: &TState) -> Option<Vec<std::sync::Arc<image::RgbaImage>>> {
        self.cached_state
            .as_ref()
            .filter(|cached| !cached.processing_differs(state))
            .and_then(|_| self.cached_output.clone())
    }

    pub fn invalidate(&mut self) {
        self.cached_output = None;
        self.cached_state = None;
    }
}
