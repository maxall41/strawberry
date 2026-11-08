use gpui::{
    IntoElement, Path, PathBuilder, Pixels, Point, Rgba, Window, canvas, div, point, prelude::*, px,
};

pub struct Polyline {
    /// Logical pixel positions relative to the containing canvas.
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

/// GPUI's path painter consumes window coordinates, unlike positioned divs.
/// Use the current paint bounds so layout changes never leave a stale offset.
fn build_window_path(
    points: &[(f32, f32)],
    canvas_origin: Point<Pixels>,
    stroke_width: Pixels,
) -> Option<Path<Pixels>> {
    if points.len() < 2 {
        return None;
    }
    let window_point = |(x, y): (f32, f32)| {
        point(
            px(x + f32::from(canvas_origin.x)),
            px(y + f32::from(canvas_origin.y)),
        )
    };
    let (&first, rest) = points.split_first()?;
    let mut builder = PathBuilder::stroke(stroke_width);
    builder.move_to(window_point(first));
    for &position in rest {
        builder.line_to(window_point(position));
    }
    builder.build().ok()
}

impl IntoElement for Polyline {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        let Self {
            points,
            color,
            stroke_width,
        } = self;
        div().absolute().left_0().top_0().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    if let Some(path) = build_window_path(&points, bounds.origin, stroke_width) {
                        window.paint_path(path, color);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_follow_pixel_centers_through_zoom_pan_and_canvas_offsets() -> anyhow::Result<()> {
        for origin in [point(px(0.), px(0.)), point(px(120.), px(44.))] {
            for zoom in [0.25, 1.0, 2.5, 10.0, 50.0] {
                for pan in [(0.0, 0.0), (-90.5, 37.25)] {
                    // A horizontal trace between the centers of pixels (10,20)
                    // and (30,20), transformed exactly like the displayed image.
                    let points = [(10.5_f32, 20.5_f32), (30.5_f32, 20.5_f32)]
                        .map(|(x, y)| (x.mul_add(zoom, pan.0), y.mul_add(zoom, pan.1)));
                    let path = build_window_path(&points, origin, px(2.0 * zoom))
                        .ok_or_else(|| anyhow::anyhow!("Trace could not be built"))?;
                    let center = path.bounds.center();
                    let expected_x = 20.5_f32.mul_add(zoom, pan.0) + f32::from(origin.x);
                    let expected_y = 20.5_f32.mul_add(zoom, pan.1) + f32::from(origin.y);
                    anyhow::ensure!(
                        (f32::from(center.x) - expected_x).abs() < 0.001,
                        "Trace drifted horizontally at zoom {zoom}"
                    );
                    anyhow::ensure!(
                        (f32::from(center.y) - expected_y).abs() < 0.001,
                        "Trace drifted vertically at zoom {zoom}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn incomplete_polylines_do_not_paint() {
        let origin = point(px(120.), px(44.));
        assert!(build_window_path(&[], origin, px(2.)).is_none());
        assert!(build_window_path(&[(10., 20.)], origin, px(2.)).is_none());
    }
}
