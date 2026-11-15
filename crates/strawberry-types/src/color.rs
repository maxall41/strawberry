use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl RgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub const fn with_alpha(self, alpha: u8) -> RgbaColor {
        RgbaColor {
            red: self.red,
            green: self.green,
            blue: self.blue,
            alpha,
        }
    }

    pub const fn hex(self) -> u32 {
        u32::from_be_bytes([0, self.red, self.green, self.blue])
    }

    pub const fn from_hex(hex: u32) -> Self {
        let [_, red, green, blue] = hex.to_be_bytes();
        Self::new(red, green, blue)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbaColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl RgbaColor {
    pub const WHITE: Self = Self::new(255, 255, 255, 255);

    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub const fn to_bgra(self) -> BgraColor {
        BgraColor {
            blue: self.blue,
            green: self.green,
            red: self.red,
            alpha: self.alpha,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BgraColor {
    pub blue: u8,
    pub green: u8,
    pub red: u8,
    pub alpha: u8,
}

impl BgraColor {
    pub const TRANSPARENT: Self = Self {
        blue: 0,
        green: 0,
        red: 0,
        alpha: 0,
    };

    /// The image buffers used by the viewer store BGRA bytes in image::Rgba pixels.
    pub const fn into_pixel(self) -> image::Rgba<u8> {
        image::Rgba([self.blue, self.green, self.red, self.alpha])
    }
}

impl From<RgbColor> for gpui::Rgba {
    fn from(color: RgbColor) -> Self {
        color.with_alpha(255).into()
    }
}

impl From<RgbaColor> for gpui::Rgba {
    fn from(color: RgbaColor) -> Self {
        Self {
            r: f32::from(color.red) / 255.0,
            g: f32::from(color.green) / 255.0,
            b: f32::from(color.blue) / 255.0,
            a: f32::from(color.alpha) / 255.0,
        }
    }
}
