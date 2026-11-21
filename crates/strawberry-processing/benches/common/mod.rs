//! Synthetic images and timing helpers shared by the processing benchmarks.

use anyhow::Result;
use criterion::{BenchmarkGroup, Criterion, measurement::WallTime};
use fastrand::Rng;
use gag::Gag;
use image::{GrayImage, Luma, Rgba, RgbaImage};
use std::time::Instant;

const CELL_SPACING: u32 = 48;
pub const BACKGROUND: u8 = 0;
const MEMBRANE: u8 = 1;
const INTERIOR: u8 = 2;

/// Paint randomly placed cells, each an interior inside a membrane rim, into a map of every
/// pixel's true region.
fn create_regions(size: u32, rng: &mut Rng) -> GrayImage {
    let mut regions = GrayImage::from_pixel(size, size, Luma([BACKGROUND]));
    let last = size.saturating_sub(1);
    for _ in 0..(size / CELL_SPACING).pow(2) {
        let (center_x, center_y) = (rng.u32(..size), rng.u32(..size));
        let radius = rng.u32(10..=16);
        let reach = radius.saturating_add(2);
        for y in center_y.saturating_sub(reach)..=center_y.saturating_add(reach).min(last) {
            for x in center_x.saturating_sub(reach)..=center_x.saturating_add(reach).min(last) {
                let distance = x
                    .abs_diff(center_x)
                    .pow(2)
                    .saturating_add(y.abs_diff(center_y).pow(2));
                let region = if distance < radius.saturating_sub(2).pow(2) {
                    INTERIOR
                } else if distance < reach.pow(2) {
                    MEMBRANE
                } else {
                    continue;
                };
                regions.put_pixel(x, y, Luma([region]));
            }
        }
    }
    regions
}

/// Paint a noisy grayscale image of bright-rimmed cells on a dark background, returning it with
/// its region map.
pub fn create_cell_image(size: u32, seed: u64) -> (RgbaImage, GrayImage) {
    let mut rng = Rng::with_seed(seed);
    let regions = create_regions(size, &mut rng);
    let image = RgbaImage::from_fn(size, size, |x, y| {
        // Noise wider than the gaps between region intensities forces segmentation forests to
        // grow deep trees on the filtered features, as on real images.
        let base: u8 = match *regions.get_pixel(x, y) {
            Luma([INTERIOR]) => 110,
            Luma([MEMBRANE]) => 160,
            _ => 70,
        };
        let value = base.saturating_add(rng.u8(..128)).saturating_sub(64);
        Rgba([value, value, value, u8::MAX])
    });
    (image, regions)
}

/// Time a fallible routine with its standard output silenced, returning its first error instead
/// of reporting its time.
pub fn bench_fallible<T>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    mut routine: impl FnMut() -> Result<T>,
) -> Result<()> {
    let mut failure = None;
    group.bench_function(name, |bencher| {
        bencher.iter_custom(|iterations| {
            // Tools log every call. Criterion prints only between batches, so silencing each
            // batch hides that log without hiding criterion's output or timing the redirect.
            let _silenced = Gag::stdout().ok();
            let start = Instant::now();
            for _ in 0..iterations {
                if let Err(error) = routine() {
                    failure.get_or_insert(error);
                }
            }
            start.elapsed()
        });
    });
    failure.map_or(Ok(()), Err)
}

/// Run benchmarks in one group configured from the command line, then print the run summary.
pub fn run_benchmark_group(
    name: &str,
    run: impl FnOnce(&mut BenchmarkGroup<'_, WallTime>) -> Result<()>,
) -> Result<()> {
    let mut criterion = Criterion::default().configure_from_args();
    // Dropping the group finishes it before the summary.
    let result = run(&mut criterion.benchmark_group(name));
    criterion.final_summary();
    result
}
