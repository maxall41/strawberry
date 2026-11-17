use image::RgbaImage;
use lutz::Image;

pub struct BinaryImageWrapper<'a> {
    image: &'a RgbaImage,
    threshold: u8,
}

impl<'a> BinaryImageWrapper<'a> {
    /// Construct the tool with its initial settings and required resources.
    pub(crate) const fn new(image: &'a RgbaImage, threshold: u8) -> Self {
        Self { image, threshold }
    }
}

impl Image for BinaryImageWrapper<'_> {
    /// Return the binary image width in pixels.
    fn width(&self) -> u32 {
        self.image.width()
    }

    /// Return the binary image height in pixels.
    fn height(&self) -> u32 {
        self.image.height()
    }

    /// Return whether the given position contains a set pixel.
    fn has_pixel(&self, x: u32, y: u32) -> bool {
        if x >= self.width() || y >= self.height() {
            return false;
        }
        let pixel = self.image.get_pixel(x, y);
        pixel[3] > 0
            && u16::from(pixel[0])
                .saturating_add(u16::from(pixel[1]))
                .saturating_add(u16::from(pixel[2]))
                .checked_div(3)
                .unwrap_or_default()
                > u16::from(self.threshold)
    }
}
