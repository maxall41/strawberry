#![recursion_limit = "256"]

mod common;

use anyhow::Result;
use common::{BACKGROUND, bench_fallible, create_cell_image, run_benchmark_group};
use futures::executor::block_on;
use image::{GrayImage, Luma, Rgba, RgbaImage};
use std::sync::Arc;
use strawberry_processing::{
    GpuImageProcessor,
    compositor::GpuCompositor,
    contrast::GpuContrast,
    decomposer::GpuDecomposer,
    difference_of_gaussians::GpuDifferenceOfGaussians,
    gaussian_blur::GpuGaussianBlur,
    grayscale::GpuGrayscale,
    local_maxima::{GpuLocalMaxima, ThresholdType},
    morphology::{GpuMorphology, MorphologyOperation},
    rolling_ball::GpuRollingBall,
    sobel::GpuSobel,
    threshold::{AutoThresholdMethod, GpuThreshold},
    watershed::GpuWatershed,
};

const IMAGE_SIZE: u32 = 2048;

/// Render a region map as white cells on black, like the output of the threshold tool.
fn create_cell_mask(regions: &GrayImage) -> RgbaImage {
    RgbaImage::from_fn(regions.width(), regions.height(), |x, y| {
        let value = if *regions.get_pixel(x, y) == Luma([BACKGROUND]) {
            0
        } else {
            u8::MAX
        };
        Rgba([value, value, value, u8::MAX])
    })
}

/// Combine three independent cell images into the channels of one color image.
fn create_color_image() -> RgbaImage {
    let channels = [1, 2, 3].map(|seed| create_cell_image(IMAGE_SIZE, seed).0);
    RgbaImage::from_fn(IMAGE_SIZE, IMAGE_SIZE, |x, y| {
        let [blue, green, red] = channels
            .each_ref()
            .map(|channel| channel.get_pixel(x, y)[0]);
        Rgba([blue, green, red, u8::MAX])
    })
}

/// Build every tool and its inputs, then time one application of each tool per iteration.
fn main() -> Result<()> {
    let gpu = Arc::new(block_on(GpuImageProcessor::new())?);
    let (cells, regions) = create_cell_image(IMAGE_SIZE, 0);
    let mask = create_cell_mask(&regions);
    let color = create_color_image();
    let compositor = block_on(GpuCompositor::new(Arc::clone(&gpu)))?;
    let contrast = block_on(GpuContrast::new(Arc::clone(&gpu)))?;
    let decomposer = block_on(GpuDecomposer::new(Arc::clone(&gpu)))?;
    let difference_of_gaussians = block_on(GpuDifferenceOfGaussians::new(Arc::clone(&gpu)))?;
    let gaussian_blur = block_on(GpuGaussianBlur::new(Arc::clone(&gpu)))?;
    let grayscale = block_on(GpuGrayscale::new(Arc::clone(&gpu)))?;
    let local_maxima = block_on(GpuLocalMaxima::new(Arc::clone(&gpu)))?;
    let morphology = block_on(GpuMorphology::new(Arc::clone(&gpu)))?;
    let rolling_ball = block_on(GpuRollingBall::new(Arc::clone(&gpu)))?;
    let sobel = block_on(GpuSobel::new(Arc::clone(&gpu)))?;
    let threshold = block_on(GpuThreshold::new(Arc::clone(&gpu)))?;
    let watershed = block_on(GpuWatershed::new(gpu))?;
    let layers = block_on(decomposer.apply_decomposition(&color))?;
    run_benchmark_group("gpu_tools", |group| {
        bench_fallible(group, "compositor", || {
            block_on(compositor.composite_images(&layers))
        })?;
        bench_fallible(group, "contrast", || {
            block_on(contrast.apply_contrast(&cells, 0.0, 1.0))
        })?;
        bench_fallible(group, "decomposer", || {
            block_on(decomposer.apply_decomposition(&color))
        })?;
        bench_fallible(group, "difference_of_gaussians", || {
            block_on(difference_of_gaussians.apply(&cells, 1.0, 2.0))
        })?;
        bench_fallible(group, "gaussian_blur", || {
            block_on(gaussian_blur.apply_blur(&cells, 2.0))
        })?;
        bench_fallible(group, "grayscale", || {
            block_on(grayscale.apply_grayscale(&color))
        })?;
        bench_fallible(group, "local_maxima", || {
            block_on(local_maxima.apply_local_maxima(&cells, 5, ThresholdType::Absolute(0.0), true))
        })?;
        bench_fallible(group, "morphology_dilation", || {
            block_on(morphology.apply_morphology(&mask, MorphologyOperation::Dilation, 3))
        })?;
        bench_fallible(group, "sobel", || block_on(sobel.apply_sobel(&cells, None)))?;
        bench_fallible(group, "threshold", || {
            block_on(threshold.apply_threshold(&cells, 128))
        })?;
        bench_fallible(group, "threshold_otsu", || {
            block_on(threshold.apply_auto_threshold(&cells, AutoThresholdMethod::Otsu))
        })?;
        // Rolling ball and watershed are far slower than the other tools; hence, we use fewer samples.
        group.sample_size(10);
        bench_fallible(group, "watershed", || {
            block_on(watershed.apply_watershed(&mask, 1.0, 10))
        })?;
        bench_fallible(group, "rolling_ball", || {
            block_on(rolling_ball.apply_rolling_ball(&cells, 50))
        })
    })
}
