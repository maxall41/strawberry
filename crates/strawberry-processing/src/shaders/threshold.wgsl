struct ThresholdUniforms {
    width: u32,
    height: u32,
    threshold: u32,
    padding: u32,
}

@group(0) @binding(0) var<uniform> uniforms: ThresholdUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;

fn apply_threshold(packed: u32, threshold_value: u32) -> u32 {
    // Match the CPU histogram's integer BT.601 luminance for the pipeline's BGRA8 pixels.
    let b = packed & 0xFFu;
    let g = (packed >> 8u) & 0xFFu;
    let r = (packed >> 16u) & 0xFFu;
    let level = (299u * r + 587u * g + 114u * b) / 1000u;

    // The cutoff is the last background bin in both manual and automatic modes.
    let grayscale = select(0u, 0x00FFFFFFu, level > threshold_value);
    return (packed & 0xFF000000u) | grayscale;
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
    let input_pixel = input_buffer[index];

    // Apply threshold operation
    let output_pixel = apply_threshold(input_pixel, uniforms.threshold);

    // Write output pixel
    output_buffer[index] = output_pixel;
}
