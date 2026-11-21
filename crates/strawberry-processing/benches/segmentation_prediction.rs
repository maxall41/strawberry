#![recursion_limit = "256"]

mod common;

use anyhow::Result;
use common::{bench_fallible, create_cell_image, run_benchmark_group};
use criterion::{BenchmarkGroup, SamplingMode, measurement::WallTime};
use futures::executor::block_on;
use image::{GrayImage, Luma, RgbaImage};
use std::{
    borrow::Cow,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::Duration,
};
use strawberry_processing::{
    GpuImageProcessor,
    segmentation::{
        FeatureConfig, FeatureExtractor, ForestConfig, InferenceConfig, SegmentationModel,
    },
};

const LABEL_STRIDE: u32 = 8;
const TRAINING_SIZE: u32 = 1024;
// Predicted images use sequentially incremented seeds such that the model is not run on its own training image.
const ROOT_SEED: u64 = 0;

/// A prediction workload: the model to run and the images it predicts together.
struct Workload<'a> {
    name: &'static str,
    model: &'a SegmentationModel,
    images: Vec<Arc<RgbaImage>>,
}

impl Workload<'_> {
    /// Predict class planes for every image in one call, so their tiles share the parallel
    /// limit.
    fn predict(
        &self,
        extractor: &FeatureExtractor,
        inference: &InferenceConfig,
    ) -> Result<Vec<Vec<GrayImage>>> {
        block_on(self.model.predict(extractor, &self.images, inference))
    }
}

/// Label a regular grid of pixels in the first training image with their true regions.
fn create_training_labels(regions: &GrayImage) -> HashMap<(usize, u32, u32), u8> {
    regions
        .enumerate_pixels()
        .filter(|&(x, y, _)| x % LABEL_STRIDE == 0 && y % LABEL_STRIDE == 0)
        .map(|(x, y, &Luma([region]))| ((0, x, y), region))
        .collect()
}

/// Hash every predicted plane so outputs can be compared across implementations.
fn hash_planes(outputs: &[Vec<GrayImage>]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for plane in outputs.iter().flatten() {
        plane.as_raw().hash(&mut hasher);
    }
    hasher.finish()
}

/// Train a model on one labeled synthetic image, with or without auto-context.
fn train_model(
    extractor: &FeatureExtractor,
    inference: &InferenceConfig,
    autocontext: bool,
) -> Result<SegmentationModel> {
    let (image, regions) = create_cell_image(TRAINING_SIZE, ROOT_SEED);
    block_on(SegmentationModel::train(
        FeatureConfig::default(),
        ForestConfig {
            autocontext,
            ..ForestConfig::default()
        },
        extractor,
        &[Cow::Owned(image)],
        &create_training_labels(&regions),
        inference,
    ))
}

/// Print a hash of one workload's predictions, then time predicting them.
fn bench_workload(
    group: &mut BenchmarkGroup<'_, WallTime>,
    extractor: &FeatureExtractor,
    inference: &InferenceConfig,
    workload: &Workload<'_>,
) -> Result<()> {
    let outputs = workload.predict(extractor, inference)?;
    println!(
        "{}: output hash {:016x}",
        workload.name,
        hash_planes(&outputs)
    );
    bench_fallible(group, workload.name, || {
        workload.predict(extractor, inference)
    })
}

/// Train the models once, then time prediction on synthetic images with default inference
/// settings.
fn main() -> Result<()> {
    let gpu = Arc::new(block_on(GpuImageProcessor::new())?);
    let extractor = block_on(FeatureExtractor::new(gpu))?;
    let inference = InferenceConfig::default();
    let standard = train_model(&extractor, &inference, false)?;
    let autocontext = train_model(&extractor, &inference, true)?;
    let create_images = |count: u64, size: u32| {
        (1..=count)
            .map(|offset| Arc::new(create_cell_image(size, ROOT_SEED.wrapping_add(offset)).0))
            .collect()
    };
    let workloads = [
        // Nine tiles: one more than the default parallel limit.
        Workload {
            name: "one_image_1536px",
            model: &standard,
            images: create_images(1, 1536),
        },
        // Four tiles per image, fewer than the default parallel limit.
        Workload {
            name: "four_images_1024px",
            model: &standard,
            images: create_images(4, 1024),
        },
        Workload {
            name: "four_images_1024px_autocontext",
            model: &autocontext,
            images: create_images(4, 1024),
        },
    ];
    run_benchmark_group("segmentation_prediction", |group| {
        // Each iteration takes seconds, so time single iterations rather than growing batches.
        group
            .sampling_mode(SamplingMode::Flat)
            .sample_size(10)
            .measurement_time(Duration::from_secs(30));
        workloads
            .iter()
            .try_for_each(|workload| bench_workload(group, &extractor, &inference, workload))
    })
}
