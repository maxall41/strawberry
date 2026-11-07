struct ThresholdUniforms {
    width: u32,
    height: u32,
    threshold: f32,
    padding: f32,
}

@group(0) @binding(0) var<uniform> uniforms: ThresholdUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;

// Convert the pipeline's internal BGRA8 packed u32 to vec4<f32>.
fn unpack_bgra8(packed: u32) -> vec4<f32> {
    let b = f32((packed >> 0u) & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let r = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Convert vec4<f32> to the pipeline's internal BGRA8 representation.
fn pack_bgra8(color: vec4<f32>) -> u32 {
    let r = u32(clamp(color.r * 255.0, 0.0, 255.0));
    let g = u32(clamp(color.g * 255.0, 0.0, 255.0));
    let b = u32(clamp(color.b * 255.0, 0.0, 255.0));
    let a = u32(clamp(color.a * 255.0, 0.0, 255.0));
    return b | (g << 8u) | (r << 16u) | (a << 24u);
}

// Calculate luminance using standard weights
fn luminance(color: vec3<f32>) -> f32 {
    return 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
}

// Apply threshold operation
fn apply_threshold(color: vec4<f32>, threshold_value: f32) -> vec4<f32> {
    // Calculate luminance of the pixel
    let luma = luminance(color.rgb);

    // Apply threshold: if luminance >= threshold, output white (1.0), else black (0.0)
    let threshold_result = select(0.0, 1.0, luma >= threshold_value);

    // Return grayscale result with original alpha
    return vec4<f32>(threshold_result, threshold_result, threshold_result, color.a);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Check bounds
    if (x >= uniforms.width || y >= uniforms.height) {
        return;
    }

    let index = y * uniforms.width + x;

    // Read input pixel
    let input_pixel = unpack_bgra8(input_buffer[index]);

    // Apply threshold operation
    let output_pixel = apply_threshold(input_pixel, uniforms.threshold);

    // Write output pixel
    output_buffer[index] = pack_bgra8(output_pixel);
}
