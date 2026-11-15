use image::RgbaImage;
use strawberry_types::{BgraColor, RgbColor};

pub(crate) fn render_class_prediction(
    planes: &[image::GrayImage],
    colors: &[BgraColor],
) -> RgbaImage {
    let (width, height) = planes[0].dimensions();
    RgbaImage::from_fn(width, height, |x, y| {
        let mut winner = 0;
        for index in 1..planes.len() {
            if planes[index].get_pixel(x, y)[0] > planes[winner].get_pixel(x, y)[0] {
                winner = index;
            }
        }
        colors[winner].into_pixel()
    })
}

pub(crate) fn render_class_mask(
    planes: &[image::GrayImage],
    selected_plane: Option<usize>,
    threshold: Option<f32>,
) -> RgbaImage {
    let (width, height) = planes[0].dimensions();
    let plane = selected_plane.and_then(|index| planes.get(index));
    RgbaImage::from_fn(width, height, |x, y| {
        let probability = plane.map_or(0, |plane| plane.get_pixel(x, y)[0]);
        let value = if let Some(threshold) = threshold {
            if plane.is_some() && f32::from(probability) / 255. >= threshold {
                255
            } else {
                0
            }
        } else {
            probability
        };
        RgbColor::new(value, value, value)
            .with_alpha(255)
            .to_bgra()
            .into_pixel()
    })
}
