use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use wgpu::PipelineCompilationOptions;

mod auto;
pub use auto::AutoThresholdMethod;
use auto::luminance_histogram;

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ThresholdUniforms {
    width: u32,
    height: u32,
    // Histogram bin (0–255), widened only for the WGSL uniform layout.
    threshold: u32,
    _padding: u32,
}

pub struct GpuThreshold {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuThreshold {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        // Compute shader for threshold operation
        let shader = gpu
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Threshold Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/threshold.wgsl"
                ))),
            })
            .await?;

        let compute_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Threshold Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
            .await?;

        Ok(Self {
            compute_pipeline,
            gpu,
        })
    }

    /// Apply a histogram-bin cutoff; foreground pixels have levels strictly above it.
    ///
    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn apply_threshold(&self, image: &RgbaImage, threshold: u8) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        // Check if we need tile-based processing
        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_threshold_tiled(image, threshold, &tile_config)
                .await
        } else {
            // Process as single tile for smaller images
            self.apply_threshold_single(image, threshold).await
        }
    }

    /// Select a global threshold from this image's histogram, then apply it on the GPU.
    ///
    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn apply_auto_threshold(
        &self,
        image: &RgbaImage,
        method: AutoThresholdMethod,
    ) -> Result<RgbaImage> {
        let threshold = method.threshold(&luminance_histogram(image));
        self.apply_threshold(image, threshold).await
    }

    /// Apply threshold operation to a single image (no tiling needed)
    async fn apply_threshold_single(&self, image: &RgbaImage, threshold: u8) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        // Create buffers using helper methods
        let input_buffer = self.gpu.create_input_buffer(image).await?;
        let output_buffer = self.gpu.create_output_buffer(pixel_count).await?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count).await?;

        // Uniform buffer for dimensions and threshold value
        let uniforms = ThresholdUniforms {
            width,
            height,
            threshold: u32::from(threshold),
            _padding: 0,
        };
        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms).await?;

        // Create bind group
        let bind_group = self
            .gpu
            .create_buffer_bind_group(
                &self.compute_pipeline,
                &[&uniform_buffer, &output_buffer, &input_buffer],
                "Threshold Bind Group",
            )
            .await?;

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Threshold Encoder"),
            })
            .await?;

        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.compute_pipeline,
                &bind_group,
                width,
                height,
                "Threshold Pass",
            )
            .await?;

        // Submit commands and read results
        self.gpu.submit(encoder).await?;
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        println!("Applied threshold with value: {threshold}");
        Ok(result_image)
    }

    /// Apply threshold operation using tile-based approach for large images
    async fn apply_threshold_tiled(
        &self,
        image: &RgbaImage,
        threshold: u8,
        tile_config: &TileConfig,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config);

        println!(
            "Processing {} tiles for {}x{} image with threshold {}",
            tile_grid.tiles.len(),
            full_width,
            full_height,
            threshold
        );

        let mut processed_tiles = Vec::new();

        for (i, tile_info) in tile_grid.tiles.iter().enumerate() {
            println!(
                "Processing tile {}/{}: {}x{} at ({}, {})",
                i.saturating_add(1),
                tile_grid.tiles.len(),
                tile_info.width,
                tile_info.height,
                tile_info.x,
                tile_info.y
            );

            // Extract tile from the full image
            let tile_image = extract_rgba_tile(image, tile_info)?;

            // Process the tile
            let processed_tile = self.apply_threshold_single(&tile_image, threshold).await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        // Merge tiles back into full image
        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }
}
