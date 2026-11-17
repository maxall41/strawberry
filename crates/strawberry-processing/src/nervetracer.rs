use image::RgbaImage;
use num_traits::ToPrimitive;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(Clone, Copy, Eq, PartialEq)]
struct Node {
    position: (u64, u64),
    cost: u64, // Using fixed-point arithmetic (cost * 1000)
}

// Min-heap ordering (reverse the comparison)
impl Ord for Node {
    /// Reverse cost ordering so the heap visits the least expensive node first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.cmp(&self.cost)
    }
}

impl PartialOrd for Node {
    /// Compare nodes using their reversed traversal-cost ordering.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Read the mean color-channel intensity at a valid image coordinate.
fn get_intensity(image: &RgbaImage, pos: (u64, u64)) -> Option<f64> {
    let pixel = image.get_pixel_checked(u32::try_from(pos.0).ok()?, u32::try_from(pos.1).ok()?)?;
    let [r, g, b, _] = pixel.0;
    Some((f64::from(r) + f64::from(g) + f64::from(b)) / 3.0)
}

/// Calculate a distance-based traversal cost that favors brighter pixels.
fn edge_weight(image: &RgbaImage, from: (u64, u64), to: (u64, u64)) -> Option<f64> {
    let dx = f64::from(u32::try_from(to.0).ok()?) - f64::from(u32::try_from(from.0).ok()?);
    let dy = f64::from(u32::try_from(to.1).ok()?) - f64::from(u32::try_from(from.1).ok()?);
    let euclidean_dist = dx.hypot(dy);

    let intensity_avg = f64::midpoint(get_intensity(image, from)?, get_intensity(image, to)?);

    // Add 1.0 to avoid zero-weight edges; adjust as needed
    Some(euclidean_dist / (1.0 + intensity_avg))
}

/// Collect the eight neighboring coordinates that fall within the image.
fn get_neighbors(pos: (u64, u64), width: u64, height: u64) -> Vec<(u64, u64)> {
    let mut neighbors = Vec::new();
    let (x, y) = pos;

    // 8-connected neighborhood
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            if let Some((nx, ny)) = x
                .checked_add_signed(i64::from(dx))
                .zip(y.checked_add_signed(i64::from(dy)))
                && nx < width
                && ny < height
            {
                neighbors.push((nx, ny));
            }
        }
    }
    neighbors
}

/// Trace a brightness-weighted path between endpoints, rejecting out-of-bounds coordinates.
#[must_use]
pub fn create_path(
    start_point: (u64, u64),
    end_point: (u64, u64),
    image: &RgbaImage,
) -> Option<Vec<(u64, u64)>> {
    let width = u64::from(image.width());
    let height = u64::from(image.height());

    if start_point.0 >= width
        || start_point.1 >= height
        || end_point.0 >= width
        || end_point.1 >= height
    {
        return None;
    }
    trace_path(start_point, end_point, image, width, height)
}

/// Create a search queue containing one zero-cost starting node.
fn initial_queue(position: (u64, u64)) -> BinaryHeap<Node> {
    BinaryHeap::from([Node { position, cost: 0 }])
}

/// Search from both endpoints and reconstruct the best meeting path.
fn trace_path(
    start_point: (u64, u64),
    end_point: (u64, u64),
    image: &RgbaImage,
    width: u64,
    height: u64,
) -> Option<Vec<(u64, u64)>> {
    let mut source_queue = initial_queue(start_point);
    let mut target_queue = initial_queue(end_point);

    // Cost maps (g-values)
    let mut cost_from_source: HashMap<(u64, u64), f64> = HashMap::new();
    let mut cost_from_target: HashMap<(u64, u64), f64> = HashMap::new();

    // Parent maps for path reconstruction
    let mut parent_source: HashMap<(u64, u64), (u64, u64)> = HashMap::new();
    let mut parent_target: HashMap<(u64, u64), (u64, u64)> = HashMap::new();

    // Visited sets
    let mut visited_source: HashSet<(u64, u64)> = HashSet::new();
    let mut visited_target: HashSet<(u64, u64)> = HashSet::new();

    // Initialize
    cost_from_source.insert(start_point, 0.0);
    cost_from_target.insert(end_point, 0.0);

    let mut best_path_cost = f64::INFINITY;
    let mut meeting_point: Option<(u64, u64)> = None;

    while !source_queue.is_empty() && !target_queue.is_empty() {
        // Expand from source side
        if let Some(current) = source_queue.pop() {
            let pos = current.position;

            if visited_source.contains(&pos) {
                continue;
            }
            visited_source.insert(pos);

            let current_cost = *cost_from_source.get(&pos)?;

            // Check if we've met the other search
            consider_meeting(
                pos,
                current_cost,
                &cost_from_target,
                &mut best_path_cost,
                &mut meeting_point,
            );

            // Expand neighbors
            for neighbor in get_neighbors(pos, width, height) {
                if visited_source.contains(&neighbor) {
                    continue;
                }

                let edge_cost = edge_weight(image, pos, neighbor)?;
                let new_cost = current_cost + edge_cost;

                let existing_cost = cost_from_source
                    .get(&neighbor)
                    .copied()
                    .unwrap_or(f64::INFINITY);

                if new_cost < existing_cost {
                    cost_from_source.insert(neighbor, new_cost);
                    parent_source.insert(neighbor, pos);
                    source_queue.push(Node {
                        position: neighbor,
                        cost: (new_cost * 1000.0).to_u64()?,
                    });
                }
            }
        }

        // Expand from target side
        if let Some(current) = target_queue.pop() {
            let pos = current.position;

            if visited_target.contains(&pos) {
                continue;
            }
            visited_target.insert(pos);

            let current_cost = *cost_from_target.get(&pos)?;

            // Check if we've met the other search
            consider_meeting(
                pos,
                current_cost,
                &cost_from_source,
                &mut best_path_cost,
                &mut meeting_point,
            );

            // Expand neighbors
            for neighbor in get_neighbors(pos, width, height) {
                if visited_target.contains(&neighbor) {
                    continue;
                }

                let edge_cost = edge_weight(image, pos, neighbor)?;
                let new_cost = current_cost + edge_cost;

                let existing_cost = cost_from_target
                    .get(&neighbor)
                    .copied()
                    .unwrap_or(f64::INFINITY);

                if new_cost < existing_cost {
                    cost_from_target.insert(neighbor, new_cost);
                    parent_target.insert(neighbor, pos);
                    target_queue.push(Node {
                        position: neighbor,
                        cost: (new_cost * 1000.0).to_u64()?,
                    });
                }
            }
        }

        // Early termination check
        if let (Some(s), Some(t)) = (source_queue.peek(), target_queue.peek()) {
            let min_remaining = s.cost.checked_add(t.cost)?.to_f64()? / 1000.0;
            if min_remaining >= best_path_cost {
                break;
            }
        }
    }

    reconstruct_path(
        meeting_point?,
        start_point,
        end_point,
        &parent_source,
        &parent_target,
    )
}

/// Update the best meeting point when both searches have reached a cheaper connection.
fn consider_meeting(
    position: (u64, u64),
    current_cost: f64,
    other_costs: &HashMap<(u64, u64), f64>,
    best_cost: &mut f64,
    meeting: &mut Option<(u64, u64)>,
) {
    if let Some(&other_cost) = other_costs.get(&position) {
        let total = current_cost + other_cost;
        if total < *best_cost {
            *best_cost = total;
            *meeting = Some(position);
        }
    }
}

/// Join both parent chains into an ordered path from the start to the end.
fn reconstruct_path(
    meet: (u64, u64),
    start: (u64, u64),
    end: (u64, u64),
    source: &HashMap<(u64, u64), (u64, u64)>,
    target: &HashMap<(u64, u64), (u64, u64)>,
) -> Option<Vec<(u64, u64)>> {
    let mut path = Vec::new();
    let mut current = meet;
    while current != start {
        path.push(current);
        current = *source.get(&current)?;
    }
    path.push(start);
    path.reverse();
    current = meet;
    while current != end {
        current = *target.get(&current)?;
        path.push(current);
    }
    Some(path)
}
