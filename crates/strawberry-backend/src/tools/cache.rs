use strawberry_types::{
    CompositorState, ContrastState, CounterState, DecomposerState, DifferenceOfGaussiansState,
    GaussianBlurState, GrayscaleState, LocalMaximaState, MorphologyState, NerveTracerState,
    ObjectSizeFilterState, RollingBallState, SobelState, ThresholdState, WatershedState,
};

pub trait SyncState: Clone + PartialEq {
    // Check if enabled
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool;
    // Check if processing specific data mismatches. Does not include UI data in check.
    /// Return whether settings affecting image processing differ.
    fn processing_differs(&self, other: &Self) -> bool {
        self != other
    }
}

macro_rules! processing_states {
    ($($state:ty),* $(,)?) => {
        $(
            impl SyncState for $state {
                /// Return whether this tool is enabled.
                fn is_enabled(&self) -> bool { self.is_enabled }
            }
        )*
    };
}
processing_states!(
    ContrastState,
    DifferenceOfGaussiansState,
    GaussianBlurState,
    GrayscaleState,
    LocalMaximaState,
    MorphologyState,
    ObjectSizeFilterState,
    RollingBallState,
    SobelState,
    ThresholdState,
    DecomposerState,
    WatershedState
);

impl SyncState for CompositorState {
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    /// Return whether settings affecting image processing differ.
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled || self.enabled_layers != other.enabled_layers
    }
}
impl SyncState for CounterState {
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    /// Return whether settings affecting image processing differ.
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled || self.mode != other.mode
    }
}
impl SyncState for NerveTracerState {
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    /// Return whether settings affecting image processing differ.
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled
            || self.selected_image_index != other.selected_image_index
    }
}

/// Frontend delivery never determines whether processing is out of date.
pub struct ToolSync<TState> {
    reconciled: TState,
}

impl<TState: SyncState> ToolSync<TState> {
    /// Initialize synchronization or caching from the supplied settings.
    #[must_use]
    pub fn new(state: &TState) -> Self {
        Self {
            reconciled: state.clone(),
        }
    }

    /// Returns (invalidate this cache, rerun the pipeline).
    pub(crate) fn reconcile(&mut self, state: &TState) -> (bool, bool) {
        let changed = self.reconciled.processing_differs(state);
        let process = changed && (self.reconciled.is_enabled() || state.is_enabled());
        self.acknowledge(state);
        (changed, process)
    }

    /// Record the latest reconciled settings without changing cached images.
    pub fn acknowledge(&mut self, state: &TState) {
        if self.reconciled != *state {
            self.reconciled = state.clone();
        }
    }
}

pub struct ToolCache<TState> {
    pub cached_output: Option<Vec<std::sync::Arc<image::RgbaImage>>>,
    pub cached_state: Option<TState>,
    pub sync: ToolSync<TState>,
}

impl<TState: SyncState> ToolCache<TState> {
    /// Initialize synchronization or caching from the supplied settings.
    #[must_use]
    pub fn new(state: &TState) -> Self {
        Self {
            cached_output: None,
            cached_state: None,
            sync: ToolSync::new(state),
        }
    }

    /// Invalidate stale cached output and report whether processing is needed.
    pub fn reconcile(&mut self, state: &TState) -> bool {
        let (invalidate, process) = self.sync.reconcile(state);
        if invalidate {
            self.invalidate();
        }
        process
    }

    /// Return cached images when the processing settings still match.
    #[must_use]
    pub fn valid_output(&self, state: &TState) -> Option<Vec<std::sync::Arc<image::RgbaImage>>> {
        self.cached_state
            .as_ref()
            .filter(|cached| !cached.processing_differs(state))
            .and_then(|_| self.cached_output.clone())
    }

    /// Clear cached output and its associated settings.
    pub fn invalidate(&mut self) {
        self.cached_output = None;
        self.cached_state = None;
    }
}

impl SyncState for strawberry_types::AutomaticSegmentationState {
    /// Return whether this tool is enabled.
    fn is_enabled(&self) -> bool {
        self.is_enabled
    }
    /// Return whether settings affecting image processing differ.
    fn processing_differs(&self, other: &Self) -> bool {
        self.is_enabled != other.is_enabled
            || self.training != other.training
            || (self.training && self.show_overlay != other.show_overlay)
            || (!self.training
                && (self.use_threshold != other.use_threshold
                    || (self.use_threshold
                        && self.threshold_value.to_bits() != other.threshold_value.to_bits())))
            || self.selected_layer != other.selected_layer
            || self.classes.len() != other.classes.len()
            || self.classes.iter().any(|(id, class)| {
                other
                    .classes
                    .get(id)
                    .is_none_or(|other| class.color != other.color)
            })
            || self.output_mode != other.output_mode
            || self.output_class != other.output_class
            || self.model_revision != other.model_revision
    }
}
