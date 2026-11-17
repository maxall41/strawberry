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
struct SobelUniforms {
    width: u32,
    height: u32,
    threshold: f32,
    _padding: f32,
}

pub struct GpuSobel {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuSobel {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        let shader = gpu
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Sobel Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(concat!(
                    include_str!("shaders/sobel_math.wgsl"),
                    include_str!("shaders/sobel.wgsl")
                ))),
            })
            .await?;

        let compute_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Sobel Pipeline"),
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
    pub async fn apply_sobel(
        &self,
        image: &RgbaImage,
        threshold: Option<f32>,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_sobel_tiled(image, threshold, &tile_config).await
        } else {
            self.apply_sobel_single(image, threshold).await
        }
    }

    /// Compute Sobel edges for a single image with the GPU pipeline.
    async fn apply_sobel_single(
        &self,
        image: &RgbaImage,
        threshold: Option<f32>,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        let input_buffer = self.gpu.create_input_buffer(image).await?;
        let output_buffer = self.gpu.create_output_buffer(pixel_count).await?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count).await?;

        let uniforms = SobelUniforms {
            width,
            height,
            threshold: threshold.unwrap_or(0.0),
            _padding: 0.0,
        };
        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms).await?;

        let bind_group = self
            .gpu
            .create_buffer_bind_group(
                &self.compute_pipeline,
                &[&uniform_buffer, &output_buffer, &input_buffer],
                "Sobel Bind Group",
            )
            .await?;

        let mut encoder = self
            .gpu
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Sobel Encoder"),
            })
            .await?;

        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.compute_pipeline,
                &bind_group,
                width,
                height,
                "Sobel Pass",
            )
            .await?;

        self.gpu.submit(encoder).await?;
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        match threshold {
            Some(t) => println!("Applied Sobel edge detection with threshold: {t}"),
            None => println!("Applied Sobel edge detection without threshold"),
        }
        Ok(result_image)
    }

    /// Compute Sobel edges on overlapping tiles and merge their cores.
    async fn apply_sobel_tiled(
        &self,
        image: &RgbaImage,
        threshold: Option<f32>,
        tile_config: &TileConfig,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config);

        println!(
            "Processing {} tiles for {}x{} image with Sobel edge detection",
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
            let processed_tile = self.apply_sobel_single(&tile_image, threshold).await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }
}
