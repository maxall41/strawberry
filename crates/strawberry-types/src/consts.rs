pub const CIRCLE_COLORS: [image::Rgba<u8>; 5] = [
    image::Rgba([51, 187, 238, 255]),
    image::Rgba([238, 119, 51, 255]),
    image::Rgba([204, 51, 17, 255]),
    image::Rgba([238, 51, 119, 255]),
    image::Rgba([38, 255, 0, 255]),
];
#[must_use]
pub fn image_rgba_to_gpui_rgba(rgba: image::Rgba<u8>) -> gpui::Rgba {
    let [r, g, b, a] = rgba.0;
    gpui::Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: f32::from(a) / 255.0,
    }
}
