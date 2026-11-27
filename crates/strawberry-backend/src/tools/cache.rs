use std::{
    ptr,
    sync::{Arc, Weak},
};

use strawberry_types::{
    AutomaticSegmentationState, ClassId, CompositorState, ContrastState, CounterMode, CounterState,
    DecomposerState, DifferenceOfGaussiansState, GaussianBlurState, GrayscaleState,
    ImageCollection, LocalMaximaState, MorphologyState, NerveTracerState, ObjectSizeFilterState,
    Plane, RgbColor, RollingBallState, SegmentationOutputMode, Shape, SobelState, ThresholdState,
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
    DecomposerState,
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

    /// Capture the channel selection used for composition, leaving out the runtime channel count.
    fn capture_settings(&self) -> Self::Settings {
        self.enabled_channels.clone()
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
        self.selected_channel
    }
}

struct CachedRun<TSettings> {
    settings: TSettings,
    input_shape: Shape,
    // Weak references preserve plane identity without retaining the input pixels.
    input_planes: Vec<Weak<Plane>>,
    output: ImageCollection,
}

/// Check that the collection still has the same shape and planes at each coordinate.
fn matches_input(
    input: &ImageCollection,
    shape: Shape,
    planes: impl Iterator<Item = *const Plane>,
) -> bool {
    input.get_shape() == shape
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
            !matches_input(
                input,
                previous.get_shape(),
                previous.planes().map(Arc::as_ptr),
            )
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
            input_shape: input.get_shape(),
            input_planes: input.planes().map(Arc::downgrade).collect(),
            output,
        });
    }

    /// Clear the last run's reusable output while keeping input for canvas interaction.
    pub fn invalidate(&mut self) {
        self.cached_run = None;
    }

    /// Release the cached output if a plane it was computed from no longer exists, returning
    /// whether it was released. Reuse needs the very same input planes, so such output can
    /// never be returned again.
    pub fn release_unreachable_output(&mut self) -> bool {
        let unreachable = self.cached_run.as_ref().is_some_and(|cached| {
            cached
                .input_planes
                .iter()
                .any(|plane| plane.strong_count() == 0)
        });
        if unreachable {
            self.invalidate();
        }
        unreachable
    }

    /// Return the collections kept between runs: the cached output and the remembered input.
    #[must_use]
    pub fn get_retained_images(&self) -> Vec<&ImageCollection> {
        self.cached_run
            .iter()
            .map(|cached| &cached.output)
            .chain(&self.cached_input)
            .collect()
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
    selected_channel: usize,
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
            selected_channel: self.selected_channel,
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
            || self.selected_channel != saved.selected_channel
            || self.output_mode != saved.output_mode
            || self.output_class != saved.output_class
            || self.model_revision != saved.model_revision
            || get_class_colors(self).ne(saved.class_colors.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::ToolCache;
    use image::{Rgba, RgbaImage};
    use std::sync::Arc;
    use strawberry_types::{
        ImageArrangement, ImageCollection, Offload, Plane, SobelState, SpillFile,
    };

    fn make_stack(depth: usize) -> ImageCollection {
        let planes = (0..depth)
            .map(|_| Plane::new(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]))))
            .collect();
        ImageCollection::arrange(planes, &ImageArrangement::Stack).unwrap()
    }

    fn make_state() -> SobelState {
        SobelState {
            is_enabled: true,
            threshold_value: 0.1,
            use_threshold: false,
        }
    }

    #[test]
    fn cached_output_survives_offloading_its_input_and_output() {
        let state = make_state();
        let spill = SpillFile::new(tempfile::tempfile().unwrap());
        let input = make_stack(2);
        let output = make_stack(2);
        let mut cache = ToolCache::new(&state);
        cache.store_output(&state, &input, output.clone());

        for plane in input.planes().chain(output.planes()) {
            assert!(matches!(
                plane.offload(&spill).unwrap(),
                Offload::Released(_)
            ));
        }
        let restored = cache.valid_output(&state, &input).unwrap();
        assert!(
            restored
                .planes()
                .zip(output.planes())
                .all(|(restored, stored)| Arc::ptr_eq(restored, stored))
        );

        // Reading the pixels back does not make them different planes either.
        for plane in input.planes() {
            plane.load().unwrap();
        }
        assert!(cache.valid_output(&state, &input).is_some());
    }

    #[test]
    fn output_computed_from_planes_that_are_gone_is_released_down_the_chain() {
        let state = make_state();
        let input = make_stack(1);
        let first_output = make_stack(1);
        let second_output = make_stack(1);
        let mut first = ToolCache::new(&state);
        let mut second = ToolCache::new(&state);
        let mut third = ToolCache::new(&state);
        first.store_output(&state, &input, first_output.clone());
        second.store_output(&state, &first_output, second_output.clone());
        third.store_output(&state, &second_output, make_stack(1));
        // From here only the caches hold the intermediate planes, as in the pipeline.
        drop((first_output, second_output));
        assert!(!first.release_unreachable_output());
        assert!(!second.release_unreachable_output());
        assert!(!third.release_unreachable_output());

        // The first stage recomputes and drops its output, which the second was computed
        // from; releasing the second strands the third in turn.
        first.invalidate_all();
        assert!(!third.release_unreachable_output());
        assert!(second.release_unreachable_output());
        assert!(third.release_unreachable_output());
        assert!(third.get_retained_images().is_empty());
        assert!(!third.release_unreachable_output());

        // Output computed from planes that still exist stays until they are gone.
        first.store_output(&state, &input, make_stack(1));
        assert!(!first.release_unreachable_output());
        drop(input);
        assert!(first.release_unreachable_output());
    }

    #[test]
    fn a_remembered_input_keeps_its_cache_reachable() {
        let state = make_state();
        let upstream = make_stack(1);
        let mut cache = ToolCache::new(&state);
        cache.remember_input(&upstream);
        cache.store_output(&state, &upstream, make_stack(1));
        drop(upstream);

        assert!(!cache.release_unreachable_output());
        assert_eq!(cache.get_retained_images().len(), 2);
    }
}
