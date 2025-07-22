use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use wgpu::{util::DeviceExt, PipelineCompilationOptions};

use super::GpuImageProcessor;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ThresholdUniforms {
    width: u32,
    height: u32,
    threshold: f32,
    _padding: f32,
}

pub struct GpuThreshold {
    compute_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuThreshold {
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        // Compute shader for threshold operation
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Threshold Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/threshold.wgsl"
                ))),
            });

        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("Threshold Pipeline"),
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

    pub async fn apply_threshold(&self, image: &RgbaImage, threshold: f32) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let pixel_count = (width * height) as usize;

        // Create input buffer
        let input_buffer = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Input Image Buffer"),
                contents: bytemuck::cast_slice(image.as_raw()),
                usage: wgpu::BufferUsages::STORAGE,
            });

        // Create output buffer
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

        // Uniform buffer for dimensions and threshold value
        let uniforms = ThresholdUniforms {
            width,
            height,
            threshold: threshold.clamp(0.0, 1.0), // Ensure threshold is in valid range
            _padding: 0.0,
        };

        let uniform_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Threshold Uniform Buffer"),
                    contents: bytemuck::cast_slice(&[uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

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
                label: Some("Threshold Bind Group"),
            });

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Threshold Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Threshold Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.compute_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            // Dispatch with 8x8 work groups (matching the shader workgroup size)
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
            if let Err(e) = sender.send(result) {
                eprintln!("Failed to send GPU buffer mapping result: {}", e);
            }
        });

        self.gpu.device.poll(wgpu::MaintainBase::Wait);
        let map_result = receiver
            .recv_async()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {}", e))?;
        map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {:?}", e))?;

        let data = buffer_slice.get_mapped_range();
        let result_bytes: &[u8] = &data;

        // Create result image from the GPU buffer data
        let result_image = RgbaImage::from_raw(width, height, result_bytes.to_vec())
            .ok_or_else(|| anyhow::anyhow!("Failed to create image from GPU buffer data"))?;

        println!("Applied threshold with value: {}", threshold);
        Ok(result_image)
    }
}
