struct GaussianBlurUniforms {
    width: u32,
    height: u32,
    radius: f32,
    sigma: f32,
}

@group(0) @binding(0) var<uniform> uniforms: GaussianBlurUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;

// Convert RGBA8 packed u32 to vec4<f32>
fn unpack_rgba8(packed: u32) -> vec4<f32> {
    let r = f32((packed >> 0u) & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let b = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Convert vec4<f32> to RGBA8 packed u32
fn pack_rgba8(color: vec4<f32>) -> u32 {
    let r = u32(clamp(color.r * 255.0, 0.0, 255.0));
    let g = u32(clamp(color.g * 255.0, 0.0, 255.0));
    let b = u32(clamp(color.b * 255.0, 0.0, 255.0));
    let a = u32(clamp(color.a * 255.0, 0.0, 255.0));
    return r | (g << 8u) | (b << 16u) | (a << 24u);
}

// Calculate gaussian weight for given distance and sigma
fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    let two_sigma_squared = 2.0 * sigma * sigma;
    return exp(-(distance * distance) / two_sigma_squared) / (3.14159265359 * two_sigma_squared);
}

// Sample pixel with bounds checking
fn sample_pixel(x: i32, y: i32) -> vec4<f32> {
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);

    // Clamp coordinates to image bounds
    let clamped_x = clamp(x, 0, width - 1);
    let clamped_y = clamp(y, 0, height - 1);

    let index = u32(clamped_y * width + clamped_x);
    return unpack_rgba8(input_buffer[index]);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let radius = i32(uniforms.radius);
    let sigma = uniforms.sigma;

    // If radius is 0 or sigma is too small, just copy the input
    if (radius <= 0 || sigma <= 0.001) {
        let index = u32(y * i32(uniforms.width) + x);
        output_buffer[index] = input_buffer[index];
        return;
    }

    var accumulated_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var total_weight = 0.0;

    // Sample in a square around the current pixel
    for (var dy = -radius; dy <= radius; dy++) {
        for (var dx = -radius; dx <= radius; dx++) {
            let sample_x = x + dx;
            let sample_y = y + dy;

            // Calculate distance from center
            let distance = sqrt(f32(dx * dx + dy * dy));

            // Skip samples outside the circular radius
            if (distance > uniforms.radius) {
                continue;
            }

            // Calculate gaussian weight
            let weight = gaussian_weight(distance, sigma);

            // Sample the pixel
            let sample_color = sample_pixel(sample_x, sample_y);

            // Accumulate weighted color
            accumulated_color += sample_color * weight;
            total_weight += weight;
        }
    }

    // Normalize by total weight to preserve brightness
    var final_color = accumulated_color;
    if (total_weight > 0.001) {
        final_color = accumulated_color / total_weight;
    }

    // Ensure alpha channel is preserved from original pixel
    let original_color = sample_pixel(x, y);
    final_color.a = original_color.a;

    // Write output pixel
    let index = u32(y * i32(uniforms.width) + x);
    output_buffer[index] = pack_rgba8(final_color);
}
