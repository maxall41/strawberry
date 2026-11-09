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
struct GrayscaleUniforms {
    width: u32,
    height: u32,
}

pub struct GpuGrayscale {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuGrayscale {
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Grayscale Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/grayscale.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Grayscale Pipeline"),
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
    pub async fn apply_grayscale(&self, image: &RgbaImage) -> Result<RgbaImage> {
        // Ok(image.clone())
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_grayscale_tiled(image, &tile_config).await
        } else {
            self.apply_grayscale_single(image).await
        }
    }

    async fn apply_grayscale_single(&self, image: &RgbaImage) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        let input_buffer = self.gpu.create_input_buffer(image);
        let output_buffer = self.gpu.create_output_buffer(pixel_count)?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count)?;

        let uniforms = GrayscaleUniforms { width, height };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms);

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
                label: Some("Grayscale Bind Group"),
            });

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Grayscale Encoder"),
            });

        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.compute_pipeline,
            &bind_group,
            width,
            height,
            "Grayscale Pass",
        );

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        println!("Applied grayscale conversion");
        Ok(result_image)
    }

    async fn apply_grayscale_tiled(
        &self,
        image: &RgbaImage,
        tile_config: &TileConfig,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config);

        println!(
            "Processing {} tiles for {}x{} image with grayscale conversion",
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

            let tile_image = extract_rgba_tile(image, tile_info)?;

            let processed_tile = self.apply_grayscale_single(&tile_image).await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }
}
