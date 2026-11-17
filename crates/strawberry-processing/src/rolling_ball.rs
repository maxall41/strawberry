use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use wgpu::PipelineCompilationOptions;

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

pub struct GpuRollingBall {
    erosion_pipeline: wgpu::ComputePipeline,
    reconstruction_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct RollingBallUniforms {
    width: u32,
    height: u32,
    ball_radius: u32,
    _padding: u32,
}

impl GpuRollingBall {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        let shader = gpu
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Rolling Ball Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/rolling_ball.wgsl"
                ))),
            })
            .await?;

        let erosion_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Rolling Ball Erosion Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("erosion_pass"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
            .await?;

        let reconstruction_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Rolling Ball Reconstruction Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
            .await?;

        Ok(Self {
            erosion_pipeline,
            reconstruction_pipeline,
            gpu,
        })
    }

    /// Apply rolling ball background subtraction to an image.
    ///
    /// # Arguments
    ///
    /// * `image` - The input RGBA image to process
    /// * `ball_radius` - The radius of the rolling ball in pixels. Larger values
    ///   remove more of the background but may also remove features of interest.
    ///   Typical values range from 10-100 pixels depending on image size and
    ///   background characteristics.
    ///
    /// # Returns
    ///
    /// A new RGBA image with background subtracted. The output preserves the
    /// alpha channel from the input image.
    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn apply_rolling_ball(
        &self,
        image: &RgbaImage,
        ball_radius: u32,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_rolling_ball_tiled(image, ball_radius, &tile_config)
                .await
        } else {
            self.apply_rolling_ball_single(image, ball_radius).await
        }
    }

    async fn apply_rolling_ball_single(
        &self,
        image: &RgbaImage,
        ball_radius: u32,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        let input_buffer = self.gpu.create_input_buffer(image).await?;
        let output_buffer = self.gpu.create_output_buffer(pixel_count).await?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count).await?;
        let eroded_buffer = self
            .gpu
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("Rolling Ball Eroded Luminance Buffer"),
                size: crate::buffer_size(pixel_count, 4)?,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
            .await?;

        let uniforms = RollingBallUniforms {
            width,
            height,
            ball_radius,
            _padding: 0,
        };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms).await?;

        let erosion_layout = self
            .gpu
            .get_bind_group_layout(&self.erosion_pipeline, 0)
            .await?;
        let erosion_entries = [
            (0, &uniform_buffer),
            (2, &input_buffer),
            (3, &eroded_buffer),
        ]
        .map(|(binding, buffer)| wgpu::BindGroupEntry {
            binding,
            resource: buffer.as_entire_binding(),
        });
        let erosion_bind_group = self
            .gpu
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Rolling Ball Erosion Bind Group"),
                layout: &erosion_layout,
                entries: &erosion_entries,
            })
            .await?;

        let reconstruction_bind_group = self
            .gpu
            .create_buffer_bind_group(
                &self.reconstruction_pipeline,
                &[
                    &uniform_buffer,
                    &output_buffer,
                    &input_buffer,
                    &eroded_buffer,
                ],
                "Rolling Ball Reconstruction Bind Group",
            )
            .await?;

        let mut encoder = self
            .gpu
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Rolling Ball Encoder"),
            })
            .await?;

        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.erosion_pipeline,
                &erosion_bind_group,
                width,
                height,
                "Rolling Ball Erosion Pass",
            )
            .await?;
        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.reconstruction_pipeline,
                &reconstruction_bind_group,
                width,
                height,
                "Rolling Ball Reconstruction Pass",
            )
            .await?;

        self.gpu.submit(encoder).await?;
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        println!("Applied rolling ball background subtraction with ball radius: {ball_radius}");
        Ok(result_image)
    }

    async fn apply_rolling_ball_tiled(
        &self,
        image: &RgbaImage,
        ball_radius: u32,
        tile_config: &TileConfig,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let mut tile_config = *tile_config;
        // Reconstruction samples eroded pixels one radius away, and each of
        // those pixels depends on another radius of the original image.
        tile_config.overlap = tile_config.overlap.max(
            usize::try_from(ball_radius)?
                .checked_mul(2)
                .ok_or_else(|| anyhow::anyhow!("Tile overlap overflow"))?,
        );
        let tile_grid = TileGrid::new(full_width, full_height, &tile_config);

        println!(
            "Processing {} tiles for {}x{} image with rolling ball (radius: {})",
            tile_grid.tiles.len(),
            full_width,
            full_height,
            ball_radius
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
            let processed_tile = self
                .apply_rolling_ball_single(&tile_image, ball_radius)
                .await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }
}
