use image::RgbaImage;
use num_traits::ToPrimitive;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoThresholdMethod {
    #[default]
    Otsu,
    Huang,
    Triangle,
}

impl AutoThresholdMethod {
    /// Select the last background bin; foreground pixels have levels strictly above it.
    /// Empty histograms return zero, and constant histograms return their occupied bin.
    #[must_use]
    pub fn threshold(self, histogram: &[u64; 256]) -> u8 {
        let mut occupied = histogram
            .iter()
            .zip(0..=u8::MAX)
            .filter_map(|(&count, level)| (count > 0).then_some(level));
        let Some(first) = occupied.next() else {
            return 0;
        };
        let last = occupied.next_back().unwrap_or(first);
        if first == last {
            return first;
        }

        let histogram = histogram.map(|count| count.to_f64().unwrap_or(0.0));
        match self {
            Self::Otsu => otsu(&histogram, first, last),
            Self::Huang => huang(&histogram, first, last),
            Self::Triangle => triangle(&histogram, first, last),
        }
    }
}

/// Use the same integer BT.601 luminance in the histogram and automatic GPU comparison.
/// Pipeline images contain BGRA bytes, regardless of the image buffer's Rust type.
pub(super) fn luminance_histogram(image: &RgbaImage) -> [u64; 256] {
    image
        .as_raw()
        .par_chunks(64 * 1024)
        .map(|chunk| {
            let mut histogram = [0_u64; 256];
            for &[blue, green, red, _alpha] in chunk.as_chunks::<4>().0 {
                let level = u32::from(red)
                    .saturating_mul(299)
                    .saturating_add(u32::from(green).saturating_mul(587))
                    .saturating_add(u32::from(blue).saturating_mul(114))
                    / 1000;
                if let Ok(index) = usize::try_from(level)
                    && let Some(count) = histogram.get_mut(index)
                {
                    *count = count.saturating_add(1);
                }
            }
            histogram
        })
        .reduce(
            || [0; 256],
            |mut combined, histogram| {
                for (total, count) in combined.iter_mut().zip(histogram) {
                    *total = total.saturating_add(count);
                }
                combined
            },
        )
}

fn moments(histogram: &[f64; 256]) -> (f64, f64) {
    histogram
        .iter()
        .zip(0..=u8::MAX)
        .fold((0.0, 0.0), |(count, sum), (&frequency, level)| {
            (count + frequency, frequency.mul_add(f64::from(level), sum))
        })
}

fn otsu(histogram: &[f64; 256], first: u8, last: u8) -> u8 {
    // Otsu (1979), equation (14): maximize between-class variance.
    // doi:10.1109/TSMC.1979.4310076. Counts omit the constant factor 1/N².
    // Equation (19) permits any maximizer; retain the first on ties.
    let (total_count, total_sum) = moments(histogram);
    let (mut lower_count, mut lower_sum) = (0.0, 0.0);
    let mut best_variance = f64::NEG_INFINITY;
    let mut threshold = first;
    for (&count, level) in histogram.iter().zip(0..=u8::MAX).take(usize::from(last)) {
        lower_count += count;
        lower_sum = count.mul_add(f64::from(level), lower_sum);
        let upper_count = total_count - lower_count;
        if lower_count <= 0.0 || upper_count <= 0.0 {
            continue;
        }
        let difference = lower_sum / lower_count - (total_sum - lower_sum) / upper_count;
        let variance = lower_count * upper_count * difference.powi(2);
        if variance > best_variance {
            best_variance = variance;
            threshold = level;
        }
    }
    threshold
}

fn huang(histogram: &[f64; 256], first: u8, last: u8) -> u8 {
    // Huang & Wang (1995), entropy variant, equations (4), (8), (20)–(21).
    // doi:10.1016/0031-3203(94)E0043-K. Use the base minimum-entropy method,
    // without the optional, tolerance-dependent valley refinement (22)–(23).
    let (total_count, total_sum) = moments(histogram);
    let range = f64::from(last.saturating_sub(first));
    let (mut lower_count, mut lower_sum) = (0.0, 0.0);
    let mut best_entropy = f64::INFINITY;
    let mut threshold = first;
    for (&count, candidate) in histogram.iter().zip(0..=u8::MAX).take(usize::from(last)) {
        lower_count += count;
        lower_sum = count.mul_add(f64::from(candidate), lower_sum);
        let upper_count = total_count - lower_count;
        if lower_count <= 0.0 || upper_count <= 0.0 {
            continue;
        }
        // The computation procedure rounds both target gray levels to integers.
        let lower_mean = (lower_sum / lower_count).round();
        let upper_mean = ((total_sum - lower_sum) / upper_count).round();
        let entropy = histogram
            .iter()
            .zip(0..=u8::MAX)
            .filter(|(count, _)| **count > 0.0)
            .map(|(&frequency, level)| {
                let mean = if level <= candidate {
                    lower_mean
                } else {
                    upper_mean
                };
                let membership = 1.0 / (1.0 + (f64::from(level) - mean).abs() / range);
                let complement = 1.0 - membership;
                if complement <= 0.0 {
                    0.0
                } else {
                    frequency
                        * (-membership).mul_add(membership.ln(), -complement * complement.ln())
                }
            })
            .sum::<f64>();
        if entropy < best_entropy {
            best_entropy = entropy;
            threshold = candidate;
        }
    }
    threshold
}

fn triangle(histogram: &[f64; 256], first: u8, last: u8) -> u8 {
    // Zack et al. (1977), Figure 2, doi:10.1177/25.7.70454: maximize distance
    // below the mode-to-baseline line.
    let (mut peak, mut peak_count) = (first, 0.0);
    for (&count, level) in histogram.iter().zip(0..=u8::MAX) {
        if count > peak_count {
            peak = level;
            peak_count = count;
        }
    }
    // Include the adjacent empty bin so the line reaches the baseline, where possible.
    // Choosing the longer tail generalizes the construction to either image polarity.
    let left = first.saturating_sub(1);
    let right = last.saturating_add(1);
    let tail = if peak.saturating_sub(left) >= right.saturating_sub(peak) {
        left
    } else {
        right
    };
    let tail_count = histogram.get(usize::from(tail)).copied().unwrap_or(0.0);
    let width = f64::from(tail) - f64::from(peak);
    let height = tail_count - peak_count;
    let direction = width.signum();
    let mut threshold = peak;
    let mut best_distance = 0.0;
    for (&count, level) in histogram.iter().zip(0..=u8::MAX) {
        if level < peak.min(tail) || level > peak.max(tail) {
            continue;
        }
        // The line length is constant, so the signed cross product has the same maximum
        // as perpendicular distance. No histogram normalization or reversal is needed.
        let distance = direction
            * height.mul_add(
                f64::from(level) - f64::from(peak),
                -width * (count - peak_count),
            );
        if distance > best_distance {
            best_distance = distance;
            threshold = level;
        }
    }
    threshold.min(last.saturating_sub(1))
}
