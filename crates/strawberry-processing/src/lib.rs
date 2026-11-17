// wgpu auto-trait checks exceed the default compiler recursion depth.
#![recursion_limit = "256"]

use anyhow::{Result, anyhow};
use image::RgbaImage;

pub(crate) mod gpu_error;
use gpu_error::GpuErrors;

pub use gpu_error::{GpuError, GpuErrorKind};
use wgpu::{Trace, util::DeviceExt};

pub mod colorspace;
pub mod compositor;
pub mod contrast;
pub mod decomposer;
pub mod difference_of_gaussians;
pub mod gaussian_blur;
pub mod grayscale;
pub mod local_maxima;
pub mod morphology;
pub mod nervetracer;
pub mod rolling_ball;
pub mod sobel;
pub mod threshold;
pub mod tiling;
pub mod watershed;

pub struct GpuImageProcessor {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    errors: GpuErrors,
}

pub(crate) fn pixel_count(width: u32, height: u32) -> Result<usize> {
    Ok(usize::try_from(
        u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(|| anyhow!("Image dimensions overflow"))?,
    )?)
}

pub(crate) fn buffer_size(pixel_count: usize, bytes_per_pixel: u64) -> Result<u64> {
    u64::try_from(pixel_count)?
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| anyhow!("GPU buffer size overflow"))
}

impl GpuImageProcessor {
    /// # Errors
    /// Returns an error if no suitable GPU adapter or device is available.
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("Failed to find suitable GPU adapter: {e}"))?;

        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: adapter.limits().max_storage_buffer_binding_size,
            max_buffer_size: adapter.limits().max_buffer_size,
            ..wgpu::Limits::default()
        };

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: Trace::Off,
            })
            .await
            .map_err(|e| anyhow!("Failed to create GPU device: {e}"))?;
        let errors = GpuErrors::new(&device);
        println!(
            "GPU Device created max_storage_buffer_binding_size: {:?}",
            device.limits().max_storage_buffer_binding_size
        );

        Ok(Self {
            device,
            queue,
            errors,
        })
    }

    /// Return the next unexpected callback failure for the backend to report.
    #[must_use]
    pub fn take_uncaptured_error(&self) -> Option<GpuError> {
        self.errors.next()
    }

    /// Check whether the device can still accept work.
    ///
    /// # Errors
    /// Returns the recorded device-loss error until the application restarts.
    pub fn ensure_device_availability(&self, operation: &str) -> Result<()> {
        self.errors.check_device_availability(operation)
    }

    // The closure is deliberately synchronous: wgpu 30 error scopes are thread-local
    // and !Send. All guards are consumed before the first suspension point.
    pub(crate) async fn checked<T>(
        &self,
        operation: &str,
        work: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.ensure_device_availability(operation)?;
        let (result, pending) = {
            let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
            let result = work();
            (result, (internal.pop(), memory.pop(), validation.pop()))
        };
        let (internal, memory, validation) = futures::join!(pending.0, pending.1, pending.2);
        self.ensure_device_availability(operation)?;
        let failures: Vec<_> = [memory, internal, validation]
            .into_iter()
            .flatten()
            .collect();
        if let Some(first) = failures.first() {
            let mut error = GpuError::from_wgpu(operation, first);
            error.diagnostic = failures
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            return Err(error.into());
        }
        result
    }

    pub(crate) async fn create_shader_module(
        &self,
        descriptor: wgpu::ShaderModuleDescriptor<'_>,
    ) -> Result<wgpu::ShaderModule> {
        let label = descriptor.label.unwrap_or("Create shader").to_owned();
        self.checked(&label, || Ok(self.device.create_shader_module(descriptor)))
            .await
    }

    pub(crate) async fn create_compute_pipeline(
        &self,
        descriptor: &wgpu::ComputePipelineDescriptor<'_>,
    ) -> Result<wgpu::ComputePipeline> {
        self.checked(
            descriptor.label.unwrap_or("Create compute pipeline"),
            || Ok(self.device.create_compute_pipeline(descriptor)),
        )
        .await
    }

    pub(crate) async fn create_bind_group_layout(
        &self,
        descriptor: &wgpu::BindGroupLayoutDescriptor<'_>,
    ) -> Result<wgpu::BindGroupLayout> {
        self.checked(
            descriptor.label.unwrap_or("Create bind group layout"),
            || Ok(self.device.create_bind_group_layout(descriptor)),
        )
        .await
    }

    pub(crate) async fn create_pipeline_layout(
        &self,
        descriptor: &wgpu::PipelineLayoutDescriptor<'_>,
    ) -> Result<wgpu::PipelineLayout> {
        self.checked(descriptor.label.unwrap_or("Create pipeline layout"), || {
            Ok(self.device.create_pipeline_layout(descriptor))
        })
        .await
    }

    pub(crate) async fn get_bind_group_layout(
        &self,
        pipeline: &wgpu::ComputePipeline,
        index: u32,
    ) -> Result<wgpu::BindGroupLayout> {
        self.checked("Get compute bindings", || {
            Ok(pipeline.get_bind_group_layout(index))
        })
        .await
    }

    pub(crate) async fn create_bind_group(
        &self,
        descriptor: &wgpu::BindGroupDescriptor<'_>,
    ) -> Result<wgpu::BindGroup> {
        self.checked(descriptor.label.unwrap_or("Create bind group"), || {
            Ok(self.device.create_bind_group(descriptor))
        })
        .await
    }

    // Convenience for consecutive buffer bindings starting at zero.
    pub(crate) async fn create_buffer_bind_group(
        &self,
        pipeline: &wgpu::ComputePipeline,
        buffers: &[&wgpu::Buffer],
        label: &str,
    ) -> Result<wgpu::BindGroup> {
        let layout = self.get_bind_group_layout(pipeline, 0).await?;
        let entries = buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| {
                Ok(wgpu::BindGroupEntry {
                    binding: u32::try_from(index)?,
                    resource: buffer.as_entire_binding(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &layout,
            entries: &entries,
        })
        .await
    }

    pub(crate) async fn create_buffer(
        &self,
        descriptor: &wgpu::BufferDescriptor<'_>,
    ) -> Result<wgpu::Buffer> {
        self.checked(descriptor.label.unwrap_or("Create buffer"), || {
            Ok(self.device.create_buffer(descriptor))
        })
        .await
    }

    pub(crate) async fn create_buffer_init(
        &self,
        descriptor: &wgpu::util::BufferInitDescriptor<'_>,
    ) -> Result<wgpu::Buffer> {
        self.checked(descriptor.label.unwrap_or("Initialize buffer"), || {
            Ok(self.device.create_buffer_init(descriptor))
        })
        .await
    }

    /// # Errors
    /// Returns an error if the input buffer cannot be created.
    pub async fn create_input_buffer(&self, image: &RgbaImage) -> Result<wgpu::Buffer> {
        self.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Input Image Buffer"),
            contents: bytemuck::cast_slice(image.as_raw()),
            usage: wgpu::BufferUsages::STORAGE,
        })
        .await
    }

    /// # Errors
    /// Returns an error if the size overflows or the output buffer cannot be created.
    pub async fn create_output_buffer(&self, pixel_count: usize) -> Result<wgpu::Buffer> {
        self.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
            size: crate::buffer_size(pixel_count, 4)?,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
        .await
    }

    /// # Errors
    /// Returns an error if the size overflows or the staging buffer cannot be created.
    pub async fn create_staging_buffer(&self, pixel_count: usize) -> Result<wgpu::Buffer> {
        self.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: crate::buffer_size(pixel_count, 4)?,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
        .await
    }

    /// # Errors
    /// Returns an error if the uniform buffer cannot be created.
    pub async fn create_uniform_buffer<T: bytemuck::Pod + Sync>(
        &self,
        data: &T,
    ) -> Result<wgpu::Buffer> {
        self.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: bytemuck::cast_slice(std::slice::from_ref(data)),
            usage: wgpu::BufferUsages::UNIFORM,
        })
        .await
    }

    pub(crate) async fn create_command_encoder(
        &self,
        descriptor: &wgpu::CommandEncoderDescriptor<'_>,
    ) -> Result<wgpu::CommandEncoder> {
        self.checked(descriptor.label.unwrap_or("Create command encoder"), || {
            Ok(self.device.create_command_encoder(descriptor))
        })
        .await
    }

    /// # Errors
    /// Returns an error if the compute pass cannot be encoded.
    pub async fn dispatch_compute_with_encoder(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        bind_group: &wgpu::BindGroup,
        width: u32,
        height: u32,
        pass_label: &str,
    ) -> Result<()> {
        self.checked(pass_label, || {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(pass_label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
            Ok(())
        })
        .await
    }

    pub(crate) async fn submit(&self, encoder: wgpu::CommandEncoder) -> Result<()> {
        self.checked("Submit GPU commands", || {
            self.queue.submit([encoder.finish()]);
            Ok(())
        })
        .await
    }

    pub(crate) async fn copy_buffer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Buffer,
        destination: &wgpu::Buffer,
        size: u64,
    ) -> Result<()> {
        self.checked("Copy GPU buffer", || {
            encoder.copy_buffer_to_buffer(source, 0, destination, 0, size);
            Ok(())
        })
        .await
    }

    pub(crate) async fn poll(&self) -> Result<()> {
        self.checked("Wait for GPU completion", || {
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map(|_| ())
                .map_err(|error| {
                    GpuError::new(
                        GpuErrorKind::Polling,
                        "Wait for GPU completion",
                        error.to_string(),
                    )
                    .into()
                })
        })
        .await
    }

    pub(crate) async fn read_mapped_buffer<T: bytemuck::Pod + Send>(
        &self,
        staging: &wgpu::Buffer,
    ) -> Result<Vec<T>> {
        let (_mapping, receiver) = self
            .checked("Map GPU readback", || {
                let mapping = UnmapOnDrop {
                    device: &self.device,
                    buffer: staging,
                };
                let (sender, receiver) = flume::bounded(1);
                staging
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = sender.try_send(result);
                    });
                Ok((mapping, receiver))
            })
            .await?;
        self.poll().await?;
        receiver
            .recv_async()
            .await
            .map_err(|error| {
                GpuError::new(
                    GpuErrorKind::Readback,
                    "Receive GPU readback",
                    error.to_string(),
                )
            })?
            .map_err(|error| {
                GpuError::new(
                    GpuErrorKind::Readback,
                    "Map GPU readback",
                    error.to_string(),
                )
            })?;
        self.checked("Read GPU buffer", || {
            let data = staging.slice(..).get_mapped_range().map_err(|error| {
                GpuError::new(GpuErrorKind::Readback, "Read GPU buffer", error.to_string())
            })?;
            let values = bytemuck::try_cast_slice(&data).map_err(|error| {
                GpuError::new(
                    GpuErrorKind::Readback,
                    "Decode GPU readback",
                    error.to_string(),
                )
            })?;
            Ok(values.to_vec())
        })
        .await
    }

    pub(crate) async fn read_buffer<T: bytemuck::Pod + Send>(
        &self,
        output: &wgpu::Buffer,
        staging: &wgpu::Buffer,
        size: u64,
    ) -> Result<Vec<T>> {
        let mut encoder = self
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Buffer Copy Encoder"),
            })
            .await?;
        self.copy_buffer(&mut encoder, output, staging, size)
            .await?;
        self.submit(encoder).await?;
        self.read_mapped_buffer(staging).await
    }

    /// # Errors
    /// Returns an error if image sizes overflow, image data is invalid, or GPU readback fails.
    pub async fn read_buffer_to_image(
        &self,
        output_buffer: &wgpu::Buffer,
        staging_buffer: &wgpu::Buffer,
        width: u32,
        height: u32,
        pixel_count: usize,
    ) -> Result<RgbaImage> {
        let bytes = self
            .read_buffer(
                output_buffer,
                staging_buffer,
                crate::buffer_size(pixel_count, 4)?,
            )
            .await?;
        RgbaImage::from_raw(width, height, bytes)
            .ok_or_else(|| anyhow!("Failed to create image from GPU buffer data"))
    }
}

// Unmapping also cancels a pending mapping. This synchronous cleanup is required if
// readback is cancelled or fails, and runs after every mapped view has been dropped.
struct UnmapOnDrop<'a> {
    device: &'a wgpu::Device,
    buffer: &'a wgpu::Buffer,
}
impl Drop for UnmapOnDrop<'_> {
    fn drop(&mut self) {
        // A failed mapping is already idle, and unmap then produces a secondary
        // validation error. Cancellation races with that completion, so cleanup
        // captures and discards this secondary error; readback reports the cause.
        let cleanup = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.buffer.unmap();
        drop(cleanup);
    }
}

pub mod float_filters;
pub mod segmentation;
