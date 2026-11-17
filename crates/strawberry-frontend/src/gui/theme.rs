#![allow(dead_code)]
// Single source of truth for the app's palette.
use gpui::Rgba;

#[expect(
    clippy::as_conversions,
    reason = "Converting u8 color channels to f32 is exact; From is not const"
)]
/// Convert a packed RGB color to an opaque GPUI color.
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
pub const CANVAS: Rgba = hex(0x1e_20_23);
/// Properties sidebar and workspace chrome.
pub const WINDOW_BG: Rgba = hex(0x2e_30_35);
/// Inset panel / section background.
pub const SURFACE: Rgba = hex(0x35_37_3c);
/// Deeper inset (text fields, wells).
pub const SURFACE_DEEP: Rgba = hex(0x26_28_2c);
/// Alternate row background for lists.
pub const SURFACE_ALT: Rgba = hex(0x3a_3d_42);

/// Raised interactive control (buttons, chips).
pub const CONTROL: Rgba = hex(0x43_46_4d);
pub const CONTROL_HOVER: Rgba = hex(0x51_55_5d);
pub const CONTROL_ACTIVE: Rgba = hex(0x39_3c_42);

pub const BORDER: Rgba = hex(0x4a_4e_56);
pub const BORDER_MUTED: Rgba = hex(0x3c_3f_45);

pub const TEXT: Rgba = hex(0xe8_ea_ed);
pub const TEXT_MUTED: Rgba = hex(0xa6_ab_b4);

pub const ACCENT: Rgba = hex(0x40_88_d8);
pub const ACCENT_HOVER: Rgba = hex(0x54_9a_e4);
pub const ACCENT_ACTIVE: Rgba = hex(0x36_72_bd);

pub const SUCCESS: Rgba = hex(0x3d_9a_5f);
pub const SUCCESS_HOVER: Rgba = hex(0x47_ab_6c);
pub const SUCCESS_ACTIVE: Rgba = hex(0x35_87_53);

pub const DANGER: Rgba = hex(0xc2_50_49);
pub const DANGER_HOVER: Rgba = hex(0xd0_5e_57);
pub const DANGER_ACTIVE: Rgba = hex(0xa8_44_3e);

/// Blend a color toward white. Used for hover/feedback states.
pub fn lighten(color: Rgba, amount: f32) -> Rgba {
    Rgba {
        r: (1.0 - color.r).mul_add(amount, color.r),
        g: (1.0 - color.g).mul_add(amount, color.g),
        b: (1.0 - color.b).mul_add(amount, color.b),
        a: color.a,
    }
}
