use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use wgpu::PipelineCompilationOptions;

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum MorphologyOperation {
    #[default]
    Dilation,
    Erosion,
    Opening,
    Closing,
}

impl MorphologyOperation {
    fn to_u32(self) -> u32 {
        match self {
            MorphologyOperation::Dilation => 0,
            MorphologyOperation::Erosion => 1,
            MorphologyOperation::Opening => 2,
            MorphologyOperation::Closing => 3,
        }
    }

    fn is_two_pass(self) -> bool {
        matches!(
            self,
            MorphologyOperation::Opening | MorphologyOperation::Closing
        )
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
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        // Compute shader for morphological operations
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Morphology Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/morphology.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Morphology Pipeline"),
                    layout: None,
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: PipelineCompilationOptions::default(),
                    cache: None,
                });

        let second_pass_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Morphology Second Pass Pipeline"),
                    layout: None,
                    module: &shader,
                    entry_point: Some("second_pass"),
                    compilation_options: PipelineCompilationOptions::default(),
                    cache: None,
                });

        Self {
            compute_pipeline,
            second_pass_pipeline,
            gpu,
        }
    }

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

    /// Apply morphological operation to a single image (no tiling needed)
    async fn apply_morphology_single(
        &self,
        image: &RgbaImage,
        operation: MorphologyOperation,
        element_size: u32,
    ) -> Result<RgbaImage> {
        // Ensure element_size is odd for symmetric structuring element
        let element_size = if element_size.is_multiple_of(2) {
            element_size + 1
        } else {
            element_size
        };

        let (width, height) = image.dimensions();
        let pixel_count = (width * height) as usize;

        // Create buffers using helper methods
        let input_buffer = self.gpu.create_input_buffer(image);
        let output_buffer = self.gpu.create_output_buffer(pixel_count);
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count);

        // Create temp buffer for two-pass operations
        let temp_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Temp Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        // Uniform buffer for dimensions and morphology parameters
        let uniforms = MorphologyUniforms {
            width,
            height,
            operation: operation.to_u32(),
            element_size,
        };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms);

        // Create bind group for first pass
        let bind_group_layout = self.compute_pipeline.get_bind_group_layout(0);
        let first_pass_bind_group = self
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
                        resource: if operation.is_two_pass() {
                            temp_buffer.as_entire_binding() // First pass writes to temp
                        } else {
                            output_buffer.as_entire_binding() // Single pass writes to output
                        },
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: input_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: temp_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Morphology First Pass Bind Group"),
            });

        // Dispatch first pass
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Morphology Encoder"),
            });

        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.compute_pipeline,
            &first_pass_bind_group,
            width,
            height,
            "Morphology First Pass",
        );

        // If this is a two-pass operation, dispatch the second pass
        if operation.is_two_pass() {
            // Create bind group for second pass
            let second_pass_bind_group_layout = self.second_pass_pipeline.get_bind_group_layout(0);
            let second_pass_bind_group =
                self.gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        layout: &second_pass_bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: uniform_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: output_buffer.as_entire_binding(), // Second pass writes to output
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: input_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: temp_buffer.as_entire_binding(), // Second pass reads from temp
                            },
                        ],
                        label: Some("Morphology Second Pass Bind Group"),
                    });

            self.gpu.dispatch_compute_with_encoder(
                &mut encoder,
                &self.second_pass_pipeline,
                &second_pass_bind_group,
                width,
                height,
                "Morphology Second Pass",
            );
        }

        // Submit commands and read results
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
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
        let tile_grid = TileGrid::new(full_width, full_height, tile_config.clone());

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
                .apply_morphology_single(&tile_image, operation, element_size)
                .await?;

            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        // Merge tiles back into full image
        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }

    // Convenience methods for specific operations
    pub async fn dilate(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Dilation, element_size)
            .await
    }

    pub async fn erode(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Erosion, element_size)
            .await
    }

    pub async fn opening(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Opening, element_size)
            .await
    }

    pub async fn closing(&self, image: &RgbaImage, element_size: u32) -> Result<RgbaImage> {
        self.apply_morphology(image, MorphologyOperation::Closing, element_size)
            .await
    }
}
