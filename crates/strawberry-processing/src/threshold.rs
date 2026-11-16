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
    // Normalized cutoff in manual mode, histogram bin (0–255) in automatic mode.
    threshold: f32,
    automatic: u32,
}

pub struct GpuThreshold {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuThreshold {
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        // Compute shader for threshold operation
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Threshold Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/threshold.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Threshold Pipeline"),
                    layout: None,
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: PipelineCompilationOptions::default(),
                    cache: None,
                });

        Self {
            compute_pipeline,
            gpu,
        }
    }

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn apply_threshold(&self, image: &RgbaImage, threshold: f32) -> Result<RgbaImage> {
        self.apply_threshold_inner(image, threshold.clamp(0.0, 1.0), false)
            .await
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
        self.apply_threshold_inner(image, f32::from(threshold), true)
            .await
    }

    async fn apply_threshold_inner(
        &self,
        image: &RgbaImage,
        threshold: f32,
        automatic: bool,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        if width == 0 || height == 0 {
            return Ok(image.clone());
        }
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        // Check if we need tile-based processing
        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_threshold_tiled(image, threshold, automatic, &tile_config)
                .await
        } else {
            // Process as single tile for smaller images
            self.apply_threshold_single(image, threshold, automatic)
                .await
        }
    }

    /// Apply threshold operation to a single image (no tiling needed)
    async fn apply_threshold_single(
        &self,
        image: &RgbaImage,
        threshold: f32,
        automatic: bool,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        // Create buffers using helper methods
        let input_buffer = self.gpu.create_input_buffer(image);
        let output_buffer = self.gpu.create_output_buffer(pixel_count)?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count)?;

        // Uniform buffer for dimensions and threshold value
        let uniforms = ThresholdUniforms {
            width,
            height,
            threshold,
            automatic: u32::from(automatic),
        };
        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms);

        // Create bind group
        let bind_group_layout = self.compute_pipeline.get_bind_group_layout(0);
        let bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: output_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: input_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Threshold Bind Group"),
            });

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Threshold Encoder"),
            });

        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.compute_pipeline,
            &bind_group,
            width,
            height,
            "Threshold Pass",
        );

        // Submit commands and read results
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        println!("Applied threshold with value: {threshold}, automatic: {automatic}");
        Ok(result_image)
    }

    /// Apply threshold operation using tile-based approach for large images
    async fn apply_threshold_tiled(
        &self,
        image: &RgbaImage,
        threshold: f32,
        automatic: bool,
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
            let processed_tile = self
                .apply_threshold_single(&tile_image, threshold, automatic)
                .await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        // Merge tiles back into full image
        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }
}
