use crate::{RgbaColor, ToolId};
use serde::{Deserialize, Serialize};

/// Styling options for circles
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CircleStyle {
    /// Border only with specified width
    Border { width: f32 },
    /// Filled circle
    Filled,
    /// Selected/highlighted style (border with different appearance)
    Selected { width: f32 },
}

/// Individual overlay elements that tools can emit
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverlayElement {
    /// A cached BGRA label overlay, rendered as one image rather than individual pixels.
    ImageMask {
        x: u32,
        y: u32,
        opacity: f32,
        #[serde(skip)]
        image: Option<std::sync::Arc<image::RgbaImage>>,
    },
    Circle {
        x: f32,
        y: f32,
        radius: f32,
        color: RgbaColor,
        style: CircleStyle,
    },
    Point {
        x: f32,
        y: f32,
        radius: f32,
        color: RgbaColor,
    },
    Polyline {
        points: Vec<(f32, f32)>,
        color: RgbaColor,
        stroke_width: f32,
    },
    Polygon {
        points: Vec<(f32, f32)>,
        fill_color: Option<RgbaColor>,
        stroke_color: RgbaColor,
        stroke_width: f32,
    },
    Label {
        x: f32,
        y: f32,
        text: String,
        color: RgbaColor,
        font_size: f32,
    },
}

/// Collection of overlays from a single tool
#[derive(Debug, Clone)]
pub struct ToolOverlay {
    /// Unique identifier for the tool producing these overlays
    pub tool_id: ToolId,
    /// Priority for rendering order (higher = on top)
    pub z_order: i32,
    /// Elements to render
    pub elements: Vec<OverlayElement>,
}

/// Aggregated overlays from all tools
#[derive(Debug, Clone, Default)]
pub struct OverlayCollection {
    pub overlays: Vec<ToolOverlay>,
}

impl OverlayCollection {
    #[must_use]
    pub const fn new() -> Self {
        Self { overlays: vec![] }
    }

    pub fn merge(&mut self, overlay: ToolOverlay) {
        // Remove existing overlay from same tool
        self.overlays.retain(|o| o.tool_id != overlay.tool_id);
        self.overlays.push(overlay);
        // Sort by z_order
        self.overlays.sort_by_key(|o| o.z_order);
    }

    pub fn all_elements(&self) -> impl Iterator<Item = &OverlayElement> {
        self.overlays.iter().flat_map(|o| o.elements.iter())
    }
}
