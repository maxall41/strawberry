use std::{borrow::Cow, sync::Arc};

use image::RgbaImage;
use wgpu::{util::DeviceExt, PipelineCompilationOptions};

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
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Self {
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
    pub async fn composite_images(&self, images: &[RgbaImage]) -> Option<RgbaImage> {
        if images.is_empty() {
            return None;
        }

        let (width, height) = images[0].dimensions();
        let pixel_count = (width * height) as usize;

        // Create separate buffers for each input image
        let mut image_buffers = Vec::new();
        for img in images {
            let buffer = self
                .gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Input Image Buffer"),
                    contents: bytemuck::cast_slice(img.as_raw()),
                    usage: wgpu::BufferUsages::STORAGE,
                });
            image_buffers.push(buffer);
        }

        // Ensure we don't exceed shader limits (max 7 images for now)
        if image_buffers.len() > 7 {
            panic!("GPU compositor currently supports max 7 images");
        }

        // Output buffer
        let output_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
            size: (pixel_count * 4) as u64, // 4 bytes per RGBA pixel
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Staging buffer to read results back to CPU
        let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Uniform buffer for dimensions and layer count
        let uniforms = CompositeUniforms {
            width,
            height,
            layer_count: images.len() as u32,
            _padding: 0,
        };

        let uniform_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Uniform Buffer"),
                    contents: bytemuck::cast_slice(&[uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

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
                binding: (i + 2) as u32,
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
                binding: (i + 2) as u32,
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

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Composite Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.compute_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            // Dispatch with 8x8 work groups (adjust for optimal performance)
            let workgroup_size = 8;
            let dispatch_x = (width + workgroup_size - 1) / workgroup_size;
            let dispatch_y = (height + workgroup_size - 1) / workgroup_size;
            compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
        }

        // Copy result to staging buffer
        encoder.copy_buffer_to_buffer(
            &output_buffer,
            0,
            &staging_buffer,
            0,
            (pixel_count * 4) as u64,
        );

        // Submit commands
        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        // Read back results
        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = flume::unbounded();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });

        self.gpu.device.poll(wgpu::MaintainBase::Wait);
        receiver.recv_async().await.unwrap().unwrap();

        let data = buffer_slice.get_mapped_range();
        let result_bytes: &[u8] = &data;

        // Create result image from the GPU buffer data
        let result_image = RgbaImage::from_raw(width, height, result_bytes.to_vec())
            .expect("Failed to create image from GPU buffer");
        println!("Returning composited image");
        Some(result_image)
    }
}
