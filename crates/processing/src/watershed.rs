use anyhow::Result;
use image::RgbaImage;
use std::cmp::Ordering;
use std::sync::Arc;
use std::{borrow::Cow, collections::BinaryHeap};
use wgpu::{PipelineCompilationOptions, util::DeviceExt};

use super::{
    GpuImageProcessor,
    tiling::{TileConfig, TileGrid, extract_rgba_tile, merge_rgba_tiles},
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
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
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
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
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

    /// # Errors
    /// Returns an error if image sizes overflow, tile data is invalid, or GPU readback fails.
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

        let binary = Self::to_binary_mask(image);
        let distance_map = self.gpu_distance_transform(&binary, width, height).await?;
        let markers = self
            .find_peaks_gpu(&distance_map, width, height, sigma, min_distance)
            .await?;
        let labels = Self::watershed_transform(
            &distance_map,
            &markers,
            usize::try_from(width)?,
            usize::try_from(height)?,
        )?;

        Self::to_rgba_image(&labels, width, height)
    }

    fn to_rgba_image(labels: &[u32], width: u32, height: u32) -> Result<RgbaImage> {
        anyhow::ensure!(
            labels.len() == crate::pixel_count(width, height)?,
            "Label dimensions do not match image"
        );
        let mut result = RgbaImage::new(width, height);
        for (pixel, &label) in result.pixels_mut().zip(labels) {
            *pixel = if label == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            };
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
        let tile_grid = TileGrid::new(full_width, full_height, tile_config);

        let mut processed_tiles = Vec::new();

        for tile_info in &tile_grid.tiles {
            let tile_image = extract_rgba_tile(image, tile_info)?;
            let processed_tile = self
                .apply_watershed_single(&tile_image, sigma, min_distance)
                .await?;
            processed_tiles.push((tile_info.clone(), processed_tile));
        }

        merge_rgba_tiles(&processed_tiles, full_width, full_height)
    }

    fn to_binary_mask(image: &RgbaImage) -> Vec<bool> {
        image
            .pixels()
            .map(|pixel| {
                // Pipeline images are BGRA internally.
                let gray = 0.114f32.mul_add(
                    f32::from(pixel[0]),
                    0.587f32.mul_add(f32::from(pixel[1]), 0.299 * f32::from(pixel[2])),
                ) / 255.0;
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
        let pixel_count = crate::pixel_count(width, height)?;

        let binary_u32: Vec<u32> = binary.iter().map(|&b| u32::from(b)).collect();

        let buffers = self.create_distance_buffers(&binary_u32, pixel_count)?;

        self.run_distance_pass(
            &self.distance_init_pipeline,
            &buffers,
            &buffers.seeds_b,
            &buffers.seeds_a,
            (width, height),
            0,
        )?;

        let max_dimension = width.max(height);
        let mut step_size = max_dimension.next_power_of_two() / 2;
        let mut seeds_are_in_a = true;

        while step_size >= 1 {
            let (seeds_input, seeds_output) = if seeds_are_in_a {
                (&buffers.seeds_a, &buffers.seeds_b)
            } else {
                (&buffers.seeds_b, &buffers.seeds_a)
            };
            self.run_distance_pass(
                &self.distance_jump_pipeline,
                &buffers,
                seeds_input,
                seeds_output,
                (width, height),
                step_size,
            )?;
            seeds_are_in_a = !seeds_are_in_a;
            step_size /= 2;
        }

        let (final_seeds, scratch_seeds) = if seeds_are_in_a {
            (&buffers.seeds_a, &buffers.seeds_b)
        } else {
            (&buffers.seeds_b, &buffers.seeds_a)
        };
        self.run_distance_pass(
            &self.distance_final_pipeline,
            &buffers,
            final_seeds,
            scratch_seeds,
            (width, height),
            0,
        )?;

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

        let seeds_a = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Seeds Buffer A"),
            size: crate::buffer_size(pixel_count, 8)?,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let seeds_b = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Seeds Buffer B"),
            size: crate::buffer_size(pixel_count, 8)?,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let distance_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Distance Buffer"),
            size: crate::buffer_size(pixel_count, 4)?,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Distance Staging Buffer"),
            size: crate::buffer_size(pixel_count, 4)?,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(DistanceBuffers {
            binary: binary_buffer,
            seeds_a,
            seeds_b,
            distance: distance_buffer,
            staging: staging_buffer,
        })
    }

    fn run_distance_pass(
        &self,
        pipeline: &wgpu::ComputePipeline,
        buffers: &DistanceBuffers,
        seeds_input: &wgpu::Buffer,
        seeds_output: &wgpu::Buffer,
        dimensions: (u32, u32),
        step_size: u32,
    ) -> Result<()> {
        let (width, height) = dimensions;
        let uniforms = [width, height, step_size, 0];
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
                        resource: seeds_input.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: seeds_output.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffers.binary.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
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
            crate::buffer_size(pixel_count, 4)?,
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
            .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {e}"))?;
        map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {e:?}"))?;

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
        let pixel_count = crate::pixel_count(width, height)?;

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
            size: crate::buffer_size(pixel_count, 4)?,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size: crate::buffer_size(pixel_count, 4)?,
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
            crate::buffer_size(pixel_count, 4)?,
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
            .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {e}"))?;
        map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {e:?}"))?;

        let data = buffer_slice.get_mapped_range()?;
        let markers: &[u32] = bytemuck::cast_slice(&data);

        Ok(markers.to_vec())
    }

    fn watershed_transform(
        distance_map: &[f32],
        local_maxima_points: &[u32],
        width: usize,
        height: usize,
    ) -> Result<Vec<u32>> {
        let count = width
            .checked_mul(height)
            .ok_or_else(|| anyhow::anyhow!("Image dimensions overflow"))?;
        anyhow::ensure!(
            distance_map.len() == count && local_maxima_points.len() == count,
            "Watershed input dimensions do not match"
        );
        let mut labels = vec![0u32; count];
        let mut priority_queue = BinaryHeap::new();
        let mut age_counter = 0u32;

        for (idx, ((label, &marker), &distance)) in labels
            .iter_mut()
            .zip(local_maxima_points)
            .zip(distance_map)
            .enumerate()
        {
            if marker > 0 {
                *label = marker;
                priority_queue.push(QueueItem {
                    value: distance,
                    age: age_counter,
                    index: idx,
                    source: marker,
                });
            }
        }

        while let Some(current) = priority_queue.pop() {
            *labels
                .get_mut(current.index)
                .ok_or_else(|| anyhow::anyhow!("Invalid watershed queue index"))? = current.source;
            let is_boundary = Self::neighbor_indices(current.index, width, height)
                .filter_map(|i| labels.get(i))
                .any(|&label| label != 0 && label != IN_QUEUE_MARKER && label != current.source);
            if is_boundary {
                *labels
                    .get_mut(current.index)
                    .ok_or_else(|| anyhow::anyhow!("Invalid watershed queue index"))? = 0;
            } else {
                Self::expand_to_neighbors(
                    &mut labels,
                    &mut priority_queue,
                    &mut age_counter,
                    distance_map,
                    &current,
                    width,
                    height,
                )?;
            }
        }
        for label in &mut labels {
            if *label == IN_QUEUE_MARKER {
                *label = 0;
            }
        }
        Ok(labels)
    }

    fn neighbor_indices(index: usize, width: usize, height: usize) -> impl Iterator<Item = usize> {
        const OFFSETS: [(isize, isize); 8] = [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];
        OFFSETS.into_iter().filter_map(move |(dx, dy)| {
            let x = index.checked_rem(width)?.checked_add_signed(dx)?;
            let y = index.checked_div(width)?.checked_add_signed(dy)?;
            if x < width && y < height {
                y.checked_mul(width)?.checked_add(x)
            } else {
                None
            }
        })
    }

    fn expand_to_neighbors(
        labels: &mut [u32],
        priority_queue: &mut BinaryHeap<QueueItem>,
        age_counter: &mut u32,
        distance_map: &[f32],
        current: &QueueItem,
        width: usize,
        height: usize,
    ) -> Result<()> {
        for index in Self::neighbor_indices(current.index, width, height) {
            let label = labels
                .get_mut(index)
                .ok_or_else(|| anyhow::anyhow!("Invalid watershed neighbor"))?;
            let &distance = distance_map
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("Missing watershed distance"))?;
            if *label == 0 && distance > 0.0 {
                *label = IN_QUEUE_MARKER;
                *age_counter = age_counter
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Watershed queue age overflow"))?;
                priority_queue.push(QueueItem {
                    value: distance,
                    age: *age_counter,
                    index,
                    source: current.source,
                });
            }
        }
        Ok(())
    }
}

struct DistanceBuffers {
    binary: wgpu::Buffer,
    seeds_a: wgpu::Buffer,
    seeds_b: wgpu::Buffer,
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
