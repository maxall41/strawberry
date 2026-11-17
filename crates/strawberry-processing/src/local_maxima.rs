use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use wgpu::PipelineCompilationOptions;

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ThresholdType {
    Absolute(f32),
    Relative(f32),
    None,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LocalMaximaUniforms {
    width: u32,
    height: u32,
    min_distance: u32,
    exclude_border: u32,
    threshold_abs: f32,
    threshold_rel: f32,
    max_intensity: f32,
    _padding: f32,
}

pub struct GpuLocalMaxima {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuLocalMaxima {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        // Compute shader for local maxima detection
        let shader = gpu
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Local Maxima Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/local_maxima.wgsl"
                ))),
            })
            .await?;

        let compute_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Local Maxima Pipeline"),
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

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn apply_local_maxima(
        &self,
        image: &RgbaImage,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        // Check if we need tile-based processing
        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_local_maxima_tiled(
                image,
                min_distance,
                threshold,
                exclude_border,
                &tile_config,
            )
            .await
        } else {
            // Process as single tile for smaller images
            self.apply_local_maxima_single(image, min_distance, threshold, exclude_border)
                .await
        }
    }

    /// Apply local maxima detection to a single image (no tiling needed)
    async fn apply_local_maxima_single(
        &self,
        image: &RgbaImage,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        // Calculate max intensity for threshold_rel
        let max_intensity = image
            .pixels()
            .map(|p| {
                let r = f32::from(p[2]) / 255.0;
                let g = f32::from(p[1]) / 255.0;
                let b = f32::from(p[0]) / 255.0;
                0.114f32.mul_add(b, 0.587f32.mul_add(g, 0.299 * r))
            })
            .fold(0.0f32, f32::max);

        // Determine effective threshold based on threshold type
        let (effective_threshold, threshold_rel_value) = match threshold {
            ThresholdType::Absolute(value) => (value, 0.0),
            ThresholdType::Relative(value) => (max_intensity * value, value),
            ThresholdType::None => (0.0, 0.0),
        };

        // Create buffers using helper methods
        let input_buffer = self.gpu.create_input_buffer(image).await?;
        let output_buffer = self.gpu.create_output_buffer(pixel_count).await?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count).await?;

        // Uniform buffer for dimensions and local maxima parameters
        let uniforms = LocalMaximaUniforms {
            width,
            height,
            min_distance,
            exclude_border: u32::from(exclude_border),
            threshold_abs: effective_threshold,
            threshold_rel: threshold_rel_value,
            max_intensity,
            _padding: 0.0,
        };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms).await?;

        // Create bind group
        let bind_group = self
            .gpu
            .create_buffer_bind_group(
                &self.compute_pipeline,
                &[&uniform_buffer, &output_buffer, &input_buffer],
                "Local Maxima Bind Group",
            )
            .await?;

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Local Maxima Encoder"),
            })
            .await?;

        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.compute_pipeline,
                &bind_group,
                width,
                height,
                "Local Maxima Pass",
            )
            .await?;

        // Submit commands and read results
        self.gpu.submit(encoder).await?;
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        println!(
            "Applied local maxima detection with min_distance: {min_distance}, threshold: {threshold:?}, exclude_border: {exclude_border}"
        );
        Ok(result_image)
    }

    /// Apply local maxima detection using tile-based approach for large images
    async fn apply_local_maxima_tiled(
        &self,
        image: &RgbaImage,
        min_distance: u32,
        threshold: ThresholdType,
        exclude_border: bool,
        tile_config: &TileConfig,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config);

        println!(
            "Processing {} tiles for {}x{} image with local maxima detection",
            tile_grid.tiles.len(),
            full_width,
            full_height
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
                .apply_local_maxima_single(
                    &tile_image,
                    min_distance,
                    threshold.clone(),
                    exclude_border,
                )
                .await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        // Merge tiles back into full image
        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }
}
