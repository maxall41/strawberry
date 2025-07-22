use std::{borrow::Cow, sync::Arc};

use anyhow::Result;
use image::RgbaImage;
use wgpu::{util::DeviceExt, PipelineCompilationOptions};

use super::GpuImageProcessor;

#[derive(Debug, Clone, PartialEq)]
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
        let pixel_count = (width * height) as usize;

        // Calculate max intensity for threshold_rel
        let max_intensity = image
            .pixels()
            .map(|p| {
                let r = p[0] as f32 / 255.0;
                let g = p[1] as f32 / 255.0;
                let b = p[2] as f32 / 255.0;
                0.299 * r + 0.587 * g + 0.114 * b
            })
            .fold(0.0f32, f32::max);

        // Determine effective threshold based on threshold type
        let (effective_threshold, threshold_rel_value) = match threshold {
            ThresholdType::Absolute(value) => (value, 0.0),
            ThresholdType::Relative(value) => (max_intensity * value, value),
            ThresholdType::None => (0.0, 0.0),
        };

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

        let uniform_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Local Maxima Uniform Buffer"),
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
                label: Some("Local Maxima Bind Group"),
            });

        // Dispatch compute shader
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Local Maxima Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Local Maxima Pass"),
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

        println!(
            "Applied local maxima detection with min_distance: {}, threshold: {:?}, exclude_border: {}",
            min_distance, threshold, exclude_border
        );
        Ok(result_image)
    }
}
