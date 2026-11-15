fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    let two_sigma_squared = 2.0 * sigma * sigma;
    return exp(-(distance * distance) / two_sigma_squared) / (3.14159265359 * two_sigma_squared);
}
