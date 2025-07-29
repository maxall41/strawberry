struct ColorspaceUniforms {
    width: u32,
    height: u32,
}

@group(0) @binding(0) var<uniform> uniforms: ColorspaceUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;

// Convert BGRA8 packed u32 to vec4<f32>
fn unpack_bgra8(packed: u32) -> vec4<f32> {
    let b = f32((packed >> 0u) & 0xFFu) / 255.0;   // Blue in lowest bits
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;   // Green
    let r = f32((packed >> 16u) & 0xFFu) / 255.0;  // Red
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;  // Alpha in highest bits
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

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check
    if (x >= uniforms.width || y >= uniforms.height) {
        return;
    }

    let pixel_index = y * uniforms.width + x;

    // Read BGRA pixel and convert to RGBA
    let bgra_pixel = input_buffer[pixel_index];
    let color = unpack_bgra8(bgra_pixel);
    let rgba_pixel = pack_rgba8(color);

    // Write converted pixel
    output_buffer[pixel_index] = rgba_pixel;
}
