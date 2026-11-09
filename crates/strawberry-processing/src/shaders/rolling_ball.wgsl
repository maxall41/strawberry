struct RollingBallUniforms {
    width: u32,
    height: u32,
    ball_radius: u32,
    padding: u32,
}

@group(0) @binding(0) var<uniform> uniforms: RollingBallUniforms;
@group(0) @binding(1) var<storage, read_write> output_buffer: array<u32>;
@group(0) @binding(2) var<storage, read> input_buffer: array<u32>;
@group(0) @binding(3) var<storage, read_write> eroded_buffer: array<f32>;

fn unpack_bgra8(packed: u32) -> vec4<f32> {
    let b = f32((packed >> 0u) & 0xFFu) / 255.0;
    let g = f32((packed >> 8u) & 0xFFu) / 255.0;
    let r = f32((packed >> 16u) & 0xFFu) / 255.0;
    let a = f32((packed >> 24u) & 0xFFu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

fn pack_bgra8(color: vec4<f32>) -> u32 {
    let r = u32(clamp(color.r * 255.0, 0.0, 255.0));
    let g = u32(clamp(color.g * 255.0, 0.0, 255.0));
    let b = u32(clamp(color.b * 255.0, 0.0, 255.0));
    let a = u32(clamp(color.a * 255.0, 0.0, 255.0));
    return b | (g << 8u) | (r << 16u) | (a << 24u);
}

fn luminance(color: vec3<f32>) -> f32 {
    return 0.299 * color.r + 0.587 * color.g + 0.114 * color.b;
}

fn clamped_index(x: i32, y: i32) -> u32 {
    let width = i32(uniforms.width);
    let height = i32(uniforms.height);
    let clamped_x = clamp(x, 0, width - 1);
    let clamped_y = clamp(y, 0, height - 1);
    return u32(clamped_y * width + clamped_x);
}

fn sample_pixel(x: i32, y: i32) -> vec4<f32> {
    return unpack_bgra8(input_buffer[clamped_index(x, y)]);
}

fn sample_luminance(x: i32, y: i32) -> f32 {
    return luminance(sample_pixel(x, y).rgb);
}

fn sample_eroded(x: i32, y: i32) -> f32 {
    return eroded_buffer[clamped_index(x, y)];
}

// Upper hemisphere in 8-bit intensity units. Together, erosion by this surface
// followed by dilation implements a non-flat spherical morphological opening:
// the envelope traced by a ball rolling under the image intensity surface.
fn ball_height(dx: i32, dy: i32, radius: i32) -> f32 {
    let fx = f32(dx);
    let fy = f32(dy);
    let fr = f32(radius);
    return sqrt(max(0.0, fr * fr - fx * fx - fy * fy)) / 255.0;
}

@compute @workgroup_size(8, 8)
fn erosion_pass(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let index = u32(y * i32(uniforms.width) + x);
    let radius = i32(uniforms.ball_radius);
    if (radius <= 0) {
        eroded_buffer[index] = sample_luminance(x, y);
        return;
    }

    let radius_squared = radius * radius;
    var eroded = 1.0;
    for (var dy = -radius; dy <= radius; dy++) {
        for (var dx = -radius; dx <= radius; dx++) {
            if (dx * dx + dy * dy > radius_squared) {
                continue;
            }
            eroded = min(
                eroded,
                sample_luminance(x + dx, y + dy) - ball_height(dx, dy, radius),
            );
        }
    }

    eroded_buffer[index] = eroded;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);

    if (x >= i32(uniforms.width) || y >= i32(uniforms.height)) {
        return;
    }

    let original_pixel = sample_pixel(x, y);
    let original_luma = luminance(original_pixel.rgb);
    let radius = i32(uniforms.ball_radius);

    var background_luma = original_luma;
    if (radius > 0) {
        let radius_squared = radius * radius;
        background_luma = -1.0;
        for (var dy = -radius; dy <= radius; dy++) {
            for (var dx = -radius; dx <= radius; dx++) {
                if (dx * dx + dy * dy > radius_squared) {
                    continue;
                }
                background_luma = max(
                    background_luma,
                    sample_eroded(x + dx, y + dy) + ball_height(dx, dy, radius),
                );
            }
        }
        background_luma = clamp(background_luma, 0.0, 1.0);
    }

    let corrected_luma = max(0.0, original_luma - background_luma);
    var output_pixel = vec4<f32>(0.0, 0.0, 0.0, original_pixel.a);
    if (original_luma > 0.001) {
        let scale_factor = corrected_luma / original_luma;
        output_pixel = vec4<f32>(original_pixel.rgb * scale_factor, original_pixel.a);
    }

    let index = u32(y * i32(uniforms.width) + x);
    output_buffer[index] = pack_bgra8(clamp(output_pixel, vec4<f32>(0.0), vec4<f32>(1.0)));
}
