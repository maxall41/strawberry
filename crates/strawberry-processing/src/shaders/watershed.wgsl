struct WatershedUniforms {
    width: u32,
    height: u32,
    sigma: f32,
    min_distance: u32,
}

@group(0) @binding(0) var<uniform> uniforms: WatershedUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> distance_buffer: array<f32>;

// Calculate gaussian weight for given distance and sigma
fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    let two_sigma_squared = 2.0 * sigma * sigma;
    return exp(-(distance * distance) / two_sigma_squared) / (3.14159265359 * two_sigma_squared);
}

// Sample distance value with bounds checking
fn sample_distance(x: i32, y: i32) -> f32 {
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);

    // Clamp coordinates to image bounds
    let clamped_x = clamp(x, 0, width - 1);
    let clamped_y = clamp(y, 0, height - 1);

    let index = u32(clamped_y * width + clamped_x);
    return distance_buffer[index];
}

// Apply gaussian blur to distance value at given coordinates
fn blur_distance(x: i32, y: i32, sigma: f32) -> f32 {
    if (sigma <= 0.001) {
        return sample_distance(x, y);
    }

    let radius = i32(sigma * 3.0); // 3-sigma radius
    var accumulated_value = 0.0;
    var total_weight = 0.0;

    // Sample in a square around the current pixel
    for (var dy = -radius; dy <= radius; dy++) {
        for (var dx = -radius; dx <= radius; dx++) {
            let sample_x = x + dx;
            let sample_y = y + dy;

            // Calculate distance from center
            let distance = sqrt(f32(dx * dx + dy * dy));

            // Skip samples outside the circular radius
            if (distance > f32(radius)) {
                continue;
            }

            // Calculate gaussian weight
            let weight = gaussian_weight(distance, sigma);

            // Sample the distance value
            let sample_value = sample_distance(sample_x, sample_y);

            // Accumulate weighted value
            accumulated_value += sample_value * weight;
            total_weight += weight;
        }
    }

    // Normalize by total weight
    if (total_weight > 0.001) {
        return accumulated_value / total_weight;
    } else {
        return sample_distance(x, y);
    }
}

// Check if pixel at (x,y) is a local maximum in the distance map
fn is_local_maximum(x: i32, y: i32, center_value: f32) -> bool {
    let min_dist = i32(uniforms.min_distance);
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);
    let center_index = u32(y) * uniforms.width + u32(x);
    let epsilon = 0.000001;

    // Check neighborhood for any higher values
    for (var dy = -min_dist; dy <= min_dist; dy++) {
        for (var dx = -min_dist; dx <= min_dist; dx++) {
            if (dx == 0 && dy == 0) {
                continue;
            }

            let neighbor_x = x + dx;
            let neighbor_y = y + dy;

            if (neighbor_x < 0 || neighbor_x >= width || neighbor_y < 0 || neighbor_y >= height) {
                continue;
            }

            // Get blurred distance value at neighbor
            let neighbor_value = blur_distance(neighbor_x, neighbor_y, uniforms.sigma);

            if (neighbor_value > center_value + epsilon) {
                return false;
            }

            // Keep exactly one deterministic marker on a flat maximum. This also
            // suppresses equal-height peaks inside the configured min_distance.
            let neighbor_index = u32(neighbor_y) * uniforms.width + u32(neighbor_x);
            if (abs(neighbor_value - center_value) <= epsilon && neighbor_index < center_index) {
                return false;
            }
        }
    }

    return true;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let index = u32(y * i32(uniforms.width) + x);

    // Get the original distance value
    let original_distance = distance_buffer[index];

    // Skip background pixels (distance = 0)
    if (original_distance <= 0.0) {
        output_buffer[index] = 0u;
        return;
    }

    // Get blurred distance value at current position
    let blurred_distance = blur_distance(x, y, uniforms.sigma);

    // Check if this is a local maximum
    let is_peak = is_local_maximum(x, y, blurred_distance);

    if (is_peak) {
        // A pixel index is already unique and deterministic. The +1 reserves zero
        // for non-markers without introducing hash collisions between basins.
        output_buffer[index] = index + 1u;
    } else {
        output_buffer[index] = 0u;
    }
}
