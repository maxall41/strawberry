struct ContrastUniforms {
    width: u32,
    height: u32,
    min_value: f32,
    max_value: f32,
}

@group(0) @binding(0) var<uniform> uniforms: ContrastUniforms;
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

// Apply contrast remapping to RGB channels
fn apply_contrast(color: vec4<f32>, min_val: f32, max_val: f32) -> vec4<f32> {
    let range = max_val - min_val;

    // Avoid division by zero
    if (range <= 0.0) {
        return color;
    }

    // Apply contrast remapping to RGB channels only, preserve alpha
    var result = color;
    result.r = clamp((color.r - min_val) / range, 0.0, 1.0);
    result.g = clamp((color.g - min_val) / range, 0.0, 1.0);
    result.b = clamp((color.b - min_val) / range, 0.0, 1.0);

    return result;
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
    let input_pixel = unpack_rgba8(input_buffer[index]);

    // Apply contrast remapping
    let output_pixel = apply_contrast(input_pixel, uniforms.min_value, uniforms.max_value);

    // Write output pixel
    output_buffer[index] = pack_rgba8(output_pixel);
}
