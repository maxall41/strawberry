use anyhow::Result;
use image::RgbaImage;

#[derive(Debug, Clone, Copy)]
pub struct TileConfig {
    pub max_tile_pixels: usize,
    pub overlap: usize,
}

impl Default for TileConfig {
    /// Set fallback tile capacity for eight-byte pixels and a sixteen-pixel overlap.
    fn default() -> Self {
        const FALLBACK_BUFFER_SIZE: usize = 134_217_728; // 128MB fallback
        const BYTES_PER_PIXEL_WORST_CASE: usize = 8; // vec2<i32> in watershed
        const MAX_PIXELS: usize = FALLBACK_BUFFER_SIZE / BYTES_PER_PIXEL_WORST_CASE;

        Self {
            max_tile_pixels: MAX_PIXELS,
            overlap: 16,
        }
    }
}

impl TileConfig {
    /// Derive tile capacity from the device storage-buffer and allocation limits.
    #[must_use]
    pub fn from_limits(limits: &wgpu::Limits) -> Self {
        const BYTES_PER_PIXEL_WORST_CASE: u64 = 8; // vec2<i32> in watershed
        let max_buffer_size = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let max_pixels =
            usize::try_from(max_buffer_size / BYTES_PER_PIXEL_WORST_CASE).unwrap_or(usize::MAX);

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
    /// Construct a grid of overlapping tiles covering the image.
    #[must_use]
    pub fn new(width: u32, height: u32, config: &TileConfig) -> Self {
        let tiles = Self::calculate_tiles(width, height, config);
        Self { tiles }
    }

    /// Partition the image into tile cores with clipped overlapping borders.
    fn calculate_tiles(width: u32, height: u32, config: &TileConfig) -> Vec<TileInfo> {
        let mut tiles = Vec::new();
        let total_pixels = u64::from(width).saturating_mul(u64::from(height));

        if total_pixels <= u64::try_from(config.max_tile_pixels).unwrap_or(u64::MAX) {
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

        let ideal_tile_side = u32::try_from(config.max_tile_pixels.isqrt())
            .unwrap_or(u32::MAX)
            .max(1);
        let tile_width = ideal_tile_side.min(width);
        let tile_height = ideal_tile_side.min(height);
        let overlap = u32::try_from(config.overlap).unwrap_or(u32::MAX);

        let mut y = 0;
        while y < height {
            let mut x = 0;
            while x < width {
                let tile_start_x = x.saturating_sub(overlap);
                let tile_start_y = y.saturating_sub(overlap);
                let tile_end_x = (x.saturating_add(tile_width).saturating_add(overlap)).min(width);
                let tile_end_y =
                    (y.saturating_add(tile_height).saturating_add(overlap)).min(height);

                let actual_tile_width = tile_end_x.saturating_sub(tile_start_x);
                let actual_tile_height = tile_end_y.saturating_sub(tile_start_y);

                let core_end_x = (x.saturating_add(tile_width)).min(width);
                let core_end_y = (y.saturating_add(tile_height)).min(height);
                let core_width = core_end_x.saturating_sub(x);
                let core_height = core_end_y.saturating_sub(y);

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

                x = core_end_x;
            }
            y = y.saturating_add(tile_height);
        }

        tiles
    }

    /// Check whether the image pixel count exceeds the configured tile capacity.
    #[must_use]
    pub fn needs_tiling(width: u32, height: u32, config: &TileConfig) -> bool {
        let total_pixels = u64::from(width).saturating_mul(u64::from(height));
        total_pixels > u64::try_from(config.max_tile_pixels).unwrap_or(u64::MAX)
    }
}

/// Extracts a tile, filling pixels outside the source image with opaque black.
///
/// # Errors
/// Returns an error if the tile dimensions overflow or cannot form an RGBA image.
pub fn extract_rgba_tile(image: &RgbaImage, tile: &TileInfo) -> Result<RgbaImage> {
    let bytes = usize::try_from(crate::buffer_size(
        crate::pixel_count(tile.width, tile.height)?,
        4,
    )?)?;
    let mut data = Vec::new();
    data.try_reserve_exact(bytes)?;
    for y in 0..tile.height {
        for x in 0..tile.width {
            let pixel = tile
                .x
                .checked_add(x)
                .zip(tile.y.checked_add(y))
                .and_then(|(x, y)| image.get_pixel_checked(x, y))
                .copied()
                .unwrap_or(image::Rgba([0, 0, 0, 255]));
            data.extend_from_slice(&pixel.0);
        }
    }
    RgbaImage::from_raw(tile.width, tile.height, data)
        .ok_or_else(|| anyhow::anyhow!("Failed to create tile image"))
}

/// Merges the core regions of processed tiles into an image.
///
/// # Errors
/// Returns an error if image sizes overflow or a tile core lies outside its tile.
pub fn merge_rgba_tiles(
    tiles: &[(TileInfo, RgbaImage)],
    full_width: u32,
    full_height: u32,
) -> Result<RgbaImage> {
    let bytes = usize::try_from(crate::buffer_size(
        crate::pixel_count(full_width, full_height)?,
        4,
    )?)?;
    let mut data = Vec::new();
    data.try_reserve_exact(bytes)?;
    data.resize(bytes, 0);
    let mut result = RgbaImage::from_raw(full_width, full_height, data)
        .ok_or_else(|| anyhow::anyhow!("Failed to create merged RGBA image"))?;
    for (tile, image) in tiles {
        let offset_x = tile
            .core_x
            .checked_sub(tile.x)
            .ok_or_else(|| anyhow::anyhow!("Tile core starts before tile"))?;
        let offset_y = tile
            .core_y
            .checked_sub(tile.y)
            .ok_or_else(|| anyhow::anyhow!("Tile core starts before tile"))?;
        for y in 0..tile.core_height {
            for x in 0..tile.core_width {
                let src_x = offset_x
                    .checked_add(x)
                    .ok_or_else(|| anyhow::anyhow!("Tile coordinate overflow"))?;
                let src_y = offset_y
                    .checked_add(y)
                    .ok_or_else(|| anyhow::anyhow!("Tile coordinate overflow"))?;
                let pixel = image
                    .get_pixel_checked(src_x, src_y)
                    .ok_or_else(|| anyhow::anyhow!("Tile core lies outside tile image"))?;
                if let Some(dst) = tile
                    .core_x
                    .checked_add(x)
                    .zip(tile.core_y.checked_add(y))
                    .and_then(|(x, y)| result.get_pixel_mut_checked(x, y))
                {
                    *dst = *pixel;
                }
            }
        }
    }
    Ok(result)
}
