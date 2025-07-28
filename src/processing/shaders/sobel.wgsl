struct SobelUniforms {
    width: u32,
    height: u32,
    threshold: f32,
    padding: f32,
}

@group(0) @binding(0) var<uniform> uniforms: SobelUniforms;
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

// Apply Sobel edge detection
fn apply_sobel(center_x: i32, center_y: i32) -> f32 {
    // Sobel X kernel:        Sobel Y kernel:
    // -1  0  1               -1 -2 -1
    // -2  0  2                0  0  0
    // -1  0  1                1  2  1

    // Sample 3x3 neighborhood
    let p00 = sample_luminance(center_x - 1, center_y - 1);
    let p01 = sample_luminance(center_x,     center_y - 1);
    let p02 = sample_luminance(center_x + 1, center_y - 1);
    let p10 = sample_luminance(center_x - 1, center_y);
    let p12 = sample_luminance(center_x + 1, center_y);
    let p20 = sample_luminance(center_x - 1, center_y + 1);
    let p21 = sample_luminance(center_x,     center_y + 1);
    let p22 = sample_luminance(center_x + 1, center_y + 1);

    // Apply Sobel X kernel
    let gx = (-1.0 * p00) + (0.0 * p01) + (1.0 * p02) +
             (-2.0 * p10) + (0.0 * 0.0)  + (2.0 * p12) +
             (-1.0 * p20) + (0.0 * p21) + (1.0 * p22);

    // Apply Sobel Y kernel
    let gy = (-1.0 * p00) + (-2.0 * p01) + (-1.0 * p02) +
             (0.0 * p10)  + (0.0 * 0.0)   + (0.0 * p12) +
             (1.0 * p20)  + (2.0 * p21)  + (1.0 * p22);

    // Calculate gradient magnitude
    let magnitude = sqrt(gx * gx + gy * gy);

    // Normalize and apply threshold
    let normalized_magnitude = clamp(magnitude, 0.0, 1.0);

    // Apply threshold if specified
    if (uniforms.threshold > 0.0) {
        return select(0.0, normalized_magnitude, normalized_magnitude >= uniforms.threshold);
    } else {
        return normalized_magnitude;
    }
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    // Get the current pixel for alpha channel
    let current_pixel = sample_pixel(x, y);

    // Apply Sobel edge detection
    let edge_intensity = apply_sobel(x, y);

    // Create output pixel (grayscale edge map)
    let output_pixel = vec4<f32>(edge_intensity, edge_intensity, edge_intensity, current_pixel.a);

    // Write output pixel
    let index = u32(y * i32(uniforms.width) + x);
    output_buffer[index] = pack_rgba8(output_pixel);
}
