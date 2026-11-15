use std::sync::Arc;

use anyhow::{Result, ensure};
use image::RgbaImage;

use crate::{
    GpuImageProcessor,
    float_filters::GpuFloatFilters,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

pub struct GpuDifferenceOfGaussians {
    filters: GpuFloatFilters,
    tile_config: TileConfig,
}

impl GpuDifferenceOfGaussians {
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        Self {
            tile_config: TileConfig::from_limits(&gpu.device.limits()),
            filters: GpuFloatFilters::new(gpu),
        }
    }

    /// Apply small Gaussian minus large Gaussian to internal BGRA luminance.
    /// Negative responses are clipped to black for the unsigned image pipeline.
    ///
    /// # Errors
    /// Returns an error for invalid sigmas, empty images, invalid tiles, or GPU failures.
    pub async fn apply(
        &self,
        image: &RgbaImage,
        small_sigma: f32,
        large_sigma: f32,
    ) -> Result<RgbaImage> {
        ensure!(
            small_sigma.is_finite()
                && large_sigma.is_finite()
                && small_sigma >= 0.1
                && small_sigma < large_sigma
                && large_sigma <= 64.0,
            "DoG requires 0.1 <= small sigma < large sigma <= 64"
        );
        ensure!(image.width() > 0 && image.height() > 0, "Empty DoG image");

        if !TileGrid::needs_tiling(image.width(), image.height(), &self.tile_config) {
            return self.apply_single(image, small_sigma, large_sigma).await;
        }

        let mut config = self.tile_config;
        config.overlap = num_traits::cast((large_sigma * 3.0).ceil())
            .ok_or_else(|| anyhow::anyhow!("Invalid DoG overlap"))?;
        // Reserve room for each tile's Gaussian halo within the GPU buffer limit.
        let side = config.max_tile_pixels.isqrt();
        let core_side = side.saturating_sub(config.overlap.saturating_mul(2));
        ensure!(core_side > 0, "GPU buffer is too small for DoG kernels");
        config.max_tile_pixels = core_side.saturating_mul(core_side);

        let grid = TileGrid::new(image.width(), image.height(), &config);
        let mut processed = Vec::with_capacity(grid.tiles.len());
        for tile in grid.tiles {
            let input = extract_rgba_tile(image, &tile)?;
            let output = self.apply_single(&input, small_sigma, large_sigma).await?;
            processed.push((tile, output));
        }
        merge_rgba_tiles(&processed, image.width(), image.height())
    }

    async fn apply_single(
        &self,
        image: &RgbaImage,
        small_sigma: f32,
        large_sigma: f32,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let luminance = self.filters.channel(image, 0).await?;
        let response = self
            .filters
            .difference_of_gaussians(width, height, &luminance, small_sigma, large_sigma)
            .await?;
        let mut output = image.clone();
        for (pixel, value) in output.pixels_mut().zip(response) {
            let value =
                num_traits::cast::<f32, u8>((value * 255.0).clamp(0.0, 255.0).round()).unwrap_or(0);
            // Equal color channels work in both BGRA and RGBA; preserve source alpha.
            let [_, _, _, alpha] = pixel.0;
            *pixel = image::Rgba([value, value, value, alpha]);
        }
        Ok(output)
    }
}
