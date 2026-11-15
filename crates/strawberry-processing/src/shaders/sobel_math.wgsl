fn sobel_magnitude(center_x: i32, center_y: i32) -> f32 {
    // Sobel X kernel:        Sobel Y kernel:
    // -1  0  1               -1 -2 -1
    // -2  0  2                0  0  0
    // -1  0  1                1  2  1

    // Sample 3x3 neighborhood
    let p00 = sample_luminance(center_x - 1, center_y - 1);
    let p01 = sample_luminance(center_x,     center_y - 1);
    let p02 = sample_luminance(center_x + 1, center_y - 1);
    let p10 = sample_luminance(center_x - 1, center_y);
    let p12 = sample_luminance(center_x + 1, center_y);
    let p20 = sample_luminance(center_x - 1, center_y + 1);
    let p21 = sample_luminance(center_x,     center_y + 1);
    let p22 = sample_luminance(center_x + 1, center_y + 1);

    // Apply Sobel X kernel
    let gx = (-1.0 * p00) + (0.0 * p01) + (1.0 * p02) +
             (-2.0 * p10) + (0.0 * 0.0)  + (2.0 * p12) +
             (-1.0 * p20) + (0.0 * p21) + (1.0 * p22);

    // Apply Sobel Y kernel
    let gy = (-1.0 * p00) + (-2.0 * p01) + (-1.0 * p02) +
             (0.0 * p10)  + (0.0 * 0.0)   + (0.0 * p12) +
             (1.0 * p20)  + (2.0 * p21)  + (1.0 * p22);

    // Calculate gradient magnitude
    let magnitude = sqrt(gx * gx + gy * gy);

    return magnitude;
}
