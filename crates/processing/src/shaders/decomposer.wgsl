struct DecomposerUniforms {
    width: u32,
    height: u32,
    _padding1: u32,
    _padding2: u32,
}

@group(0) @binding(0) var<uniform> uniforms: DecomposerUniforms;
@group(0) @binding(1) var<storage, read_write> red_output: array<u32>;
@group(0) @binding(2) var<storage, read_write> green_output: array<u32>;
@group(0) @binding(3) var<storage, read_write> blue_output: array<u32>;
@group(0) @binding(4) var<storage, read> input_buffer: array<u32>;

// Convert the pipeline's internal BGRA8 packed u32 to vec4<f32>.
fn unpack_bgra8(packed: u32) -> vec4<f32> {
    let b = f32((packed >> 0u) & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let r = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Convert vec4<f32> to BGRA8 packed u32
fn pack_bgra8(color: vec4<f32>) -> u32 {
    let r = u32(clamp(color.r * 255.0, 0.0, 255.0));
    let g = u32(clamp(color.g * 255.0, 0.0, 255.0));
    let b = u32(clamp(color.b * 255.0, 0.0, 255.0));
    let a = u32(clamp(color.a * 255.0, 0.0, 255.0));
    return b | (g << 8u) | (r << 16u) | (a << 24u);
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

    // Create decomposed channel images
    // Red channel: keep red, zero out green and blue
    let red_pixel = vec4<f32>(input_pixel.r, 0.0, 0.0, input_pixel.a);

    // Green channel: keep green, zero out red and blue
    let green_pixel = vec4<f32>(0.0, input_pixel.g, 0.0, input_pixel.a);

    // Blue channel: keep blue, zero out red and green
    let blue_pixel = vec4<f32>(0.0, 0.0, input_pixel.b, input_pixel.a);

    // Write all three outputs in BGRA format
    red_output[index] = pack_bgra8(red_pixel);
    green_output[index] = pack_bgra8(green_pixel);
    blue_output[index] = pack_bgra8(blue_pixel);
}
