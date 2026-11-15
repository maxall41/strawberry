fn hessian_eigenvalues(x: i32, y: i32) -> vec2<f32> {
    let center = sample_luminance(x, y);
    let dxx = sample_luminance(x + 1, y) - 2.0 * center + sample_luminance(x - 1, y);
    let dyy = sample_luminance(x, y + 1) - 2.0 * center + sample_luminance(x, y - 1);
    let dxy = 0.25 * (
        sample_luminance(x + 1, y + 1) - sample_luminance(x + 1, y - 1)
        - sample_luminance(x - 1, y + 1) + sample_luminance(x - 1, y - 1)
    );
    let mean = 0.5 * (dxx + dyy);
    let half_difference = 0.5 * (dxx - dyy);
    let spread = sqrt(half_difference * half_difference + dxy * dxy);
    return vec2<f32>(mean + spread, mean - spread);
}
