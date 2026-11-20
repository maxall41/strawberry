use gpui::{IntoElement, Rgba, SharedString, div, prelude::*, px, svg};

use super::theme;

/// Build the filesystem path for an icon asset.
pub fn asset_path(name: &str) -> String {
    format!("{}/../../assets/icons/{name}", env!("CARGO_MANIFEST_DIR"))
}

pub struct Icon {
    path: SharedString,
    size: f32,
    color: Rgba,
}

impl Icon {
    /// Create `Icon` with its initial state and configuration.
    pub fn new(path: impl Into<SharedString>) -> Self {
        Self {
            path: path.into(),
            size: 16f32,
            color: theme::TEXT,
        }
    }

    /// Set the icon’s displayed size.
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Set the icon’s displayed color.
    pub const fn color(mut self, color: Rgba) -> Self {
        self.color = color;
        self
    }

    /// Create an icon from a named bundled asset.
    fn from_asset(name: &str) -> Self {
        Self::new(asset_path(name))
    }

    /// Create the visible-state eye icon.
    pub fn eye_open() -> Self {
        Self::from_asset("eye_open.svg")
    }

    /// Create the hidden-state eye icon.
    pub fn eye_closed() -> Self {
        Self::from_asset("eye_closed.svg")
    }

    /// Create the close icon.
    pub fn x() -> Self {
        Self::from_asset("x.svg").size(12.)
    }

    /// Create the add icon.
    pub fn plus() -> Self {
        Self::from_asset("plus.svg").size(12.)
    }

    /// Create the subtract icon.
    pub fn minus() -> Self {
        Self::from_asset("minus.svg").size(12.)
    }

    /// Create the warning icon, which marks out-of-date results.
    pub fn warning() -> Self {
        Self::from_asset("warning.svg").size(12.)
    }

    /// Create the code icon, which marks script controls.
    pub fn code() -> Self {
        Self::from_asset("code.svg").size(14.)
    }

    /// Create the expanded or collapsed disclosure icon.
    pub fn disclosure(expanded: bool) -> Self {
        Self::from_asset(if expanded {
            "caret_down.svg"
        } else {
            "caret_right.svg"
        })
    }
}

impl IntoElement for Icon {
    type Element = gpui::Div;

    /// Convert this component into its GPUI element.
    fn into_element(self) -> Self::Element {
        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(self.size))
            .child(svg().path(self.path).text_color(self.color).size_full())
    }
}

impl RenderOnce for Icon {
    /// Build the icon view and its interaction handlers.
    fn render(self, _window: &mut gpui::Window, _cx: &mut gpui::App) -> impl IntoElement {
        self.into_element()
    }
}
