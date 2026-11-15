use anyhow::{Context, Result, ensure};
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
    let top = i64::from(y).saturating_sub(radius).max(0);
    let bottom = i64::from(y)
        .saturating_add(radius)
        .min(i64::from(height).saturating_sub(1));
    for row in top..=bottom {
        let dy = row.saturating_sub(i64::from(y));
        let extent = radius
            .saturating_mul(radius)
            .saturating_sub(dy.saturating_mul(dy))
            .unsigned_abs()
            .isqrt();
        let extent = i64::try_from(extent).unwrap_or(0);
        let left = i64::from(x).saturating_sub(extent).max(0);
        let right = i64::from(x)
            .saturating_add(extent)
            .saturating_add(1)
            .min(i64::from(width));
        if let (Ok(row), Ok(left), Ok(right)) = (
            u32::try_from(row),
            u32::try_from(left),
            u32::try_from(right),
        ) {
            visit(row, left, right);
        }
    }
}

pub fn append_stroke_points(
    points: &mut Arc<Vec<(u32, u32)>>,
    (x, y): (u32, u32),
    radius: BrushRadius,
) -> Range<usize> {
    let first_new = points.len();
    if let Some(&(a, b)) = points.last() {
        let dx = i128::from(x).saturating_sub(i128::from(a));
        let dy = i128::from(y).saturating_sub(i128::from(b));
        let distance = dx.unsigned_abs().max(dy.unsigned_abs());
        if distance == 0 {
            return first_new..first_new;
        }
        let spacing = u128::from(radius.get().div_ceil(2));
        let steps = distance.div_ceil(spacing);
        for i in 1..=steps {
            if let (Ok(px), Ok(py)) = (
                u32::try_from(
                    i128::from(a).saturating_add(
                        dx.saturating_mul(i128::try_from(i).unwrap_or_default())
                            .checked_div(i128::try_from(steps).unwrap_or(1))
                            .unwrap_or_default(),
                    ),
                ),
                u32::try_from(
                    i128::from(b).saturating_add(
                        dy.saturating_mul(i128::try_from(i).unwrap_or_default())
                            .checked_div(i128::try_from(steps).unwrap_or(1))
                            .unwrap_or_default(),
                    ),
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
) -> Result<BTreeMap<(u32, u32), u8>> {
    ensure!(
        source_width > 0 && source_height > 0 && width > 0 && height > 0,
        "Empty training image"
    );
    let mut labels = BTreeMap::new();
    let scaled_x = |x| {
        u32::try_from(
            u64::from(x)
                .saturating_mul(u64::from(width))
                .checked_div(u64::from(source_width))
                .unwrap_or_default(),
        )
        .unwrap_or_default()
    };
    let scaled_y = |y| {
        u32::try_from(
            u64::from(y)
                .saturating_mul(u64::from(height))
                .checked_div(u64::from(source_height))
                .unwrap_or_default(),
        )
        .unwrap_or_default()
    };
    for (kind, stroke) in annotations {
        let label = match kind {
            AnnotationKind::Paint(id) => Some(u8::try_from(
                class_ids
                    .iter()
                    .position(|&class| class == id)
                    .context("Unknown annotation class")?,
            )?),
            AnnotationKind::Eraser => None,
        };
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
                    for column in scaled_x(left)..=scaled_x(right.saturating_sub(1)) {
                        match label {
                            None => {
                                labels.remove(&(row, column));
                            }
                            Some(label) => {
                                labels.insert((row, column), label);
                            }
                        }
                    }
                },
            );
        }
    }
    Ok(labels)
}

#[derive(Default)]
pub struct AnnotationMask {
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
                        (column / MASK_TILE_SIZE).saturating_mul(MASK_TILE_SIZE),
                        (row / MASK_TILE_SIZE).saturating_mul(MASK_TILE_SIZE),
                    );
                    let end = right.min(origin.0.saturating_add(MASK_TILE_SIZE));
                    if color != BgraColor::TRANSPARENT || self.tiles.contains_key(&origin) {
                        let tile = self.tiles.entry(origin).or_insert_with(|| {
                            Arc::new(RgbaImage::new(
                                width.saturating_sub(origin.0).min(MASK_TILE_SIZE),
                                height.saturating_sub(origin.1).min(MASK_TILE_SIZE),
                            ))
                        });
                        let tile = Arc::make_mut(tile);
                        for px in column..end {
                            tile.put_pixel(
                                px.saturating_sub(origin.0),
                                row.saturating_sub(origin.1),
                                color.into_pixel(),
                            );
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
            if let Some(old) = self.tiles.get(key)
                && old == tile
            {
                *tile = old.clone();
            }
        }
        *self = mask;
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&(u32, u32), &Arc<RgbaImage>)> {
        self.tiles.iter()
    }
}
