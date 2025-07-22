use image::RgbaImage;
use std::borrow::Cow;
use wgpu::{util::DeviceExt, PipelineCompilationOptions, Trace};

pub mod compositor;
pub mod contrast;
pub mod gaussian_blur;

pub struct GpuImageProcessor {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuImageProcessor {
    pub async fn new() -> Self {
        // Initialize wgpu
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: Trace::Off,
            })
            .await
            .unwrap();

        Self { device, queue }
    }
}
