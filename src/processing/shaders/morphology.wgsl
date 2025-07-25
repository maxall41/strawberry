struct MorphologyUniforms {
    width: u32,
    height: u32,
    operation: u32,
    element_size: u32,
}

@group(0) @binding(0) var<uniform> uniforms: MorphologyUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;
@group(0) @binding(3) var<storage, read_write> temp_buffer: array<u32>;

// Morphological operations
const OPERATION_DILATION: u32 = 0u;
const OPERATION_EROSION: u32 = 1u;
const OPERATION_OPENING: u32 = 2u;
const OPERATION_CLOSING: u32 = 3u;

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

// Sample pixel with bounds checking from input buffer
fn sample_pixel_input(x: i32, y: i32) -> vec4<f32> {
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);

    // Clamp coordinates to image bounds
    let clamped_x = clamp(x, 0, width - 1);
    let clamped_y = clamp(y, 0, height - 1);

    let index = u32(clamped_y * width + clamped_x);
    return unpack_rgba8(input_buffer[index]);
}

// Sample pixel with bounds checking from temp buffer
fn sample_pixel_temp(x: i32, y: i32) -> vec4<f32> {
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);

    // Clamp coordinates to image bounds
    let clamped_x = clamp(x, 0, width - 1);
    let clamped_y = clamp(y, 0, height - 1);

    let index = u32(clamped_y * width + clamped_x);
    return unpack_rgba8(temp_buffer[index]);
}

// Perform morphological operation on a neighborhood
fn apply_morphology_operation(center_x: i32, center_y: i32, operation: u32, use_temp: bool) -> vec4<f32> {
    let element_radius = i32(uniforms.element_size) / 2;

    // Initialize result based on operation type
    var result_luma: f32;
    if (operation == OPERATION_DILATION || operation == OPERATION_CLOSING) {
        result_luma = 0.0; // Start with minimum for dilation
    } else {
        result_luma = 1.0; // Start with maximum for erosion
    }

    var result_pixel: vec4<f32>;

    // Iterate through structuring element (square neighborhood)
    for (var dy = -element_radius; dy <= element_radius; dy++) {
        for (var dx = -element_radius; dx <= element_radius; dx++) {
            let sample_x = center_x + dx;
            let sample_y = center_y + dy;

            // Sample from appropriate buffer
            var sample_pixel: vec4<f32>;
            if (use_temp) {
                sample_pixel = sample_pixel_temp(sample_x, sample_y);
            } else {
                sample_pixel = sample_pixel_input(sample_x, sample_y);
            }

            let sample_luma = luminance(sample_pixel.rgb);

            // Apply morphological operation
            if (operation == OPERATION_DILATION || operation == OPERATION_CLOSING) {
                // Dilation: take maximum
                if (sample_luma > result_luma) {
                    result_luma = sample_luma;
                    result_pixel = sample_pixel;
                }
            } else {
                // Erosion: take minimum
                if (sample_luma < result_luma) {
                    result_luma = sample_luma;
                    result_pixel = sample_pixel;
                }
            }
        }
    }

    // If no pixel was selected (shouldn't happen), use center pixel
    if (operation == OPERATION_DILATION || operation == OPERATION_CLOSING) {
        if (result_luma == 0.0) {
            if (use_temp) {
                result_pixel = sample_pixel_temp(center_x, center_y);
            } else {
                result_pixel = sample_pixel_input(center_x, center_y);
            }
        }
    } else {
        if (result_luma == 1.0) {
            if (use_temp) {
                result_pixel = sample_pixel_temp(center_x, center_y);
            } else {
                result_pixel = sample_pixel_input(center_x, center_y);
            }
        }
    }

    return result_pixel;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let operation = uniforms.operation;
    var result_pixel: vec4<f32>;

    if (operation == OPERATION_DILATION || operation == OPERATION_EROSION) {
        // Single-pass operations: work directly from input to output
        result_pixel = apply_morphology_operation(x, y, operation, false);
    } else if (operation == OPERATION_OPENING) {
        // Opening: First pass is erosion (input -> temp)
        result_pixel = apply_morphology_operation(x, y, OPERATION_EROSION, false);
    } else if (operation == OPERATION_CLOSING) {
        // Closing: First pass is dilation (input -> temp)
        result_pixel = apply_morphology_operation(x, y, OPERATION_DILATION, false);
    } else {
        // Unknown operation, just copy input
        result_pixel = sample_pixel_input(x, y);
    }

    // Write to appropriate buffer
    let index = u32(y * i32(uniforms.width) + x);
    if (operation == OPERATION_DILATION || operation == OPERATION_EROSION) {
        // Single-pass: write to output
        output_buffer[index] = pack_rgba8(result_pixel);
    } else {
        // Two-pass: write to temp buffer
        temp_buffer[index] = pack_rgba8(result_pixel);
    }
}

@compute @workgroup_size(8, 8)
fn second_pass(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let operation = uniforms.operation;
    var result_pixel: vec4<f32>;

    if (operation == OPERATION_OPENING) {
        // Opening: Second pass is dilation (temp -> output)
        result_pixel = apply_morphology_operation(x, y, OPERATION_DILATION, true);
    } else if (operation == OPERATION_CLOSING) {
        // Closing: Second pass is erosion (temp -> output)
        result_pixel = apply_morphology_operation(x, y, OPERATION_EROSION, true);
    } else {
        // This shouldn't happen for single-pass operations, but just copy temp
        result_pixel = sample_pixel_temp(x, y);
    }

    // Write to output buffer
    let index = u32(y * i32(uniforms.width) + x);
    output_buffer[index] = pack_rgba8(result_pixel);
}
