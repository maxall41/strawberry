use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use wgpu::PipelineCompilationOptions;

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DecomposerUniforms {
    width: u32,
    height: u32,
    _padding1: u32,
    _padding2: u32,
}

pub struct GpuDecomposer {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuDecomposer {
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        // Compute shader for channel decomposition
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Decomposer Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/decomposer.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Decomposer Pipeline"),
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

    pub async fn apply_decomposition(&self, image: &RgbaImage) -> Result<Vec<RgbaImage>> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        // Check if we need tile-based processing
        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_decomposition_tiled(image, &tile_config).await
        } else {
            // Process as single tile for smaller images
            self.apply_decomposition_single(image).await
        }
    }

    /// Apply decomposition to a single image (no tiling needed)
    async fn apply_decomposition_single(&self, image: &RgbaImage) -> Result<Vec<RgbaImage>> {
        let (width, height) = image.dimensions();
        let pixel_count = (width * height) as usize;

        // Create input buffer
        let input_buffer = self.gpu.create_input_buffer(image);

        // Create output buffers for all three channels
        let red_output_buffer = self.gpu.create_output_buffer(pixel_count);
        let green_output_buffer = self.gpu.create_output_buffer(pixel_count);
        let blue_output_buffer = self.gpu.create_output_buffer(pixel_count);

        // Create staging buffers for all three channels
        let red_staging_buffer = self.gpu.create_staging_buffer(pixel_count);
        let green_staging_buffer = self.gpu.create_staging_buffer(pixel_count);
        let blue_staging_buffer = self.gpu.create_staging_buffer(pixel_count);

        // Uniform buffer for dimensions
        let uniforms = DecomposerUniforms {
            width,
            height,
            _padding1: 0,
            _padding2: 0,
        };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms);

        // Create bind group with all output buffers
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
                        resource: red_output_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: green_output_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: blue_output_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: input_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Decomposer Bind Group"),
            });

        // Dispatch compute shader once for all channels
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Decomposer Encoder"),
            });

        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.compute_pipeline,
            &bind_group,
            width,
            height,
            "Decomposer Pass",
        );

        // Submit commands
        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        // Read results for all three channels
        let red_image = self
            .gpu
            .read_buffer_to_image(
                &red_output_buffer,
                &red_staging_buffer,
                width,
                height,
                pixel_count,
            )
            .await?;

        let green_image = self
            .gpu
            .read_buffer_to_image(
                &green_output_buffer,
                &green_staging_buffer,
                width,
                height,
                pixel_count,
            )
            .await?;

        let blue_image = self
            .gpu
            .read_buffer_to_image(
                &blue_output_buffer,
                &blue_staging_buffer,
                width,
                height,
                pixel_count,
            )
            .await?;

        let result_images = vec![red_image, green_image, blue_image];

        println!("Applied GPU decomposition to {width}x{height} image in single dispatch");

        // Verify we have exactly 3 channel images
        assert_eq!(result_images.len(), 3);
        Ok(result_images)
    }

    /// Apply decomposition using tile-based approach for large images
    async fn apply_decomposition_tiled(
        &self,
        image: &RgbaImage,
        tile_config: &TileConfig,
    ) -> Result<Vec<RgbaImage>> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config.clone());

        println!(
            "Processing {} tiles for {}x{} image with decomposition",
            tile_grid.tiles.len(),
            full_width,
            full_height
        );

        // Store processed tiles for each channel
        let mut red_tiles = Vec::new();
        let mut green_tiles = Vec::new();
        let mut blue_tiles = Vec::new();

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
            let processed_tiles = self.apply_decomposition_single(&tile_image).await?;

            // Store tiles for each channel
            red_tiles.push((tile_info.clone(), processed_tiles[0].clone()));
            green_tiles.push((tile_info.clone(), processed_tiles[1].clone()));
            blue_tiles.push((tile_info.clone(), processed_tiles[2].clone()));
        }

        // Merge tiles back into full images for each channel
        let red_image = merge_rgba_tiles(&red_tiles, full_width, full_height)?;
        let green_image = merge_rgba_tiles(&green_tiles, full_width, full_height)?;
        let blue_image = merge_rgba_tiles(&blue_tiles, full_width, full_height)?;

        let result = vec![red_image, green_image, blue_image];
        assert_eq!(result.len(), 3);
        Ok(result)
    }
}
