use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use strawberry_types::{BgraColor, RgbColor};

pub fn render_class_prediction(
    planes: &[image::GrayImage],
    colors: &[BgraColor],
) -> Result<RgbaImage> {
    let (width, height) = plane_dimensions(planes)?;
    ensure!(
        planes.len() == colors.len(),
        "Prediction color count mismatch"
    );
    Ok(RgbaImage::from_fn(width, height, |x, y| {
        planes
            .iter()
            .zip(colors)
            .map(|(plane, color)| (plane.get_pixel(x, y)[0], *color))
            .reduce(|winner, candidate| {
                if candidate.0 > winner.0 {
                    candidate
                } else {
                    winner
                }
            })
            .map_or(BgraColor::TRANSPARENT, |(_, color)| color)
            .into_pixel()
    }))
}

pub fn render_class_mask(
    planes: &[image::GrayImage],
    selected_plane: Option<usize>,
    threshold: Option<f32>,
) -> Result<RgbaImage> {
    let (width, height) = plane_dimensions(planes)?;
    let plane = selected_plane.and_then(|index| planes.get(index));
    Ok(RgbaImage::from_fn(width, height, |x, y| {
        let probability = plane.map_or(0, |plane| plane.get_pixel(x, y)[0]);
        let value = threshold.map_or(probability, |threshold| {
            if plane.is_some() && f32::from(probability) / 255. >= threshold {
                255
            } else {
                0
            }
        });
        RgbColor::new(value, value, value)
            .with_alpha(255)
            .to_bgra()
            .into_pixel()
    }))
}

fn plane_dimensions(planes: &[image::GrayImage]) -> Result<(u32, u32)> {
    let dimensions = planes
        .first()
        .context("Missing probability planes")?
        .dimensions();
    ensure!(
        planes.iter().all(|plane| plane.dimensions() == dimensions),
        "Probability plane dimensions differ"
    );
    Ok(dimensions)
}
