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
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        // Compute shader for local maxima detection
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Local Maxima Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/local_maxima.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Local Maxima Pipeline"),
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
        let pixel_count = (width * height) as usize;

        // Calculate max intensity for threshold_rel
        let max_intensity = image
            .pixels()
            .map(|p| {
                let r = p[2] as f32 / 255.0;
                let g = p[1] as f32 / 255.0;
                let b = p[0] as f32 / 255.0;
                0.299 * r + 0.587 * g + 0.114 * b
            })
            .fold(0.0f32, f32::max);

        // Determine effective threshold based on threshold type
        let (effective_threshold, threshold_rel_value) = match threshold {
            ThresholdType::Absolute(value) => (value, 0.0),
            ThresholdType::Relative(value) => (max_intensity * value, value),
            ThresholdType::None => (0.0, 0.0),
        };

        // Create buffers using helper methods
        let input_buffer = self.gpu.create_input_buffer(image);
        let output_buffer = self.gpu.create_output_buffer(pixel_count);
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count);

        // Uniform buffer for dimensions and local maxima parameters
        let uniforms = LocalMaximaUniforms {
            width,
            height,
            min_distance,
            exclude_border: if exclude_border { 1 } else { 0 },
            threshold_abs: effective_threshold,
            threshold_rel: threshold_rel_value,
            max_intensity,
            _padding: 0.0,
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
                label: Some("Local Maxima Bind Group"),
            });

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Local Maxima Encoder"),
            });

        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.compute_pipeline,
            &bind_group,
            width,
            height,
            "Local Maxima Pass",
        );

        // Submit commands and read results
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
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
        let tile_grid = TileGrid::new(full_width, full_height, tile_config.clone());

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
                i + 1,
                tile_grid.tiles.len(),
                tile_info.width,
                tile_info.height,
                tile_info.x,
                tile_info.y
            );

            // Extract tile from the full image
            let tile_image = extract_rgba_tile(image, tile_info);

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
