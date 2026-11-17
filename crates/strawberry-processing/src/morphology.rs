use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use wgpu::PipelineCompilationOptions;

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MorphologyOperation {
    #[default]
    Dilation,
    Erosion,
    Opening,
    Closing,
}

impl MorphologyOperation {
    /// Return the shader identifier for this morphology operation.
    const fn to_u32(self) -> u32 {
        match self {
            Self::Dilation => 0,
            Self::Erosion => 1,
            Self::Opening => 2,
            Self::Closing => 3,
        }
    }

    /// Check whether this operation requires successive erosion and dilation passes.
    const fn is_two_pass(self) -> bool {
        matches!(self, Self::Opening | Self::Closing)
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MorphologyUniforms {
    width: u32,
    height: u32,
    operation: u32,
    element_size: u32,
}

pub struct GpuMorphology {
    compute_pipeline: wgpu::ComputePipeline,
    second_pass_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuMorphology {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        // Compute shader for morphological operations
        let shader = gpu
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Morphology Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/morphology.wgsl"
                ))),
            })
            .await?;

        let compute_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Morphology Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
            .await?;

        let second_pass_pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Morphology Second Pass Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("second_pass"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
            .await?;

        Ok(Self {
            compute_pipeline,
            second_pass_pipeline,
            gpu,
        })
    }

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn apply_morphology(
        &self,
        image: &RgbaImage,
        operation: MorphologyOperation,
        element_size: u32,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        // Check if we need tile-based processing
        if TileGrid::needs_tiling(width, height, &tile_config) {
            println!("Image {width}x{height} requires tile-based processing for GPU limits");
            self.apply_morphology_tiled(image, operation, element_size, &tile_config)
                .await
        } else {
            // Process as single tile for smaller images
            self.apply_morphology_single(image, operation, element_size)
                .await
        }
    }

    /// Bind the image, temporary, and parameter buffers for a morphology pass.
    async fn create_bind_group(
        &self,
        pipeline: &wgpu::ComputePipeline,
        uniforms: &wgpu::Buffer,
        output: &wgpu::Buffer,
        input: &wgpu::Buffer,
        temp: &wgpu::Buffer,
    ) -> Result<wgpu::BindGroup> {
        self.gpu
            .create_buffer_bind_group(
                pipeline,
                &[uniforms, output, input, temp],
                "Morphology Bind Group",
            )
            .await
    }

    /// Apply morphological operation to a single image (no tiling needed)
    async fn apply_morphology_single(
        &self,
        image: &RgbaImage,
        operation: MorphologyOperation,
        element_size: u32,
    ) -> Result<RgbaImage> {
        // Ensure element_size is odd for symmetric structuring element
        let element_size = if element_size.is_multiple_of(2) {
            element_size | 1
        } else {
            element_size
        };

        let (width, height) = image.dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        // Create buffers using helper methods
        let input_buffer = self.gpu.create_input_buffer(image).await?;
        let output_buffer = self.gpu.create_output_buffer(pixel_count).await?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count).await?;

        // Create temp buffer for two-pass operations
        let temp_buffer = self
            .gpu
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("Temp Buffer"),
                size: crate::buffer_size(pixel_count, 4)?,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
            .await?;

        // Uniform buffer for dimensions and morphology parameters
        let uniforms = MorphologyUniforms {
            width,
            height,
            operation: operation.to_u32(),
            element_size,
        };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms).await?;

        let first_pass_bind_group = self
            .create_bind_group(
                &self.compute_pipeline,
                &uniform_buffer,
                if operation.is_two_pass() {
                    &temp_buffer
                } else {
                    &output_buffer
                },
                &input_buffer,
                &temp_buffer,
            )
            .await?;

        // Dispatch first pass
        let mut encoder = self
            .gpu
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Morphology Encoder"),
            })
            .await?;

        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.compute_pipeline,
                &first_pass_bind_group,
                width,
                height,
                "Morphology First Pass",
            )
            .await?;

        // If this is a two-pass operation, dispatch the second pass
        if operation.is_two_pass() {
            let second_pass_bind_group = self
                .create_bind_group(
                    &self.second_pass_pipeline,
                    &uniform_buffer,
                    &output_buffer,
                    &input_buffer,
                    &temp_buffer,
                )
                .await?;

            self.gpu
                .dispatch_compute_with_encoder(
                    &mut encoder,
                    &self.second_pass_pipeline,
                    &second_pass_bind_group,
                    width,
                    height,
                    "Morphology Second Pass",
                )
                .await?;
        }

        // Submit commands and read results
        self.gpu.submit(encoder).await?;
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;

        println!("Applied morphological {operation:?} with element size: {element_size}");
        Ok(result_image)
    }

    /// Apply morphological operation using tile-based approach for large images
    async fn apply_morphology_tiled(
        &self,
        image: &RgbaImage,
        operation: MorphologyOperation,
        element_size: u32,
        tile_config: &TileConfig,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config);

        println!(
            "Processing {} tiles for {}x{} image with morphology {:?}",
            tile_grid.tiles.len(),
            full_width,
            full_height,
            operation
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
                .apply_morphology_single(&tile_image, operation, element_size)
                .await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        // Merge tiles back into full image
        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }

    // Convenience methods for specific operations
    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn dilate(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Dilation, element_size)
            .await
    }

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn erode(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Erosion, element_size)
            .await
    }

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn opening(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Opening, element_size)
            .await
    }

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
    pub async fn closing(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Closing, element_size)
            .await
    }
}
