struct RollingBallUniforms {
    width: u32,
    height: u32,
    ball_radius: u32,
    padding: u32,
}

@group(0) @binding(0) var<uniform> uniforms: RollingBallUniforms;
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

// Calculate luminance using standard weights
fn luminance(color: vec3<f32>) -> f32 {
    return 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
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

// Get luminance at specific coordinates with bounds checking
fn sample_luminance(x: i32, y: i32) -> f32 {
    let pixel = sample_pixel(x, y);
    return luminance(pixel.rgb);
}

// Find minimum luminance in circular neighborhood (rolling ball background)
fn find_rolling_ball_background(center_x: i32, center_y: i32) -> f32 {
    let radius = i32(uniforms.ball_radius);

    // If radius is 0, return the center pixel luminance
    if (radius <= 0) {
        return sample_luminance(center_x, center_y);
    }

    var min_luminance = 1.0; // Start with maximum possible luminance
    let radius_squared = radius * radius;

    // Search in square bounding box and filter by circular distance
    for (var dy = -radius; dy <= radius; dy++) {
        for (var dx = -radius; dx <= radius; dx++) {
            // Check if point is within circle
            let distance_squared = dx * dx + dy * dy;
            if (distance_squared <= radius_squared) {
                let sample_x = center_x + dx;
                let sample_y = center_y + dy;

                let sample_luma = sample_luminance(sample_x, sample_y);
                min_luminance = min(min_luminance, sample_luma);
            }
        }
    }

    return min_luminance;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    // Get the original pixel
    let original_pixel = sample_pixel(x, y);
    let original_luma = luminance(original_pixel.rgb);

    // Find the rolling ball background (minimum in circular neighborhood)
    let background_luma = find_rolling_ball_background(x, y);

    // Subtract background from original luminance
    let corrected_luma = max(0.0, original_luma - background_luma);

    // Calculate the scaling factor to preserve color ratios
    var output_pixel: vec4<f32>;
    if (original_luma > 0.001) {
        let scale_factor = corrected_luma / original_luma;
        output_pixel = vec4<f32>(
            original_pixel.r * scale_factor,
            original_pixel.g * scale_factor,
            original_pixel.b * scale_factor,
            original_pixel.a
        );
    } else {
        // If original luminance is near zero, output black
        output_pixel = vec4<f32>(0.0, 0.0, 0.0, original_pixel.a);
    }

    // Ensure output values are in valid range
    output_pixel = clamp(output_pixel, vec4<f32>(0.0), vec4<f32>(1.0, 1.0, 1.0, 1.0));

    // Write output pixel
    let index = u32(y * i32(uniforms.width) + x);
    output_buffer[index] = pack_rgba8(output_pixel);
}
