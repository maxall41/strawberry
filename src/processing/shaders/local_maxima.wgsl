struct LocalMaximaUniforms {
    width: u32,
    height: u32,
    min_distance: u32,
    exclude_border: u32,
    threshold_abs: f32,
    threshold_rel: f32,
    max_intensity: f32,
    padding: f32,
}

@group(0) @binding(0) var<uniform> uniforms: LocalMaximaUniforms;
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

// Check if pixel is within excluded border region
fn is_in_excluded_border(x: i32, y: i32) -> bool {
    if (uniforms.exclude_border == 0u) {
        return false;
    }

    let border_size = i32(uniforms.min_distance);
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);

    return x < border_size || y < border_size ||
           x >= (width - border_size) || y >= (height - border_size);
}

// Apply maximum filter in min_distance neighborhood (scikit-image approach)
fn apply_maximum_filter(center_x: i32, center_y: i32) -> f32 {
    let min_dist = i32(uniforms.min_distance);
    var max_value = 0.0;

    // Search in square neighborhood of size (2 * min_distance + 1)
    for (var dy = -min_dist; dy <= min_dist; dy++) {
        for (var dx = -min_dist; dx <= min_dist; dx++) {
            let sample_x = center_x + dx;
            let sample_y = center_y + dy;

            let sample_luma = sample_luminance(sample_x, sample_y);
            max_value = max(max_value, sample_luma);
        }
    }

    return max_value;
}

// Check if current pixel is a local maximum using scikit-image algorithm
fn is_peak_local_maximum(center_x: i32, center_y: i32, center_luma: f32) -> bool {
    // Calculate effective threshold (max of absolute and relative thresholds)
    let rel_threshold = uniforms.max_intensity * uniforms.threshold_rel;
    let effective_threshold = max(uniforms.threshold_abs, rel_threshold);

    // If the center pixel is below threshold, it can't be a maximum
    if (center_luma < effective_threshold) {
        return false;
    }

    // Check if pixel is in excluded border region
    if (is_in_excluded_border(center_x, center_y)) {
        return false;
    }

    // Apply maximum filter to the neighborhood
    let max_in_neighborhood = apply_maximum_filter(center_x, center_y);

    // Peak is detected if the original value equals the maximum filtered value
    // Use small epsilon for floating point comparison
    let epsilon = 0.001;
    return abs(center_luma - max_in_neighborhood) < epsilon;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    // Get the current pixel
    let current_pixel = sample_pixel(x, y);
    let current_luma = luminance(current_pixel.rgb);

    // Check if this pixel is a local maximum using scikit-image algorithm
    let is_maximum = is_peak_local_maximum(x, y, current_luma);

    // Create output pixel
    var output_pixel: vec4<f32>;
    if (is_maximum) {
        // Local maximum: output white
        output_pixel = vec4<f32>(1.0, 1.0, 1.0, current_pixel.a);
    } else {
        // Not a local maximum: output black
        output_pixel = vec4<f32>(0.0, 0.0, 0.0, current_pixel.a);
    }

    // Write output pixel
    let index = u32(y * i32(uniforms.width) + x);
    output_buffer[index] = pack_rgba8(output_pixel);
}
