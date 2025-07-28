struct GrayscaleUniforms {
    width: u32,
    height: u32,
}

@group(0) @binding(0) var<uniform> uniforms: GrayscaleUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;

// Convert RGBA8 packed u32 to vec4<f32> (ABGR format)
// Convert BGRA8 packed u32 to vec4<f32> (note: format is BGRA, not RGBA)
fn unpack_rgba8(packed: u32) -> vec4<f32> {
    let b = f32((packed >> 0u) & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let r = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

// Convert vec4<f32> to BGRA8 packed u32 (note: format is BGRA, not RGBA)
fn pack_rgba8(color: vec4<f32>) -> u32 {
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

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    // Check bounds
    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    // Get input pixel
    let index = u32(y * i32(uniforms.width) + x);
    let input_pixel = unpack_rgba8(input_buffer[index]);

    // Calculate grayscale value
    let gray_value = luminance(input_pixel.rgb);

    // Create output pixel with grayscale value in all RGB channels
    let output_pixel = vec4<f32>(gray_value, gray_value, gray_value, input_pixel.a);

    // Write output pixel
    output_buffer[index] = pack_rgba8(output_pixel);
}
