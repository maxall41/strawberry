use std::{
    ptr,
    sync::{Arc, Weak},
};

use image::RgbaImage;
use strawberry_types::{
    AutomaticSegmentationState, ClassId, CompositorState, ContrastState, CounterMode, CounterState,
    DecomposerState, DifferenceOfGaussiansState, GaussianBlurState, GrayscaleState,
    ImageCollection, LocalMaximaState, MorphologyState, NerveTracerState, ObjectSizeFilterState,
    RgbColor, RollingBallState, SegmentationOutputMode, Shape, SobelState, ThresholdState,
    WatershedState,
};

pub trait SyncState {
    type Settings: PartialEq;

    /// Snapshot the settings that affect a tool's processing.
    fn capture_settings(&self) -> Self::Settings;

    /// Return whether settings affecting processing differ when the tool is active.
    fn processing_differs(&self, saved: &Self::Settings) -> bool {
        self.capture_settings() != *saved
    }
}

macro_rules! processing_states {
    ($($state:ty),* $(,)?) => {
        $(
            impl SyncState for $state {
                type Settings = Self;

                /// Capture the tool settings independently of pipeline enablement.
                fn capture_settings(&self) -> Self {
                    let mut settings = self.clone();
                    settings.is_enabled = false;
                    settings
                }
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
    WatershedState
);

impl SyncState for CompositorState {
    type Settings = Vec<bool>;

    /// Capture the channel selection used for composition.
    fn capture_settings(&self) -> Self::Settings {
        self.enabled_layers.clone()
    }

    /// Compare the channel selection used for composition.
    fn processing_differs(&self, saved: &Self::Settings) -> bool {
        self.enabled_layers != *saved
    }
}

impl SyncState for DecomposerState {
    type Settings = Vec<bool>;

    /// Capture the channel selection used for decomposition.
    fn capture_settings(&self) -> Self::Settings {
        self.enabled_layers.clone()
    }

    /// Compare the channel selection used for decomposition.
    fn processing_differs(&self, saved: &Self::Settings) -> bool {
        self.enabled_layers != *saved
    }
}

impl SyncState for CounterState {
    type Settings = CounterMode;

    /// Capture the mode that determines whether to detect objects.
    fn capture_settings(&self) -> Self::Settings {
        self.mode
    }
}

impl SyncState for NerveTracerState {
    type Settings = usize;

    /// Capture the channel used for tracing.
    fn capture_settings(&self) -> Self::Settings {
        self.selected_layer
    }
}

struct CachedRun<TSettings> {
    settings: TSettings,
    input_shape: Shape,
    // Weak references preserve plane identity without retaining the input pixels.
    input_planes: Vec<Weak<RgbaImage>>,
    output: ImageCollection,
}

/// Check that the collection still has the same shape and planes at each coordinate.
fn matches_input(
    input: &ImageCollection,
    shape: Shape,
    planes: impl Iterator<Item = *const RgbaImage>,
) -> bool {
    input.shape() == shape
        && planes
            .zip(input.planes())
            .all(|(previous, current)| ptr::eq(previous, Arc::as_ptr(current)))
}

pub struct ToolCache<TState: SyncState> {
    cached_run: Option<CachedRun<TState::Settings>>,
    cached_input: Option<ImageCollection>,
    reconciled: TState::Settings,
}

impl<TState: SyncState> ToolCache<TState> {
    /// Create an empty cache and record the initial settings.
    #[must_use]
    pub fn new(state: &TState) -> Self {
        Self {
            cached_run: None,
            cached_input: None,
            reconciled: state.capture_settings(),
        }
    }

    /// Keep the images a run received so canvas interaction can read them until the next run.
    pub fn remember_input(&mut self, images: &ImageCollection) {
        self.cached_input = Some(images.clone());
    }

    /// Return the images the last run received, if this tool remembers its input.
    #[must_use]
    pub const fn get_input(&self) -> Option<&ImageCollection> {
        self.cached_input.as_ref()
    }

    /// Check whether the input differs from the last run or the remembered canvas input.
    #[must_use]
    pub fn input_differs(&self, input: &ImageCollection) -> bool {
        if let Some(cached) = &self.cached_run {
            return !matches_input(
                input,
                cached.input_shape,
                cached.input_planes.iter().map(Weak::as_ptr),
            );
        }
        self.cached_input.as_ref().is_some_and(|previous| {
            !matches_input(input, previous.shape(), previous.planes().map(Arc::as_ptr))
        })
    }

    /// Record current settings and report whether the processing settings changed.
    pub fn reconcile(&mut self, state: &TState) -> bool {
        if !state.processing_differs(&self.reconciled) {
            return false;
        }
        self.reconciled = state.capture_settings();
        true
    }

    /// Return cached images when the settings, shape, and ordered input planes still match.
    #[must_use]
    pub fn valid_output(&self, state: &TState, input: &ImageCollection) -> Option<ImageCollection> {
        let cached = self.cached_run.as_ref()?;
        if state.processing_differs(&cached.settings) || self.input_differs(input) {
            return None;
        }
        Some(cached.output.clone())
    }

    /// Keep the output of a successful run with the input and settings that produced it.
    pub fn store_output(
        &mut self,
        state: &TState,
        input: &ImageCollection,
        output: ImageCollection,
    ) {
        self.cached_run = Some(CachedRun {
            settings: state.capture_settings(),
            input_shape: input.shape(),
            input_planes: input.planes().map(Arc::downgrade).collect(),
            output,
        });
    }

    /// Clear the last run's reusable output while keeping input for canvas interaction.
    pub fn invalidate(&mut self) {
        self.cached_run = None;
    }

    /// Clear cached output, its settings, and the remembered input.
    pub fn invalidate_all(&mut self) {
        self.invalidate();
        self.cached_input = None;
    }
}

#[derive(PartialEq, Eq)]
pub struct SegmentationSettings {
    training: bool,
    show_overlay: bool,
    threshold: Option<u32>,
    selected_layer: usize,
    class_colors: Vec<(ClassId, RgbColor)>,
    output_mode: SegmentationOutputMode,
    output_class: ClassId,
    model_revision: u64,
}

/// Return the exact threshold used in inference, when thresholding is enabled.
fn get_segmentation_threshold(state: &AutomaticSegmentationState) -> Option<u32> {
    (!state.training && state.use_threshold).then_some(state.threshold_value.to_bits())
}

/// Iterate over the class colors in class ID order.
fn get_class_colors(
    state: &AutomaticSegmentationState,
) -> impl Iterator<Item = (ClassId, RgbColor)> + '_ {
    state.classes.iter().map(|(&id, class)| (id, class.color))
}

impl SyncState for AutomaticSegmentationState {
    type Settings = SegmentationSettings;

    /// Capture the parameters used for the training preview or segmentation output.
    fn capture_settings(&self) -> Self::Settings {
        SegmentationSettings {
            training: self.training,
            show_overlay: self.training && self.show_overlay,
            threshold: get_segmentation_threshold(self),
            selected_layer: self.selected_layer,
            class_colors: get_class_colors(self).collect(),
            output_mode: self.output_mode,
            output_class: self.output_class,
            model_revision: self.model_revision,
        }
    }

    /// Compare processing parameters directly, borrowing class colors from the live state.
    fn processing_differs(&self, saved: &Self::Settings) -> bool {
        self.training != saved.training
            || (self.training && self.show_overlay != saved.show_overlay)
            || get_segmentation_threshold(self) != saved.threshold
            || self.selected_layer != saved.selected_layer
            || self.output_mode != saved.output_mode
            || self.output_class != saved.output_class
            || self.model_revision != saved.model_revision
            || get_class_colors(self).ne(saved.class_colors.iter().copied())
    }
}
