use anyhow::Result;
use image::RgbaImage;

#[derive(Debug, Clone)]
pub struct TileConfig {
    pub max_tile_pixels: usize,
    pub overlap: usize,
}

impl Default for TileConfig {
    fn default() -> Self {
        const FALLBACK_BUFFER_SIZE: u64 = 134217728; // 128MB fallback
        const BYTES_PER_PIXEL_WORST_CASE: u64 = 8; // vec2<i32> in watershed
        const MAX_PIXELS: usize = (FALLBACK_BUFFER_SIZE / BYTES_PER_PIXEL_WORST_CASE) as usize;

        Self {
            max_tile_pixels: MAX_PIXELS,
            overlap: 16,
        }
    }
}

impl TileConfig {
    pub fn from_limits(limits: &wgpu::Limits) -> Self {
        const BYTES_PER_PIXEL_WORST_CASE: u64 = 8; // vec2<i32> in watershed
        let max_buffer_size = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let max_pixels = (max_buffer_size / BYTES_PER_PIXEL_WORST_CASE) as usize;

        Self {
            max_tile_pixels: max_pixels,
            overlap: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TileInfo {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub core_x: u32,
    pub core_y: u32,
    pub core_width: u32,
    pub core_height: u32,
}

#[derive(Debug)]
pub struct TileGrid {
    pub tiles: Vec<TileInfo>,
}

impl TileGrid {
    pub fn new(width: u32, height: u32, config: TileConfig) -> Self {
        let tiles = Self::calculate_tiles(width, height, &config);
        Self { tiles }
    }

    fn calculate_tiles(width: u32, height: u32, config: &TileConfig) -> Vec<TileInfo> {
        let mut tiles = Vec::new();
        let total_pixels = (width as usize) * (height as usize);

        if total_pixels <= config.max_tile_pixels {
            tiles.push(TileInfo {
                x: 0,
                y: 0,
                width,
                height,
                core_x: 0,
                core_y: 0,
                core_width: width,
                core_height: height,
            });
            return tiles;
        }

        let ideal_tile_side = (config.max_tile_pixels as f32).sqrt() as u32;
        let tile_width = ideal_tile_side.min(width);
        let tile_height = ideal_tile_side.min(height);
        let overlap = config.overlap as u32;

        let mut y = 0;
        while y < height {
            let mut x = 0;
            while x < width {
                let tile_start_x = x.saturating_sub(overlap);
                let tile_start_y = y.saturating_sub(overlap);
                let tile_end_x = (x + tile_width + overlap).min(width);
                let tile_end_y = (y + tile_height + overlap).min(height);

                let actual_tile_width = tile_end_x - tile_start_x;
                let actual_tile_height = tile_end_y - tile_start_y;

                let core_end_x = (x + tile_width).min(width);
                let core_end_y = (y + tile_height).min(height);
                let core_width = core_end_x - x;
                let core_height = core_end_y - y;

                tiles.push(TileInfo {
                    x: tile_start_x,
                    y: tile_start_y,
                    width: actual_tile_width,
                    height: actual_tile_height,
                    core_x: x,
                    core_y: y,
                    core_width,
                    core_height,
                });

                x += tile_width;
            }
            y += tile_height;
        }

        tiles
    }

    pub fn needs_tiling(width: u32, height: u32, config: &TileConfig) -> bool {
        let total_pixels = (width as usize) * (height as usize);
        total_pixels > config.max_tile_pixels
    }
}

pub fn extract_rgba_tile(image: &RgbaImage, tile: &TileInfo) -> RgbaImage {
    let mut tile_data = Vec::with_capacity((tile.width * tile.height * 4) as usize);

    for y in tile.y..tile.y + tile.height {
        for x in tile.x..tile.x + tile.width {
            let pixel = if x < image.width() && y < image.height() {
                *image.get_pixel(x, y)
            } else {
                image::Rgba([0, 0, 0, 255])
            };
            tile_data.extend_from_slice(&pixel.0);
        }
    }

    RgbaImage::from_raw(tile.width, tile.height, tile_data).expect("Failed to create tile image")
}

pub fn merge_rgba_tiles(
    tiles: &[(TileInfo, RgbaImage)],
    full_width: u32,
    full_height: u32,
) -> Result<RgbaImage> {
    let mut result_data = vec![0u8; (full_width * full_height * 4) as usize];

    for (tile_info, tile_image) in tiles {
        for y in 0..tile_info.core_height {
            for x in 0..tile_info.core_width {
                let src_x = (tile_info.core_x - tile_info.x) + x;
                let src_y = (tile_info.core_y - tile_info.y) + y;
                let dst_x = tile_info.core_x + x;
                let dst_y = tile_info.core_y + y;

                if dst_x < full_width && dst_y < full_height {
                    let src_pixel = tile_image.get_pixel(src_x, src_y);
                    let dst_idx = ((dst_y * full_width + dst_x) * 4) as usize;
                    result_data[dst_idx..dst_idx + 4].copy_from_slice(&src_pixel.0);
                }
            }
        }
    }

    RgbaImage::from_raw(full_width, full_height, result_data)
        .ok_or_else(|| anyhow::anyhow!("Failed to create merged RGBA image"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tile_config_from_limits() {
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: 268435456, // 256MB
            ..Default::default()
        };

        let config = TileConfig::from_limits(&limits);

        // With 8 bytes per pixel worst case, 256MB should allow ~33M pixels
        let expected_pixels = 268435456 / 8;
        assert_eq!(config.max_tile_pixels, expected_pixels as usize);
        assert_eq!(config.overlap, 16);
    }

    #[test]
    fn test_tile_config_small_limits() {
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: 67108864, // 64MB
            ..Default::default()
        };

        let config = TileConfig::from_limits(&limits);

        // With 8 bytes per pixel worst case, 64MB should allow ~8M pixels
        let expected_pixels = 67108864 / 8;
        assert_eq!(config.max_tile_pixels, expected_pixels as usize);
    }

    #[test]
    fn test_needs_tiling_with_custom_limits() {
        let config = TileConfig {
            max_tile_pixels: 1000000, // 1M pixels
            overlap: 16,
        };

        // Small image shouldn't need tiling
        assert!(!TileGrid::needs_tiling(800, 600, &config)); // 480k pixels

        // Large image should need tiling
        assert!(TileGrid::needs_tiling(2000, 2000, &config)); // 4M pixels
    }
}
