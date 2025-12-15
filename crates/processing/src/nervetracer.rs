use image::RgbaImage;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(Clone, Copy, Eq, PartialEq)]
struct Node {
    position: (u64, u64),
    cost: u64, // Using fixed-point arithmetic (cost * 1000)
}

// Min-heap ordering (reverse the comparison)
impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.cmp(&self.cost)
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn get_intensity(image: &RgbaImage, pos: (u64, u64)) -> f64 {
    let pixel = image.get_pixel(pos.0 as u32, pos.1 as u32);
    // Grayscale intensity (or use just one channel if preferred)
    (pixel[0] as f64 + pixel[1] as f64 + pixel[2] as f64) / 3.0
}

fn edge_weight(image: &RgbaImage, from: (u64, u64), to: (u64, u64)) -> f64 {
    let dx = (to.0 as f64) - (from.0 as f64);
    let dy = (to.1 as f64) - (from.1 as f64);
    let euclidean_dist = (dx * dx + dy * dy).sqrt();

    let intensity_avg = (get_intensity(image, from) + get_intensity(image, to)) / 2.0;

    // Add 1.0 to avoid zero-weight edges; adjust as needed
    euclidean_dist / (1.0 + intensity_avg)
}

fn get_neighbors(pos: (u64, u64), width: u64, height: u64) -> Vec<(u64, u64)> {
    let mut neighbors = Vec::new();
    let (x, y) = pos;

    // 8-connected neighborhood
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx >= 0 && ny >= 0 && (nx as u64) < width && (ny as u64) < height {
                neighbors.push((nx as u64, ny as u64));
            }
        }
    }
    neighbors
}

pub fn create_path(
    start_point: (u64, u64),
    end_point: (u64, u64),
    image: &RgbaImage,
) -> Option<Vec<(u64, u64)>> {
    let width = image.width() as u64;
    let height = image.height() as u64;

    // Priority queues
    let mut source_queue: BinaryHeap<Node> = BinaryHeap::new();
    let mut target_queue: BinaryHeap<Node> = BinaryHeap::new();

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
    source_queue.push(Node {
        position: start_point,
        cost: 0,
    });
    target_queue.push(Node {
        position: end_point,
        cost: 0,
    });
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

            let current_cost = cost_from_source[&pos];

            // Check if we've met the other search
            if let Some(&target_cost) = cost_from_target.get(&pos) {
                let total = current_cost + target_cost;
                if total < best_path_cost {
                    best_path_cost = total;
                    meeting_point = Some(pos);
                }
            }

            // Expand neighbors
            for neighbor in get_neighbors(pos, width, height) {
                if visited_source.contains(&neighbor) {
                    continue;
                }

                let edge_cost = edge_weight(image, pos, neighbor);
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
                        cost: (new_cost * 1000.0) as u64,
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

            let current_cost = cost_from_target[&pos];

            // Check if we've met the other search
            if let Some(&source_cost) = cost_from_source.get(&pos) {
                let total = current_cost + source_cost;
                if total < best_path_cost {
                    best_path_cost = total;
                    meeting_point = Some(pos);
                }
            }

            // Expand neighbors
            for neighbor in get_neighbors(pos, width, height) {
                if visited_target.contains(&neighbor) {
                    continue;
                }

                let edge_cost = edge_weight(image, pos, neighbor);
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
                        cost: (new_cost * 1000.0) as u64,
                    });
                }
            }
        }

        // Early termination check
        if let (Some(s), Some(t)) = (source_queue.peek(), target_queue.peek()) {
            let min_remaining = (s.cost + t.cost) as f64 / 1000.0;
            if min_remaining >= best_path_cost {
                break;
            }
        }
    }

    // Reconstruct path
    meeting_point.map(|meet| {
        let mut path = Vec::new();

        // Path from start to meeting point
        let mut current = meet;
        while current != start_point {
            path.push(current);
            current = parent_source[&current];
        }
        path.push(start_point);
        path.reverse();

        // Path from meeting point to end
        current = meet;
        while current != end_point {
            current = parent_target[&current];
            path.push(current);
        }

        path
    })
}
