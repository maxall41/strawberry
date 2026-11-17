use num_traits::ToPrimitive;
use strawberry_types::CountData;

const EPSILON: f64 = 10.0;

/// Simplify a path by removing points within the fixed distance tolerance.
#[must_use]
pub fn ramer_douglas_peucker(points: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if points.len() <= 2 {
        return points;
    }
    rdp_recursive(&points)
}

/// Recursively simplify path sections around their most distant points.
fn rdp_recursive(points: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut max_dist = 0.0;
    let mut max_index = 0;
    for (i, &point) in points
        .iter()
        .enumerate()
        .skip(1)
        .take(points.len().saturating_sub(2))
    {
        // If a distance is unrepresentable, retain the point instead of simplifying it away.
        let dist = perpendicular_distance(point, first, last).map_or(f64::INFINITY, f64::floor);
        if dist > max_dist {
            max_dist = dist;
            max_index = i;
        }
    }
    if max_dist > EPSILON {
        let mut left = rdp_recursive(points.get(..=max_index).unwrap_or_default());
        let right = rdp_recursive(points.get(max_index..).unwrap_or_default());
        left.pop();
        left.extend(right);
        left
    } else {
        vec![first, last]
    }
}

// Subtract in the integer domain first to retain small distances near u64::MAX.
/// Return an exactly representable signed difference between coordinates.
fn delta(a: u64, b: u64) -> Option<f64> {
    let distance = a.abs_diff(b);
    let magnitude = distance.to_f64()?;
    // Computation must not silently round an integer distance. An inexact
    // conversion makes the simplifier retain the point instead.
    if magnitude.to_u64() != Some(distance) {
        return None;
    }
    Some(if a < b { -magnitude } else { magnitude })
}

/// Calculate distance from a point to a line when differences are representable.
fn perpendicular_distance(point: (u64, u64), start: (u64, u64), end: (u64, u64)) -> Option<f64> {
    let dx = delta(end.0, start.0)?;
    let dy = delta(end.1, start.1)?;
    let px = delta(point.0, start.0)?;
    let py = delta(point.1, start.1)?;
    if start == end {
        return Some(px.hypot(py));
    }
    Some(dx.mul_add(py, -(px * dy)).abs() / dx.hypot(dy))
}

/// Close a polygon path before simplifying its points.
#[must_use]
pub fn ramer_douglas_peucker_closed(mut points: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if points.len() < 3 {
        return points;
    }
    if points.first() != points.last()
        && let Some(&first) = points.first()
    {
        points.push(first);
    }
    ramer_douglas_peucker(points)
}

/// Remove counts inside the polygon and return the remaining counts.
#[must_use]
pub fn find_counts_within_polygon(
    polygon: &[(u64, u64)],
    counts: Vec<CountData>,
) -> Vec<CountData> {
    let Some(&last) = polygon.last() else {
        return Vec::new();
    };
    let (min_x, max_x, min_y, max_y) = polygon.iter().fold(
        (u64::MAX, 0, u64::MAX, 0),
        |(min_x, max_x, min_y, max_y), &(x, y)| {
            (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
        },
    );
    counts
        .into_iter()
        .filter(|data| {
            let (x, y) = data.position;
            if x < min_x || x > max_x || y < min_y || y > max_y {
                return true;
            }
            let mut previous = last;
            let mut inside = false;
            for &current in polygon {
                let (xi, yi) = current;
                let (xj, yj) = previous;
                if (yi > y) != (yj > y) {
                    // The ray crosses this edge, so the vertical fraction is in [0, 1].
                    // A product of two u64 differences fits in u128, and its quotient
                    // fits in u64. Keep integer truncation consistent with existing lassos.
                    let offset = u128::from(xj.abs_diff(xi))
                        .saturating_mul(u128::from(y.abs_diff(yi)))
                        .checked_div(u128::from(yj.abs_diff(yi)))
                        .unwrap_or_default();
                    let offset = u64::try_from(offset).unwrap_or(u64::MAX);
                    let intersection_x = if xj < xi {
                        xi.saturating_sub(offset)
                    } else {
                        xi.saturating_add(offset)
                    };
                    if x < intersection_x {
                        inside = !inside;
                    }
                }
                previous = current;
            }
            !inside
        })
        .collect()
}
