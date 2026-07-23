// Single source of truth for the app's palette. Every color in the frontend
// should come from here so windows and widgets stay visually consistent.

use gpui::Rgba;

// gpui's `rgb()` is not const, so build Rgba at compile time ourselves.
const fn hex(hex: u32) -> Rgba {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// Main viewer background behind the image.
pub const CANVAS: Rgba = hex(0x1e2023);
/// Tool window background.
pub const WINDOW_BG: Rgba = hex(0x2e3035);
/// Inset panel / section background.
pub const SURFACE: Rgba = hex(0x35373c);
/// Deeper inset (text fields, wells).
pub const SURFACE_DEEP: Rgba = hex(0x26282c);
/// Alternate row background for lists.
pub const SURFACE_ALT: Rgba = hex(0x3a3d42);

/// Raised interactive control (buttons, chips).
pub const CONTROL: Rgba = hex(0x43464d);
pub const CONTROL_HOVER: Rgba = hex(0x51555d);
pub const CONTROL_ACTIVE: Rgba = hex(0x393c42);

pub const BORDER: Rgba = hex(0x4a4e56);
pub const BORDER_MUTED: Rgba = hex(0x3c3f45);

pub const TEXT: Rgba = hex(0xe8eaed);
pub const TEXT_MUTED: Rgba = hex(0xa6abb4);

pub const ACCENT: Rgba = hex(0x4088d8);
pub const ACCENT_HOVER: Rgba = hex(0x549ae4);
pub const ACCENT_ACTIVE: Rgba = hex(0x3672bd);

pub const SUCCESS: Rgba = hex(0x3d9a5f);
pub const SUCCESS_HOVER: Rgba = hex(0x47ab6c);
pub const SUCCESS_ACTIVE: Rgba = hex(0x358753);

pub const DANGER: Rgba = hex(0xc25049);
pub const DANGER_HOVER: Rgba = hex(0xd05e57);
pub const DANGER_ACTIVE: Rgba = hex(0xa8443e);

/// Blend a color toward white. For hover/feedback states derived from dynamic
/// colors (e.g. count type colors) that have no fixed *_HOVER constant.
pub fn lighten(color: Rgba, amount: f32) -> Rgba {
    Rgba {
        r: color.r + (1.0 - color.r) * amount,
        g: color.g + (1.0 - color.g) * amount,
        b: color.b + (1.0 - color.b) * amount,
        a: color.a,
    }
}
