use std::{
    borrow::{Borrow, Cow},
    sync::Arc,
};

use anyhow::Result;
use image::RgbaImage;
use wgpu::PipelineCompilationOptions;

use super::GpuImageProcessor;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeUniforms {
    width: u32,
    height: u32,
    layer_count: u32,
    _padding: u32,
}

pub struct GpuCompositor {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuCompositor {
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        // Compute shader for additive blending
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Composite Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/composite.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Composite Pipeline"),
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
    pub async fn composite_images<T: Borrow<RgbaImage> + Sync>(
        &self,
        images: &[T],
    ) -> Result<RgbaImage> {
        if images.is_empty() {
            return Err(anyhow::anyhow!("Cannot composite empty image list"));
        }

        // If we have more than 7 images, batch them
        if images.len() > 7 {
            return self.composite_images_batched(images).await;
        }

        // Direct compositing for <= 7 images
        self.composite_images_direct(images).await
    }

    async fn composite_images_batched<T: Borrow<RgbaImage> + Sync>(
        &self,
        images: &[T],
    ) -> Result<RgbaImage> {
        let mut batched_results = Vec::new();

        // Process images in batches of 7
        for batch in images.chunks(7) {
            let batch_result = self.composite_images_direct(batch).await?;
            batched_results.push(batch_result);
        }

        // Composite the batch results
        self.composite_images_direct(&batched_results).await
    }

    async fn composite_images_direct<T: Borrow<RgbaImage> + Sync>(
        &self,
        images: &[T],
    ) -> Result<RgbaImage> {
        if images.is_empty() {
            return Err(anyhow::anyhow!("Cannot composite empty image list"));
        }

        if images.len() > 7 {
            return Err(anyhow::anyhow!("Direct compositing supports max 7 images"));
        }

        let (width, height) = images
            .first()
            .ok_or_else(|| anyhow::anyhow!("Cannot composite empty image list"))?
            .borrow()
            .dimensions();
        let pixel_count = crate::pixel_count(width, height)?;

        // Create separate buffers for each input image
        let mut image_buffers = Vec::new();
        for img in images {
            let buffer = self.gpu.create_input_buffer(img.borrow());
            image_buffers.push(buffer);
        }

        // Create buffers using helper methods
        let output_buffer = self.gpu.create_output_buffer(pixel_count)?;
        let staging_buffer = self.gpu.create_staging_buffer(pixel_count)?;

        // Uniform buffer for dimensions and layer count
        let uniforms = CompositeUniforms {
            width,
            height,
            layer_count: u32::try_from(images.len())?,
            _padding: 0,
        };

        let uniform_buffer = self.gpu.create_uniform_buffer(&uniforms);

        // Create bind group with separate buffers
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output_buffer.as_entire_binding(),
            },
        ];

        // Add input image buffers
        for (i, buffer) in image_buffers.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: u32::try_from(i)?
                    .checked_add(2)
                    .ok_or_else(|| anyhow::anyhow!("Binding index overflow"))?,
                resource: buffer.as_entire_binding(),
            });
        }

        // Fill remaining slots with dummy buffers if needed
        let dummy_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Dummy Buffer"),
            size: 4, // Minimum size
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        for i in image_buffers.len()..7 {
            entries.push(wgpu::BindGroupEntry {
                binding: u32::try_from(i)?
                    .checked_add(2)
                    .ok_or_else(|| anyhow::anyhow!("Binding index overflow"))?,
                resource: dummy_buffer.as_entire_binding(),
            });
        }

        let bind_group_layout = self.compute_pipeline.get_bind_group_layout(0);
        let bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &bind_group_layout,
                entries: &entries,
                label: Some("Composite Bind Group"),
            });

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Composite Encoder"),
            });

        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.compute_pipeline,
            &bind_group,
            width,
            height,
            "Composite Pass",
        );

        // Submit commands and read results
        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        let result_image = self
            .gpu
            .read_buffer_to_image(&output_buffer, &staging_buffer, width, height, pixel_count)
            .await?;
        println!("Returning composited image");
        Ok(result_image)
    }
}
