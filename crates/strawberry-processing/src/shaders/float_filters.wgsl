struct Params {
    width: u32,
    height: u32,
    operation: u32,
    channel: u32,
    sigma: f32,
    radius: f32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var<uniform> uniforms: Params;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<storage, read> input: array<u32>;
@group(0) @binding(3) var<storage, read> second: array<f32>;

fn sample_luminance(x: i32, y: i32) -> f32 {
    let px = u32(clamp(x, 0, i32(uniforms.width) - 1));
    let py = u32(clamp(y, 0, i32(uniforms.height) - 1));
    return bitcast<f32>(input[py * uniforms.width + px]);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= uniforms.width || id.y >= uniforms.height {
        return;
    }
    let index = id.y * uniforms.width + id.x;
    if uniforms.operation == 0u {
        let packed = input[index];
        let b = f32(packed & 255u) / 255.0;
        let g = f32((packed >> 8u) & 255u) / 255.0;
        let r = f32((packed >> 16u) & 255u) / 255.0;
        var value = 0.299 * r + 0.587 * g + 0.114 * b;
        if uniforms.channel == 1u {
            value = r;
        }
        if uniforms.channel == 2u {
            value = g;
        }
        if uniforms.channel == 3u {
            value = b;
        }
        output[index] = value;
    } else if uniforms.operation == 1u {
        var total = 0.0;
        var weight_sum = 0.0;
        let radius = i32(uniforms.radius);
        for (var dy = -radius; dy <= radius; dy++) {
            for (var dx = -radius; dx <= radius; dx++) {
                let distance = sqrt(f32(dx * dx + dy * dy));
                if distance <= uniforms.radius {
                    let weight = gaussian_weight(distance, uniforms.sigma);
                    total += sample_luminance(i32(id.x) + dx, i32(id.y) + dy) * weight;
                    weight_sum += weight;
                }
            }
        }
        output[index] = total / weight_sum;
    } else if uniforms.operation == 2u {
        output[index] = sobel_magnitude(i32(id.x), i32(id.y));
    } else if uniforms.operation == 3u {
        output[index] = difference_of_gaussians(bitcast<f32>(input[index]), second[index]);
    } else if uniforms.operation == 4u {
        output[index] = hessian_eigenvalues(i32(id.x), i32(id.y))[uniforms.channel];
    }
}
