//! Reusable floating-point GPU filters. Values are never clipped to display range.
use crate::GpuImageProcessor;
use anyhow::{Result, ensure};
use std::{borrow::Cow, sync::Arc};
use wgpu::util::DeviceExt;

#[repr(u32)]
#[derive(Clone, Copy)]
enum FilterOperation {
    Channel = 0,
    Gaussian = 1,
    Sobel = 2,
    DifferenceOfGaussians = 3,
    HessianEigenvalues = 4,
    MembraneProjections = 5,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    width: u32,
    height: u32,
    operation: u32,
    channel: u32,
    sigma: f32,
    radius: f32,
    pad: [u32; 2],
}

pub struct GpuFloatFilters {
    gpu: Arc<GpuImageProcessor>,
    pipeline: wgpu::ComputePipeline,
    membrane_kernels: Vec<f32>,
}
impl GpuFloatFilters {
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Floating point filters"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(concat!(
                    include_str!("shaders/gaussian_blur_math.wgsl"),
                    include_str!("shaders/sobel_math.wgsl"),
                    include_str!("shaders/hessian_math.wgsl"),
                    include_str!("shaders/difference_of_gaussians.wgsl"),
                    include_str!("shaders/float_filters.wgsl")
                ))),
            });
        let pipeline = gpu
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Floating point filters"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        Self {
            gpu,
            pipeline,
            membrane_kernels: membrane_kernels(),
        }
    }
    async fn run(
        &self,
        width: u32,
        height: u32,
        input: &[u8],
        second: &[f32],
        operation: FilterOperation,
        channel: u32,
        sigma: f32,
    ) -> Result<Vec<f32>> {
        let count = crate::pixel_count(width, height)?;
        ensure!(
            count > 0
                && input.len()
                    == count
                        .checked_mul(4)
                        .ok_or_else(|| anyhow::anyhow!("Image too large"))?,
            "Invalid filter input"
        );
        let bytes_per_pixel = match operation {
            FilterOperation::MembraneProjections => 24,
            _ => 4,
        };
        let bytes = crate::buffer_size(count, bytes_per_pixel)?;
        ensure!(
            bytes <= u64::from(self.gpu.device.limits().max_storage_buffer_binding_size),
            "Tile exceeds GPU buffer limits"
        );
        let a = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: input,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let dummy = [0.0f32];
        let b = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(if second.is_empty() { &dummy } else { second }),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let out = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params = self.gpu.create_uniform_buffer(&Params {
            width,
            height,
            operation: operation as u32,
            channel,
            sigma,
            radius: (sigma * 3.0).ceil(),
            pad: [0; 2],
        });
        let layout = self.pipeline.get_bind_group_layout(0);
        let group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: out.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: a.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: b.as_entire_binding(),
                    },
                ],
            });
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.gpu.dispatch_compute_with_encoder(
            &mut encoder,
            &self.pipeline,
            &group,
            width,
            height,
            "Float filter",
        );
        encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, bytes);
        self.gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = flume::bounded(1);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv_async().await??;
        let result = bytemuck::cast_slice(&staging.slice(..).get_mapped_range()?).to_vec();
        staging.unmap();
        Ok(result)
    }
    /// Extract luminance (0) or R/G/B (1/2/3) from internal BGRA pixels.
    pub async fn channel(&self, image: &image::RgbaImage, channel: u32) -> Result<Vec<f32>> {
        ensure!(channel <= 3, "Unknown channel");
        self.run(
            image.width(),
            image.height(),
            image.as_raw(),
            &[],
            FilterOperation::Channel,
            channel,
            1.0,
        )
        .await
    }
    pub async fn membrane_projections(
        &self,
        width: u32,
        height: u32,
        input: &[f32],
    ) -> Result<[Vec<f32>; 6]> {
        let values = self
            .run(
                width,
                height,
                bytemuck::cast_slice(input),
                &self.membrane_kernels,
                FilterOperation::MembraneProjections,
                0,
                1.0,
            )
            .await?;
        let count = crate::pixel_count(width, height)?;
        let mut projections = [const { Vec::new() }; 6];
        for (plane, values) in projections.iter_mut().zip(values.chunks_exact(count)) {
            *plane = values.to_vec();
        }
        Ok(projections)
    }

    pub async fn gaussian(
        &self,
        width: u32,
        height: u32,
        input: &[f32],
        sigma: f32,
    ) -> Result<Vec<f32>> {
        ensure!(
            sigma.is_finite() && (0.1..=64.0).contains(&sigma),
            "Sigma must be between 0.1 and 64"
        );
        self.run(
            width,
            height,
            bytemuck::cast_slice(input),
            &[],
            FilterOperation::Gaussian,
            0,
            sigma,
        )
        .await
    }
    pub async fn sobel(&self, width: u32, height: u32, input: &[f32]) -> Result<Vec<f32>> {
        self.run(
            width,
            height,
            bytemuck::cast_slice(input),
            &[],
            FilterOperation::Sobel,
            0,
            1.0,
        )
        .await
    }
    /// Signed Hessian eigenvalues in descending order, using central differences.
    /// Pass a Gaussian-smoothed plane to obtain smoothed image curvature.
    pub async fn hessian_eigenvalues(
        &self,
        width: u32,
        height: u32,
        input: &[f32],
    ) -> Result<[Vec<f32>; 2]> {
        let mut eigenvalues = [Vec::new(), Vec::new()];
        for (channel, output) in (0..2).zip(&mut eigenvalues) {
            *output = self
                .run(
                    width,
                    height,
                    bytemuck::cast_slice(input),
                    &[],
                    FilterOperation::HessianEigenvalues,
                    channel,
                    1.0,
                )
                .await?;
        }
        Ok(eigenvalues)
    }
    /// Compute a signed difference of Gaussians for any floating-point plane.
    pub async fn difference_of_gaussians(
        &self,
        width: u32,
        height: u32,
        input: &[f32],
        small_sigma: f32,
        large_sigma: f32,
    ) -> Result<Vec<f32>> {
        ensure!(
            small_sigma < large_sigma,
            "DoG requires small sigma < large sigma"
        );
        let small = self.gaussian(width, height, input, small_sigma).await?;
        let large = self.gaussian(width, height, input, large_sigma).await?;
        self.difference(width, height, &small, &large).await
    }

    /// Signed difference of two Gaussian response planes.
    pub async fn difference(
        &self,
        width: u32,
        height: u32,
        small: &[f32],
        large: &[f32],
    ) -> Result<Vec<f32>> {
        ensure!(small.len() == large.len(), "DoG plane dimensions differ");
        self.run(
            width,
            height,
            bytemuck::cast_slice(small),
            large,
            FilterOperation::DifferenceOfGaussians,
            0,
            1.0,
        )
        .await
    }
}

fn membrane_kernels() -> Vec<f32> {
    let mut kernels = Vec::new();
    for angle in 0..30 {
        let (sin, cos) = (-f64::from(angle) * 6.0).to_radians().sin_cos();
        let mut offsets = Vec::new();
        let mut count = 0.0;
        for y in -9i16..=9 {
            for x in -9i16..=9 {
                let source_x = f64::from(x) * cos - f64::from(y) * sin + 9.0;
                let source_y = f64::from(x) * sin + f64::from(y) * cos + 9.0;
                if (-0.01..19.0).contains(&source_x)
                    && (-0.01..19.0).contains(&source_y)
                    && source_x.round() == 9.0
                {
                    offsets.extend([f32::from(x), f32::from(y)]);
                    count += 1.0;
                }
            }
        }
        kernels.push(count);
        kernels.extend(offsets);
    }
    kernels
}
