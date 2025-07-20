// composite.wgsl - GPU go brrrrr shader

struct Uniforms {
    width: u32,
    height: u32,
    layer_count: u32,
    padding: u32,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;

// Input texture buffers - one per image
@group(0) @binding(2) var<storage, read> input_0: array<u32>;
@group(0) @binding(3) var<storage, read> input_1: array<u32>;
@group(0) @binding(4) var<storage, read> input_2: array<u32>;
@group(0) @binding(5) var<storage, read> input_3: array<u32>;
@group(0) @binding(6) var<storage, read> input_4: array<u32>;
@group(0) @binding(7) var<storage, read> input_5: array<u32>;
@group(0) @binding(8) var<storage, read> input_6: array<u32>;


@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;

    // Bounds check
    if (x >= uniforms.width || y >= uniforms.height) {
        return;
    }

    let pixel_index = y * uniforms.width + x;

    // Initialize with transparent black
    var final_color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    // Composite all layers using additive blending
    // GPU goes BRRRRR with parallel processing!

    for (var layer = 0u; layer < min(uniforms.layer_count, 7u); layer++) {
        var layer_pixel: u32;

        // Use function to get pixel from the right buffer
        layer_pixel = get_pixel_from_layer(layer, pixel_index);

        // Unpack RGBA from u32 (Red, Green, Blue, Alpha)
        let r = f32((layer_pixel >> 0u) & 0xFFu) / 255.0;   // Red in lowest bits
        let g = f32((layer_pixel >> 8u) & 0xFFu) / 255.0;   // Green
        let b = f32((layer_pixel >> 16u) & 0xFFu) / 255.0;  // Blue
        let a = f32((layer_pixel >> 24u) & 0xFFu) / 255.0;  // Alpha in highest bits

        let layer_color = vec4<f32>(r, g, b, a);

        // Additive blending: just add the colors together!
        final_color = additive_blend(final_color, layer_color);
    }

    // Pack back to BGRA u32 format and store
    let r_packed = u32(clamp(final_color.r * 255.0, 0.0, 255.0));
    let g_packed = u32(clamp(final_color.g * 255.0, 0.0, 255.0));
    let b_packed = u32(clamp(final_color.b * 255.0, 0.0, 255.0));
    let a_packed = u32(clamp(final_color.a * 255.0, 0.0, 255.0));

    output_buffer[pixel_index] = b_packed | (g_packed << 8u) | (r_packed << 16u) | (a_packed << 24u);
}

// Helper function to select the right input buffer
fn get_pixel_from_layer(layer: u32, pixel_index: u32) -> u32 {
    switch layer {
        case 0u: { return input_0[pixel_index]; }
        case 1u: { return input_1[pixel_index]; }
        case 2u: { return input_2[pixel_index]; }
        case 3u: { return input_3[pixel_index]; }
        case 4u: { return input_4[pixel_index]; }
        case 5u: { return input_5[pixel_index]; }
        case 6u: { return input_6[pixel_index]; }

        default: { return 0u; }
    }
}

fn additive_blend(bottom: vec4<f32>, top: vec4<f32>) -> vec4<f32> {
    return min(bottom + top, vec4<f32>(1.0, 1.0, 1.0, 1.0));
}
