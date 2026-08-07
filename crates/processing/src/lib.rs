use anyhow::{Result, anyhow};
use image::RgbaImage;
use wgpu::{Trace, util::DeviceExt};

pub mod colorspace;
pub mod compositor;
pub mod contrast;
pub mod decomposer;
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
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuImageProcessor {
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
            .map_err(|e| anyhow!("Failed to find suitable GPU adapter: {}", e))?;

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
            .map_err(|e| anyhow!("Failed to create GPU device: {}", e))?;
        println!(
            "GPU Device created max_storage_buffer_binding_size: {:?}",
            device.limits().max_storage_buffer_binding_size
        );

        Ok(Self { device, queue })
    }

    // Helper methods for common GPU operations
    pub fn create_input_buffer(&self, image: &RgbaImage) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Input Image Buffer"),
                contents: bytemuck::cast_slice(image.as_raw()),
                usage: wgpu::BufferUsages::STORAGE,
            })
    }

    pub fn create_output_buffer(&self, pixel_count: usize) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Output Buffer"),
            size: (pixel_count * 4) as u64, // 4 bytes per RGBA pixel
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    pub fn create_staging_buffer(&self, pixel_count: usize) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn create_uniform_buffer<T: bytemuck::Pod>(&self, data: &T) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Uniform Buffer"),
                contents: bytemuck::cast_slice(std::slice::from_ref(data)),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    pub fn dispatch_compute_with_encoder(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        bind_group: &wgpu::BindGroup,
        width: u32,
        height: u32,
        pass_label: &str,
    ) {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(pass_label),
            timestamp_writes: None,
        });

        compute_pass.set_pipeline(pipeline);
        compute_pass.set_bind_group(0, bind_group, &[]);

        // Dispatch with 8x8 work groups (matching the shader workgroup size)
        let workgroup_size = 8;
        let dispatch_x = width.div_ceil(workgroup_size);
        let dispatch_y = height.div_ceil(workgroup_size);
        compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
    }

    pub async fn read_buffer_to_image(
        &self,
        output_buffer: &wgpu::Buffer,
        staging_buffer: &wgpu::Buffer,
        width: u32,
        height: u32,
        pixel_count: usize,
    ) -> Result<RgbaImage> {
        // Create encoder for copy operation
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Buffer Copy Encoder"),
            });

        // Copy result to staging buffer
        encoder.copy_buffer_to_buffer(
            output_buffer,
            0,
            staging_buffer,
            0,
            (pixel_count * 4) as u64,
        );

        // Submit commands
        self.queue.submit(std::iter::once(encoder.finish()));

        // Read back results
        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = flume::unbounded();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        let map_result = receiver
            .recv_async()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {}", e))?;
        map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {:?}", e))?;

        let data = buffer_slice.get_mapped_range()?;
        let result_bytes: &[u8] = &data;

        // Create result image from the GPU buffer data
        RgbaImage::from_raw(width, height, result_bytes.to_vec())
            .ok_or_else(|| anyhow::anyhow!("Failed to create image from GPU buffer data"))
    }
}
