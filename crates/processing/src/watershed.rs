use anyhow::Result;
use image::RgbaImage;
use std::cmp::Ordering;
use std::sync::Arc;
use std::{borrow::Cow, collections::BinaryHeap};
use wgpu::{util::DeviceExt, PipelineCompilationOptions};

use super::{
    tiling::{extract_rgba_tile, merge_rgba_tiles, TileConfig, TileGrid},
    GpuImageProcessor,
};

const WORKGROUP_SIZE: u32 = 8;
const GRAYSCALE_THRESHOLD: f32 = 0.5;
const IN_QUEUE_MARKER: u32 = u32::MAX;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WatershedUniforms {
    width: u32,
    height: u32,
    sigma: f32,
    min_distance: u32,
}

/// GPU-accelerated watershed segmentation processor.
pub struct GpuWatershed {
    watershed_pipeline: wgpu::ComputePipeline,
    distance_init_pipeline: wgpu::ComputePipeline,
    distance_jump_pipeline: wgpu::ComputePipeline,
    distance_final_pipeline: wgpu::ComputePipeline,
    gpu: Arc<GpuImageProcessor>,
}

impl GpuWatershed {
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        let watershed_pipeline = Self::create_watershed_pipeline(&gpu);
        let (distance_init_pipeline, distance_jump_pipeline, distance_final_pipeline) =
            Self::create_distance_pipelines(&gpu);

        Self {
            watershed_pipeline,
            distance_init_pipeline,
            distance_jump_pipeline,
            distance_final_pipeline,
            gpu,
        }
    }

    fn create_watershed_pipeline(gpu: &GpuImageProcessor) -> wgpu::ComputePipeline {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Watershed Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/watershed.wgsl"
                ))),
            });

        gpu.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Watershed Pipeline"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: PipelineCompilationOptions::default(),
                cache: None,
            })
    }

    fn create_distance_pipelines(
        gpu: &GpuImageProcessor,
    ) -> (
        wgpu::ComputePipeline,
        wgpu::ComputePipeline,
        wgpu::ComputePipeline,
    ) {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Distance Transform Shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "shaders/distance_transform.wgsl"
                ))),
            });

        let bind_group_layout =
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Distance Transform Bind Group Layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });

        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Distance Transform Pipeline Layout"),
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });

        let create_pipeline = |entry_point: &str, label: &str| {
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: PipelineCompilationOptions::default(),
                    cache: None,
                })
        };

        (
            create_pipeline("init_seeds", "Distance Init Pipeline"),
            create_pipeline("jump_flood", "Distance Jump Pipeline"),
            create_pipeline("finalize_distances", "Distance Final Pipeline"),
        )
    }

    pub async fn apply_watershed(
        &self,
        image: &RgbaImage,
        sigma: f32,
        min_distance: u32,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();
        let tile_config = TileConfig::from_limits(&self.gpu.device.limits());

        if TileGrid::needs_tiling(width, height, &tile_config) {
            self.apply_watershed_tiled(image, &tile_config, sigma, min_distance)
                .await
        } else {
            self.apply_watershed_single(image, sigma, min_distance)
                .await
        }
    }

    async fn apply_watershed_single(
        &self,
        image: &RgbaImage,
        sigma: f32,
        min_distance: u32,
    ) -> Result<RgbaImage> {
        let (width, height) = image.dimensions();

        let binary = self.to_binary_mask(image);
        let distance_map = self.gpu_distance_transform(&binary, width, height).await?;
        let markers = self
            .find_peaks_gpu(&distance_map, width, height, sigma, min_distance)
            .await?;
        let labels =
            self.watershed_transform(&distance_map, &markers, width as usize, height as usize);

        self.to_rgba_image(&labels, width, height)
    }

    fn to_rgba_image(&self, labels: &[u32], width: u32, height: u32) -> Result<RgbaImage> {
        let mut result = RgbaImage::new(width, height);

        for (idx, &label) in labels.iter().enumerate() {
            let x = (idx % width as usize) as u32;
            let y = (idx / width as usize) as u32;

            let color = if label == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            };

            result.put_pixel(x, y, color);
        }

        Ok(result)
    }

    async fn apply_watershed_tiled(
        &self,
        image: &RgbaImage,
        tile_config: &TileConfig,
        sigma: f32,
        min_distance: u32,
    ) -> Result<RgbaImage> {
        let (full_width, full_height) = image.dimensions();
        let tile_grid = TileGrid::new(full_width, full_height, tile_config.clone());

        let mut processed_tiles = Vec::new();

        for tile_info in &tile_grid.tiles {
            let tile_image = extract_rgba_tile(image, tile_info);
            let processed_tile = self
                .apply_watershed_single(&tile_image, sigma, min_distance)
                .await?;
            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }

    fn to_binary_mask(&self, image: &RgbaImage) -> Vec<bool> {
        image
            .pixels()
            .map(|pixel| {
                let gray =
                    (0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32)
                        / 255.0;
                gray > GRAYSCALE_THRESHOLD
            })
            .collect()
    }

    async fn gpu_distance_transform(
        &self,
        binary: &[bool],
        width: u32,
        height: u32,
    ) -> Result<Vec<f32>> {
        let pixel_count = (width * height) as usize;

        let binary_u32: Vec<u32> = binary
            .iter()
            .map(|&b| if b { 1u32 } else { 0u32 })
            .collect();

        let buffers = self.create_distance_buffers(&binary_u32, pixel_count)?;

        self.run_distance_pass(&self.distance_init_pipeline, &buffers, width, height, 0, 0)?;

        let max_dimension = width.max(height);
        let mut step_size = max_dimension.next_power_of_two() / 2;

        while step_size >= 1 {
            self.run_distance_pass(
                &self.distance_jump_pipeline,
                &buffers,
                width,
                height,
                step_size,
                0,
            )?;
            step_size /= 2;
        }

        self.run_distance_pass(&self.distance_final_pipeline, &buffers, width, height, 0, 0)?;

        self.read_distance_results(&buffers.distance, &buffers.staging, pixel_count)
            .await
    }

    fn create_distance_buffers(
        &self,
        binary_u32: &[u32],
        pixel_count: usize,
    ) -> Result<DistanceBuffers> {
        let binary_buffer = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Binary Buffer"),
                contents: bytemuck::cast_slice(binary_u32),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let seeds_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Seeds Buffer"),
            size: (pixel_count * 8) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let distance_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Distance Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Distance Staging Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(DistanceBuffers {
            binary: binary_buffer,
            seeds: seeds_buffer,
            distance: distance_buffer,
            staging: staging_buffer,
        })
    }

    fn run_distance_pass(
        &self,
        pipeline: &wgpu::ComputePipeline,
        buffers: &DistanceBuffers,
        width: u32,
        height: u32,
        step_size: u32,
        iteration: u32,
    ) -> Result<()> {
        let uniforms = [width, height, step_size, iteration];
        let uniform_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Distance Uniform Buffer"),
                    contents: bytemuck::cast_slice(&uniforms),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

        let bind_group_layout = pipeline.get_bind_group_layout(0);
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
                        resource: buffers.seeds.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffers.binary.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffers.distance.as_entire_binding(),
                    },
                ],
                label: Some("Distance Transform Bind Group"),
            });

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Distance Transform Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Distance Transform Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            let dispatch_x = width.div_ceil(WORKGROUP_SIZE);
            let dispatch_y = height.div_ceil(WORKGROUP_SIZE);
            compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
        }

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }

    async fn read_distance_results(
        &self,
        distance_buffer: &wgpu::Buffer,
        staging_buffer: &wgpu::Buffer,
        pixel_count: usize,
    ) -> Result<Vec<f32>> {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Distance Copy Encoder"),
            });

        encoder.copy_buffer_to_buffer(
            distance_buffer,
            0,
            staging_buffer,
            0,
            (pixel_count * 4) as u64,
        );

        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = flume::unbounded();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        let map_result = receiver
            .recv_async()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {}", e))?;
        map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {:?}", e))?;

        let data = buffer_slice.get_mapped_range()?;
        let distances: &[f32] = bytemuck::cast_slice(&data);

        Ok(distances.to_vec())
    }

    async fn find_peaks_gpu(
        &self,
        distance: &[f32],
        width: u32,
        height: u32,
        sigma: f32,
        min_distance: u32,
    ) -> Result<Vec<u32>> {
        let pixel_count = (width * height) as usize;

        let distance_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Distance Buffer"),
                    contents: bytemuck::cast_slice(distance),
                    usage: wgpu::BufferUsages::STORAGE,
                });

        let output_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Markers Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: (pixel_count * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniforms = WatershedUniforms {
            width,
            height,
            sigma,
            min_distance,
        };

        let uniform_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Watershed Uniform Buffer"),
                    contents: bytemuck::cast_slice(&[uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });

        let bind_group_layout = self.watershed_pipeline.get_bind_group_layout(0);
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
                        resource: distance_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Watershed Bind Group"),
            });

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Watershed Encoder"),
            });

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Watershed Pass"),
                timestamp_writes: None,
            });

            compute_pass.set_pipeline(&self.watershed_pipeline);
            compute_pass.set_bind_group(0, &bind_group, &[]);

            let dispatch_x = width.div_ceil(WORKGROUP_SIZE);
            let dispatch_y = height.div_ceil(WORKGROUP_SIZE);
            compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
        }

        encoder.copy_buffer_to_buffer(
            &output_buffer,
            0,
            &staging_buffer,
            0,
            (pixel_count * 4) as u64,
        );

        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        let (sender, receiver) = flume::unbounded();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });

        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
        let map_result = receiver
            .recv_async()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {}", e))?;
        map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {:?}", e))?;

        let data = buffer_slice.get_mapped_range()?;
        let markers: &[u32] = bytemuck::cast_slice(&data);

        Ok(markers.to_vec())
    }

    fn watershed_transform(
        &self,
        distance_map: &[f32],
        local_maxima_points: &[u32],
        width: usize,
        height: usize,
    ) -> Vec<u32> {
        let mut labels = vec![0u32; width * height];
        let mut priority_queue = BinaryHeap::new();
        let mut age_counter = 0u32;

        // Initialize queue with markers
        for (idx, &marker_label) in local_maxima_points.iter().enumerate() {
            if marker_label > 0 {
                labels[idx] = marker_label;
                priority_queue.push(QueueItem {
                    value: distance_map[idx],
                    age: age_counter,
                    index: idx,
                    source: marker_label,
                });
            }
        }

        // Flooding loop
        while let Some(current_pixel) = priority_queue.pop() {
            labels[current_pixel.index] = current_pixel.source;

            let x = (current_pixel.index % width) as i32;
            let y = (current_pixel.index / width) as i32;

            let is_watershed_line =
                Self::check_watershed_boundary(&labels, current_pixel.source, x, y, width);

            if is_watershed_line {
                labels[current_pixel.index] = 0;
            } else {
                Self::expand_to_neighbors(
                    &mut labels,
                    &mut priority_queue,
                    &mut age_counter,
                    distance_map,
                    &current_pixel,
                    x,
                    y,
                    width,
                    height,
                );
            }
        }

        // Cleanup any remaining IN_QUEUE markers
        for label in labels.iter_mut() {
            if *label == IN_QUEUE_MARKER {
                *label = 0;
            }
        }

        labels
    }

    fn check_watershed_boundary(
        labels: &[u32],
        current_source: u32,
        x: i32,
        y: i32,
        width: usize,
    ) -> bool {
        const OFFSETS: [(i32, i32); 8] = [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];

        for (dx, dy) in &OFFSETS {
            let nx = x + dx;
            let ny = y + dy;

            if nx >= 0 && nx < width as i32 && ny >= 0 {
                let neighbor_index = (ny as usize * width) + nx as usize;
                if neighbor_index < labels.len() {
                    let neighbor_label = labels[neighbor_index];
                    if neighbor_label != 0
                        && neighbor_label != IN_QUEUE_MARKER
                        && neighbor_label != current_source
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn expand_to_neighbors(
        labels: &mut [u32],
        priority_queue: &mut BinaryHeap<QueueItem>,
        age_counter: &mut u32,
        distance_map: &[f32],
        current_pixel: &QueueItem,
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) {
        const OFFSETS: [(i32, i32); 8] = [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];

        for (dx, dy) in &OFFSETS {
            let nx = x + dx;
            let ny = y + dy;

            if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
                let neighbor_index = (ny as usize * width) + nx as usize;

                if labels[neighbor_index] == 0 && distance_map[neighbor_index] > 0.0 {
                    labels[neighbor_index] = IN_QUEUE_MARKER;
                    *age_counter += 1;
                    priority_queue.push(QueueItem {
                        value: distance_map[neighbor_index],
                        age: *age_counter,
                        index: neighbor_index,
                        source: current_pixel.source,
                    });
                }
            }
        }
    }
}

struct DistanceBuffers {
    binary: wgpu::Buffer,
    seeds: wgpu::Buffer,
    distance: wgpu::Buffer,
    staging: wgpu::Buffer,
}

#[derive(Debug, Clone)]
struct QueueItem {
    value: f32,
    age: u32,
    index: usize,
    source: u32,
}

impl PartialEq for QueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && self.age == other.age
    }
}

impl Eq for QueueItem {}

impl PartialOrd for QueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueueItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value
            .partial_cmp(&other.value)
            .unwrap_or(Ordering::Equal)
            .then_with(|| other.age.cmp(&self.age))
    }
}
