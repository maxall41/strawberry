use gpui::{IntoElement, Rgba, SharedString, div, prelude::*, px, svg};

use super::theme;

pub fn asset_path(name: &str) -> String {
    format!("{}/../../assets/icons/{name}", env!("CARGO_MANIFEST_DIR"))
}

pub struct Icon {
    path: SharedString,
    size: f32,
    color: Rgba,
}

impl Icon {
    pub fn new(path: impl Into<SharedString>) -> Self {
        Self {
            path: path.into(),
            size: 16f32,
            color: theme::TEXT,
        }
    }

    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    #[allow(dead_code)]
    pub const fn color(mut self, color: Rgba) -> Self {
        self.color = color;
        self
    }

    fn from_asset(name: &str) -> Self {
        Self::new(asset_path(name))
    }

    pub fn eye_open() -> Self {
        Self::from_asset("eye_open.svg")
    }

    pub fn eye_closed() -> Self {
        Self::from_asset("eye_closed.svg")
    }

    pub fn x() -> Self {
        Self::from_asset("x.svg")
    }
}

impl IntoElement for Icon {
    type Element = gpui::Div;

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
    fn render(self, _window: &mut gpui::Window, _cx: &mut gpui::App) -> impl IntoElement {
        self.into_element()
    }
}
