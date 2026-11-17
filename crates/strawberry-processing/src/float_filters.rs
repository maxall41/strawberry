//! Reusable floating-point GPU filters. Values are never clipped to display range.
use crate::GpuImageProcessor;
use anyhow::{Result, ensure};
use std::{borrow::Cow, sync::Arc};
use wgpu::{CommandEncoderDescriptor, PipelineCompilationOptions};

/// Channel to extract from internal BGRA pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterChannel {
    Luminance,
    Red,
    Green,
    Blue,
}

#[derive(Clone, Copy)]
enum FilterOperation {
    Channel,
    Gaussian,
    Sobel,
    DifferenceOfGaussians,
    HessianEigenvalues,
    MembraneProjections,
}

impl FilterOperation {
    /// Return the shader operation identifier for this filter.
    const fn code(self) -> u32 {
        match self {
            Self::Channel => 0,
            Self::Gaussian => 1,
            Self::Sobel => 2,
            Self::DifferenceOfGaussians => 3,
            Self::HessianEigenvalues => 4,
            Self::MembraneProjections => 5,
        }
    }
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
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        let shader = gpu
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Floating point filters"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(concat!(
                    include_str!("shaders/gaussian_blur_math.wgsl"),
                    include_str!("shaders/sobel_math.wgsl"),
                    include_str!("shaders/hessian_math.wgsl"),
                    include_str!("shaders/difference_of_gaussians.wgsl"),
                    include_str!("shaders/float_filters.wgsl")
                ))),
            })
            .await?;
        let pipeline = gpu
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Floating point filters"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
            .await?;
        Ok(Self {
            gpu,
            pipeline,
            membrane_kernels: membrane_kernels(),
        })
    }
    /// Dispatch a floating-point filter and read back its output planes.
    async fn run(
        &self,
        (width, height): (u32, u32),
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
            bytes <= self.gpu.device.limits().max_storage_buffer_binding_size,
            "Tile exceeds GPU buffer limits"
        );
        let a = self
            .gpu
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: input,
                usage: wgpu::BufferUsages::STORAGE,
            })
            .await?;
        let dummy = [0.0f32];
        let b = self
            .gpu
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(if second.is_empty() { &dummy } else { second }),
                usage: wgpu::BufferUsages::STORAGE,
            })
            .await?;
        let out = self
            .gpu
            .create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
            .await?;
        let staging = self
            .gpu
            .create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
            .await?;
        let params = self
            .gpu
            .create_uniform_buffer(&Params {
                width,
                height,
                operation: operation.code(),
                channel,
                sigma,
                radius: (sigma * 3.0).ceil(),
                pad: [0; 2],
            })
            .await?;
        let group = self
            .gpu
            .create_buffer_bind_group(
                &self.pipeline,
                &[&params, &out, &a, &b],
                "Floating point filter bindings",
            )
            .await?;
        let mut encoder = self
            .gpu
            .create_command_encoder(&CommandEncoderDescriptor::default())
            .await?;
        self.gpu
            .dispatch_compute_with_encoder(
                &mut encoder,
                &self.pipeline,
                &group,
                width,
                height,
                "Float filter",
            )
            .await?;
        self.gpu
            .copy_buffer(&mut encoder, &out, &staging, bytes)
            .await?;
        self.gpu.submit(encoder).await?;
        self.gpu.read_mapped_buffer(&staging).await
    }
    /// Extract luminance or a color channel from internal BGRA pixels.
    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
    pub async fn channel(
        &self,
        image: &image::RgbaImage,
        channel: FilterChannel,
    ) -> Result<Vec<f32>> {
        let channel = match channel {
            FilterChannel::Luminance => 0,
            FilterChannel::Red => 1,
            FilterChannel::Green => 2,
            FilterChannel::Blue => 3,
        };
        self.run(
            image.dimensions(),
            image.as_raw(),
            &[],
            FilterOperation::Channel,
            channel,
            1.0,
        )
        .await
    }
    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
    pub async fn membrane_projections(
        &self,
        width: u32,
        height: u32,
        input: &[f32],
    ) -> Result<[Vec<f32>; 6]> {
        let values = self
            .run(
                (width, height),
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

    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
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
            (width, height),
            bytemuck::cast_slice(input),
            &[],
            FilterOperation::Gaussian,
            0,
            sigma,
        )
        .await
    }
    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
    pub async fn sobel(&self, width: u32, height: u32, input: &[f32]) -> Result<Vec<f32>> {
        self.run(
            (width, height),
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
    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
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
                    (width, height),
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
    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
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
    /// # Errors
    /// Returns an error if input dimensions or parameters are invalid, or GPU processing fails.
    pub async fn difference(
        &self,
        width: u32,
        height: u32,
        small: &[f32],
        large: &[f32],
    ) -> Result<Vec<f32>> {
        ensure!(small.len() == large.len(), "DoG plane dimensions differ");
        self.run(
            (width, height),
            bytemuck::cast_slice(small),
            large,
            FilterOperation::DifferenceOfGaussians,
            0,
            1.0,
        )
        .await
    }
}

/// Build rotated line-kernel offsets for the membrane projection filters.
fn membrane_kernels() -> Vec<f32> {
    let mut kernels = Vec::new();
    for angle in 0..30 {
        let (sin, cos) = (-f64::from(angle) * 6.0).to_radians().sin_cos();
        let mut offsets = Vec::new();
        let mut count = 0.0;
        for y in -9i16..=9 {
            for x in -9i16..=9 {
                let source_x = f64::mul_add(f64::from(y), -sin, f64::from(x) * cos) + 9.0;
                let source_y = f64::mul_add(f64::from(y), cos, f64::from(x) * sin) + 9.0;
                if (8.5..9.5).contains(&source_x) && (-0.01..19.0).contains(&source_y) {
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
