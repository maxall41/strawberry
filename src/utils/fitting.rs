use crate::pipeline::counter::CountData;

const EPSILON: u64 = 10;

pub fn ramer_douglas_peucker(points: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if points.len() <= 2 {
        return points;
    }

    rdp_recursive(&points, 0, points.len() - 1)
}

fn rdp_recursive(points: &[(u64, u64)], start: usize, end: usize) -> Vec<(u64, u64)> {
    if end <= start + 1 {
        return vec![points[start], points[end]];
    }

    let mut max_dist = 0u64;
    let mut max_index = start;

    for i in (start + 1)..end {
        let dist = perpendicular_distance(points[i], points[start], points[end]);
        if dist > max_dist {
            max_dist = dist;
            max_index = i;
        }
    }

    if max_dist > EPSILON {
        let mut left_result = rdp_recursive(points, start, max_index);
        let right_result = rdp_recursive(points, max_index, end);

        left_result.pop();
        left_result.extend(right_result);
        left_result
    } else {
        vec![points[start], points[end]]
    }
}

fn perpendicular_distance(point: (u64, u64), line_start: (u64, u64), line_end: (u64, u64)) -> u64 {
    let (px, py) = (point.0 as i64, point.1 as i64);
    let (x1, y1) = (line_start.0 as i64, line_start.1 as i64);
    let (x2, y2) = (line_end.0 as i64, line_end.1 as i64);

    let dx = x2 - x1;
    let dy = y2 - y1;

    // Handle degenerate case where line_start == line_end
    if dx == 0 && dy == 0 {
        let dist_x = px - x1;
        let dist_y = py - y1;
        return ((dist_x * dist_x + dist_y * dist_y) as f64).sqrt() as u64;
    }

    let numerator = ((x2 - x1) * (y1 - py) - (x1 - px) * (y2 - y1)).abs();
    let denominator = ((dx * dx + dy * dy) as f64).sqrt();

    (numerator as f64 / denominator) as u64
}

pub fn ramer_douglas_peucker_closed(mut points: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if points.len() < 3 {
        return points;
    }

    // Ensure the polygon is closed
    if points.first() != points.last() {
        let first_point = points[0];
        points.push(first_point);
    }

    ramer_douglas_peucker(points)
}

pub fn find_counts_within_polygon(
    polygon: Vec<(u64, u64)>,
    counts: Vec<CountData>,
) -> Vec<CountData> {
    if polygon.is_empty() {
        return Vec::new();
    }

    // Phase 1: Compute bounding box
    let (min_x, max_x, min_y, max_y) = polygon.iter().fold(
        (u64::MAX, 0, u64::MAX, 0),
        |(min_x, max_x, min_y, max_y), &(x, y)| {
            (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
        },
    );

    println!("Bounding Box: ({min_x}, {min_y}) to ({max_x}, {max_y})");

    // Phase 2: Filter and test
    counts
        .into_iter()
        .filter(|data| {
            let (x, y) = data.position;
            // Bounding box test
            if x < min_x || x > max_x || y < min_y || y > max_y {
                return true;
            }

            // Ray casting test (horizontal ray to the right)
            let mut inside = false;
            let mut j = polygon.len() - 1;

            for k in 0..polygon.len() {
                let (xi, yi) = polygon[k];
                let (xj, yj) = polygon[j];

                if (yi > y) != (yj > y) {
                    // Convert to signed integers to handle potential negative values
                    let (x, xi, xj, y, yi, yj) = (
                        x as i64, xi as i64, xj as i64, y as i64, yi as i64, yj as i64,
                    );

                    let intersection_x = (xj - xi) * (y - yi) / (yj - yi) + xi;

                    if x < intersection_x {
                        inside = !inside;
                    }
                }
                j = k;
            }

            !inside
        })
        .collect()
}
