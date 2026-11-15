use image::RgbaImage;
use std::{collections::BTreeMap, ops::Range, sync::Arc};
use strawberry_types::{AnnotationKind, AnnotationStroke, BgraColor, BrushRadius, ClassId};

const MASK_TILE_SIZE: u32 = 256;

fn brush_spans(
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    radius: BrushRadius,
    mut visit: impl FnMut(u32, u32, u32),
) {
    let radius = i64::from(radius.get());
    let top = (i64::from(y) - radius).max(0);
    let bottom = (i64::from(y) + radius).min(i64::from(height) - 1);
    for row in top..=bottom {
        let dy = row - i64::from(y);
        let extent = (radius * radius - dy * dy).unsigned_abs().isqrt();
        let extent = i64::try_from(extent).unwrap_or(0);
        let left = (i64::from(x) - extent).max(0);
        let right = (i64::from(x) + extent + 1).min(i64::from(width));
        if let (Ok(row), Ok(left), Ok(right)) = (
            u32::try_from(row),
            u32::try_from(left),
            u32::try_from(right),
        ) {
            visit(row, left, right);
        }
    }
}

pub(crate) fn append_stroke_points(
    points: &mut Arc<Vec<(u32, u32)>>,
    (x, y): (u32, u32),
    radius: BrushRadius,
) -> Range<usize> {
    let first_new = points.len();
    if let Some(&(a, b)) = points.last() {
        let dx = i64::from(x) - i64::from(a);
        let dy = i64::from(y) - i64::from(b);
        let distance = dx.unsigned_abs().max(dy.unsigned_abs());
        if distance == 0 {
            return first_new..first_new;
        }
        let spacing = u64::from(radius.get().div_ceil(2));
        let steps = distance.div_ceil(spacing);
        for i in 1..=steps {
            if let (Ok(px), Ok(py)) = (
                u32::try_from(
                    i64::from(a)
                        + dx * i64::try_from(i).unwrap_or(0) / i64::try_from(steps).unwrap_or(1),
                ),
                u32::try_from(
                    i64::from(b)
                        + dy * i64::try_from(i).unwrap_or(0) / i64::try_from(steps).unwrap_or(1),
                ),
            ) {
                Arc::make_mut(points).push((px, py));
            }
        }
    } else {
        Arc::make_mut(points).push((x, y));
    }
    first_new..points.len()
}

pub(super) fn rasterize_labels<'a>(
    (source_width, source_height): (u32, u32),
    (width, height): (u32, u32),
    annotations: impl IntoIterator<Item = (AnnotationKind, &'a AnnotationStroke)>,
    class_ids: &[ClassId],
) -> BTreeMap<(u32, u32), u8> {
    let mut labels = BTreeMap::new();
    let scaled_x = |x| {
        u32::try_from(u64::from(x) * u64::from(width) / u64::from(source_width)).unwrap_or_default()
    };
    let scaled_y = |y| {
        u32::try_from(u64::from(y) * u64::from(height) / u64::from(source_height))
            .unwrap_or_default()
    };
    for (kind, stroke) in annotations {
        for &(x, y) in stroke.points.iter() {
            brush_spans(
                source_width,
                source_height,
                x,
                y,
                stroke.radius,
                |row, left, right| {
                    if left >= right {
                        return;
                    }
                    let row = scaled_y(row);
                    for column in scaled_x(left)..=scaled_x(right - 1) {
                        match kind {
                            AnnotationKind::Eraser => {
                                labels.remove(&(row, column));
                            }
                            AnnotationKind::Paint(id) => {
                                labels.insert(
                                    (row, column),
                                    class_ids
                                        .iter()
                                        .position(|&class| class == id)
                                        .expect("validated stroke class")
                                        as u8,
                                );
                            }
                        }
                    }
                },
            );
        }
    }
    labels
}

#[derive(Default)]
pub(crate) struct AnnotationMask {
    tiles: BTreeMap<(u32, u32), Arc<RgbaImage>>,
}

impl AnnotationMask {
    pub(crate) fn clear(&mut self) {
        self.tiles.clear();
    }

    pub(crate) fn paint(
        &mut self,
        (width, height): (u32, u32),
        points: &[(u32, u32)],
        radius: BrushRadius,
        color: BgraColor,
    ) {
        for &(x, y) in points {
            brush_spans(width, height, x, y, radius, |row, left, right| {
                let mut column = left;
                while column < right {
                    let origin = (
                        column / MASK_TILE_SIZE * MASK_TILE_SIZE,
                        row / MASK_TILE_SIZE * MASK_TILE_SIZE,
                    );
                    let end = right.min(origin.0 + MASK_TILE_SIZE);
                    if color != BgraColor::TRANSPARENT || self.tiles.contains_key(&origin) {
                        let tile = self.tiles.entry(origin).or_insert_with(|| {
                            Arc::new(RgbaImage::new(
                                (width - origin.0).min(MASK_TILE_SIZE),
                                (height - origin.1).min(MASK_TILE_SIZE),
                            ))
                        });
                        let tile = Arc::make_mut(tile);
                        for px in column..end {
                            tile.put_pixel(px - origin.0, row - origin.1, color.into_pixel());
                        }
                    }
                    column = end;
                }
            });
        }
    }

    pub(crate) fn rebuild<'a>(
        &mut self,
        dimensions: (u32, u32),
        strokes: impl IntoIterator<Item = (&'a AnnotationStroke, BgraColor)>,
    ) {
        let mut mask = Self::default();
        for (stroke, color) in strokes {
            mask.paint(dimensions, &stroke.points, stroke.radius, color);
        }
        for (key, tile) in &mut mask.tiles {
            if let Some(old) = self.tiles.get(key) {
                if old == tile {
                    *tile = old.clone();
                }
            }
        }
        *self = mask;
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&(u32, u32), &Arc<RgbaImage>)> {
        self.tiles.iter()
    }
}
