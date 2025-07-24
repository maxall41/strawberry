struct DistanceTransformUniforms {
    width: u32,
    height: u32,
    step_size: u32,
    iteration: u32,
}

@group(0) @binding(0) var<uniform> uniforms: DistanceTransformUniforms;
@group(0) @binding(1) var<storage, read_write> seeds_buffer: array<vec2<i32>>;
@group(0) @binding(2) var<storage, read> binary_buffer: array<u32>;
@group(0) @binding(3) var<storage, read_write> distance_buffer: array<f32>;

const NO_SEED = vec2<i32>(-1, -1);

// Sample seed with bounds checking
fn sample_seed(x: i32, y: i32) -> vec2<i32> {
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);

    if (x < 0 || x >= width || y < 0 || y >= height) {
        return NO_SEED;
    }

    let index = u32(y * width + x);
    return seeds_buffer[index];
}

// Calculate squared distance between two points
fn squared_distance(p1: vec2<i32>, p2: vec2<i32>) -> f32 {
    let dx = f32(p1.x - p2.x);
    let dy = f32(p1.y - p2.y);
    return dx * dx + dy * dy;
}

// Initialize seeds and distances
@compute @workgroup_size(8, 8)
fn init_seeds(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let index = u32(y * i32(uniforms.width) + x);
    let is_foreground = binary_buffer[index] != 0u;

    if (is_foreground) {
        // Foreground pixels start with no seed
        seeds_buffer[index] = NO_SEED;
        distance_buffer[index] = 999999.0; // Large initial distance
    } else {
        // Background pixels are seeds to themselves
        seeds_buffer[index] = vec2<i32>(x, y);
        distance_buffer[index] = 0.0;
    }
}

// Jump flooding iteration
@compute @workgroup_size(8, 8)
fn jump_flood(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let index = u32(y * i32(uniforms.width) + x);
    let current_pos = vec2<i32>(x, y);
    let current_seed = seeds_buffer[index];

    var best_seed = current_seed;
    var best_distance = 999999.0;

    if (current_seed.x != -1) {
        best_distance = squared_distance(current_pos, current_seed);
    }

    let step = i32(uniforms.step_size);

    // Check 8 neighbors at current step size
    let offsets = array<vec2<i32>, 8>(
        vec2<i32>(-step, -step),
        vec2<i32>( 0,    -step),
        vec2<i32>( step, -step),
        vec2<i32>(-step,  0),
        vec2<i32>( step,  0),
        vec2<i32>(-step,  step),
        vec2<i32>( 0,     step),
        vec2<i32>( step,  step)
    );

    for (var i = 0; i < 8; i++) {
        let neighbor_pos = current_pos + offsets[i];
        let neighbor_seed = sample_seed(neighbor_pos.x, neighbor_pos.y);

        if (neighbor_seed.x != -1) {
            let dist = squared_distance(current_pos, neighbor_seed);
            if (dist < best_distance) {
                best_distance = dist;
                best_seed = neighbor_seed;
            }
        }
    }

    seeds_buffer[index] = best_seed;
}

// Finalize distances (convert from squared to actual distance)
@compute @workgroup_size(8, 8)
fn finalize_distances(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let index = u32(y * i32(uniforms.width) + x);
    let current_pos = vec2<i32>(x, y);
    let seed = seeds_buffer[index];

    if (seed.x != -1) {
        let squared_dist = squared_distance(current_pos, seed);
        distance_buffer[index] = sqrt(squared_dist);
    } else {
        distance_buffer[index] = 0.0;
    }
}
