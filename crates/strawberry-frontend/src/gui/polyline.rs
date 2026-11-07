use gpui::{
    Background, IntoElement, PathBuilder, Pixels, Rgba, Window, canvas, div, point, prelude::*, px,
};

pub struct Polyline {
    points: Vec<(f32, f32)>,
    color: Rgba,
    stroke_width: Pixels,
}

impl Polyline {
    pub const fn new(points: Vec<(f32, f32)>, color: Rgba, stroke_width: Pixels) -> Self {
        Self {
            points,
            color,
            stroke_width,
        }
    }
}

impl IntoElement for Polyline {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        let points = self.points.clone();
        let color = self.color;
        let stroke_width = self.stroke_width;

        div()
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .child(
                canvas(
                    // Event callback (no-op for static polyline)
                    move |_, _, _| {},
                    // Paint callback
                    move |_, (), window, _| {
                        if points.len() < 2 {
                            return;
                        }

                        // Create a stroked path
                        let mut builder = PathBuilder::stroke(stroke_width);

                        // Set the first point
                        let Some((&(x0, y0), rest)) = points.split_first() else {
                            return;
                        };
                        builder.move_to(point(px(x0), px(y0)));

                        // Add remaining points
                        for &(x, y) in rest {
                            builder.line_to(point(px(x), px(y)));
                        }

                        // Build and paint the path
                        if let Ok(path) = builder.build() {
                            let background: Background = color.into();
                            window.paint_path(path, background);
                        }
                    },
                )
                .size_full(),
            )
    }
}

impl RenderOnce for Polyline {
    fn render(self, _window: &mut Window, _cx: &mut gpui::App) -> impl IntoElement {
        self.into_element()
    }
}
