use gpui::{div, img, prelude::*, px, rgb, svg, Context, IntoElement, Rgba, SharedString};

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
            color: rgb(0xffffff),
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = color;
        self
    }

    pub fn eye_open() -> Self {
        Self::new(format!(
            "{}/assets/icons/eye_open.svg",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    pub fn eye_closed() -> Self {
        Self::new(format!(
            "{}/assets/icons/eye_closed.svg",
            env!("CARGO_MANIFEST_DIR")
        ))
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
