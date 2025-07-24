// use anyhow::Result;
// use image::RgbaImage;
// use std::borrow::Cow;
// use std::cmp::Ordering;
// use std::collections::BinaryHeap;
// use std::sync::Arc;
// use wgpu::{util::DeviceExt, PipelineCompilationOptions};

// use super::GpuImageProcessor;

// #[derive(Debug, Clone, PartialEq)]
// pub struct WatershedConfig {
//     pub sigma: f32,
//     pub min_distance: u32,
// }

// #[repr(C)]
// #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
// struct WatershedUniforms {
//     width: u32,
//     height: u32,
//     sigma: f32,
//     min_distance: u32,
// }

// impl Default for WatershedConfig {
//     fn default() -> Self {
//         Self {
//             sigma: 0.0,
//             min_distance: 3,
//         }
//     }
// }

// pub struct GpuWatershed {
//     watershed_pipeline: wgpu::ComputePipeline,
//     distance_init_pipeline: wgpu::ComputePipeline,
//     distance_jump_pipeline: wgpu::ComputePipeline,
//     distance_final_pipeline: wgpu::ComputePipeline,
//     gpu: Arc<GpuImageProcessor>,
//     config: WatershedConfig,
// }

// impl GpuWatershed {
//     pub async fn new(config: WatershedConfig, gpu: Arc<GpuImageProcessor>) -> Self {
//         // Watershed shader
//         let watershed_shader = gpu
//             .device
//             .create_shader_module(wgpu::ShaderModuleDescriptor {
//                 label: Some("Watershed Shader"),
//                 source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
//                     "shaders/watershed.wgsl"
//                 ))),
//             });

//         let watershed_pipeline =
//             gpu.device
//                 .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
//                     label: Some("Watershed Pipeline"),
//                     layout: None,
//                     module: &watershed_shader,
//                     entry_point: Some("main"),
//                     compilation_options: PipelineCompilationOptions::default(),
//                     cache: None,
//                 });

//         // Distance transform shader
//         let distance_shader = gpu
//             .device
//             .create_shader_module(wgpu::ShaderModuleDescriptor {
//                 label: Some("Distance Transform Shader"),
//                 source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
//                     "shaders/distance_transform.wgsl"
//                 ))),
//             });

//         let distance_init_pipeline =
//             gpu.device
//                 .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
//                     label: Some("Distance Init Pipeline"),
//                     layout: None,
//                     module: &distance_shader,
//                     entry_point: Some("init_seeds"),
//                     compilation_options: PipelineCompilationOptions::default(),
//                     cache: None,
//                 });

//         let distance_jump_pipeline =
//             gpu.device
//                 .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
//                     label: Some("Distance Jump Pipeline"),
//                     layout: None,
//                     module: &distance_shader,
//                     entry_point: Some("jump_flood"),
//                     compilation_options: PipelineCompilationOptions::default(),
//                     cache: None,
//                 });

//         let distance_final_pipeline =
//             gpu.device
//                 .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
//                     label: Some("Distance Final Pipeline"),
//                     layout: None,
//                     module: &distance_shader,
//                     entry_point: Some("finalize_distances"),
//                     compilation_options: PipelineCompilationOptions::default(),
//                     cache: None,
//                 });

//         Self {
//             watershed_pipeline,
//             distance_init_pipeline,
//             distance_jump_pipeline,
//             distance_final_pipeline,
//             gpu,
//             config,
//         }
//     }

//     pub async fn apply_watershed(&self, image: &RgbaImage) -> Result<RgbaImage> {
//         let (width, height) = image.dimensions();

//         let binary = self.to_binary_mask(image);
//         let distance_map = self.gpu_distance_transform(&binary, width, height).await?;

//         // Use GPU shader to find local maxima with optional blur
//         let markers = self.find_peaks_gpu(&distance_map, width, height).await?;

//         let labels = self.watershed_transform(
//             &distance_map,
//             &markers[..],
//             &binary,
//             width as usize,
//             height as usize,
//         );
//         let result =
//             self.extract_watershed_result(&binary, &labels, width as usize, height as usize);
//         self.to_rgba_image(&result, width, height)
//     }

//     fn to_binary_mask(&self, image: &RgbaImage) -> Vec<bool> {
//         image
//             .pixels()
//             .map(|pixel| {
//                 // Convert to grayscale and threshold
//                 let gray =
//                     (0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32)
//                         / 255.0;
//                 gray > 0.5
//             })
//             .collect()
//     }

//     async fn gpu_distance_transform(
//         &self,
//         binary: &[bool],
//         width: u32,
//         height: u32,
//     ) -> Result<Vec<f32>> {
//         let pixel_count = (width * height) as usize;

//         // Convert binary to u32 buffer
//         let binary_u32: Vec<u32> = binary
//             .iter()
//             .map(|&b| if b { 1u32 } else { 0u32 })
//             .collect();

//         // Create buffers
//         let binary_buffer = self
//             .gpu
//             .device
//             .create_buffer_init(&wgpu::util::BufferInitDescriptor {
//                 label: Some("Binary Buffer"),
//                 contents: bytemuck::cast_slice(&binary_u32),
//                 usage: wgpu::BufferUsages::STORAGE,
//             });

//         let seeds_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
//             label: Some("Seeds Buffer"),
//             size: (pixel_count * 8) as u64, // 8 bytes per vec2<i32>
//             usage: wgpu::BufferUsages::STORAGE,
//             mapped_at_creation: false,
//         });

//         let distance_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
//             label: Some("Distance Buffer"),
//             size: (pixel_count * 4) as u64, // 4 bytes per f32
//             usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
//             mapped_at_creation: false,
//         });

//         let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
//             label: Some("Distance Staging Buffer"),
//             size: (pixel_count * 4) as u64,
//             usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
//             mapped_at_creation: false,
//         });

//         // Initialize
//         self.run_distance_pass(
//             &self.distance_init_pipeline,
//             &binary_buffer,
//             &seeds_buffer,
//             &distance_buffer,
//             width,
//             height,
//             0,
//             0,
//         );

//         // Jump flooding iterations
//         let max_dimension = width.max(height);
//         let mut step_size = max_dimension.next_power_of_two() / 2;

//         while step_size >= 1 {
//             self.run_distance_pass(
//                 &self.distance_jump_pipeline,
//                 &binary_buffer,
//                 &seeds_buffer,
//                 &distance_buffer,
//                 width,
//                 height,
//                 step_size,
//                 0,
//             );
//             step_size /= 2;
//         }

//         // Finalize
//         self.run_distance_pass(
//             &self.distance_final_pipeline,
//             &binary_buffer,
//             &seeds_buffer,
//             &distance_buffer,
//             width,
//             height,
//             0,
//             0,
//         );

//         // Copy to staging buffer
//         let mut encoder = self
//             .gpu
//             .device
//             .create_command_encoder(&wgpu::CommandEncoderDescriptor {
//                 label: Some("Distance Copy Encoder"),
//             });

//         encoder.copy_buffer_to_buffer(
//             &distance_buffer,
//             0,
//             &staging_buffer,
//             0,
//             (pixel_count * 4) as u64,
//         );

//         self.gpu.queue.submit(std::iter::once(encoder.finish()));

//         // Read back results
//         let buffer_slice = staging_buffer.slice(..);
//         let (sender, receiver) = flume::unbounded();
//         buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
//             if let Err(e) = sender.send(result) {
//                 eprintln!("Failed to send GPU buffer mapping result: {}", e);
//             }
//         });

//         self.gpu.device.poll(wgpu::MaintainBase::Wait);
//         let map_result = receiver
//             .recv_async()
//             .await
//             .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {}", e))?;
//         map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {:?}", e))?;

//         let data = buffer_slice.get_mapped_range();
//         let distances: &[f32] = bytemuck::cast_slice(&data);

//         Ok(distances.to_vec())
//     }

//     fn run_distance_pass(
//         &self,
//         pipeline: &wgpu::ComputePipeline,
//         binary_buffer: &wgpu::Buffer,
//         seeds_buffer: &wgpu::Buffer,
//         distance_buffer: &wgpu::Buffer,
//         width: u32,
//         height: u32,
//         step_size: u32,
//         iteration: u32,
//     ) {
//         let uniforms = [width, height, step_size, iteration];
//         let uniform_buffer =
//             self.gpu
//                 .device
//                 .create_buffer_init(&wgpu::util::BufferInitDescriptor {
//                     label: Some("Distance Uniform Buffer"),
//                     contents: bytemuck::cast_slice(&uniforms),
//                     usage: wgpu::BufferUsages::UNIFORM,
//                 });

//         let bind_group_layout = pipeline.get_bind_group_layout(0);
//         let bind_group = self
//             .gpu
//             .device
//             .create_bind_group(&wgpu::BindGroupDescriptor {
//                 layout: &bind_group_layout,
//                 entries: &[
//                     wgpu::BindGroupEntry {
//                         binding: 0,
//                         resource: uniform_buffer.as_entire_binding(),
//                     },
//                     wgpu::BindGroupEntry {
//                         binding: 1,
//                         resource: seeds_buffer.as_entire_binding(),
//                     },
//                     wgpu::BindGroupEntry {
//                         binding: 2,
//                         resource: binary_buffer.as_entire_binding(),
//                     },
//                     wgpu::BindGroupEntry {
//                         binding: 3,
//                         resource: distance_buffer.as_entire_binding(),
//                     },
//                 ],
//                 label: Some("Distance Transform Bind Group"),
//             });

//         let mut encoder = self
//             .gpu
//             .device
//             .create_command_encoder(&wgpu::CommandEncoderDescriptor {
//                 label: Some("Distance Transform Encoder"),
//             });

//         {
//             let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
//                 label: Some("Distance Transform Pass"),
//                 timestamp_writes: None,
//             });

//             compute_pass.set_pipeline(pipeline);
//             compute_pass.set_bind_group(0, &bind_group, &[]);

//             let workgroup_size = 8;
//             let dispatch_x = (width + workgroup_size - 1) / workgroup_size;
//             let dispatch_y = (height + workgroup_size - 1) / workgroup_size;
//             compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
//         }

//         self.gpu.queue.submit(std::iter::once(encoder.finish()));
//         self.gpu.device.poll(wgpu::MaintainBase::Wait);
//     }

//     async fn find_peaks_gpu(
//         &self,
//         distance_map: &[f32],
//         width: u32,
//         height: u32,
//     ) -> Result<Vec<u32>> {
//         let pixel_count = (width * height) as usize;

//         // Create distance buffer
//         let distance_buffer =
//             self.gpu
//                 .device
//                 .create_buffer_init(&wgpu::util::BufferInitDescriptor {
//                     label: Some("Distance Buffer"),
//                     contents: bytemuck::cast_slice(distance_map),
//                     usage: wgpu::BufferUsages::STORAGE,
//                 });

//         // Create output buffer for markers
//         let output_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
//             label: Some("Markers Buffer"),
//             size: (pixel_count * 4) as u64, // 4 bytes per u32
//             usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
//             mapped_at_creation: false,
//         });

//         // Staging buffer to read results back to CPU
//         let staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
//             label: Some("Staging Buffer"),
//             size: (pixel_count * 4) as u64,
//             usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
//             mapped_at_creation: false,
//         });

//         // Uniform buffer
//         let uniforms = WatershedUniforms {
//             width,
//             height,
//             sigma: self.config.sigma,
//             min_distance: self.config.min_distance,
//         };

//         let uniform_buffer =
//             self.gpu
//                 .device
//                 .create_buffer_init(&wgpu::util::BufferInitDescriptor {
//                     label: Some("Watershed Uniform Buffer"),
//                     contents: bytemuck::cast_slice(&[uniforms]),
//                     usage: wgpu::BufferUsages::UNIFORM,
//                 });

//         // Create bind group
//         let bind_group_layout = self.watershed_pipeline.get_bind_group_layout(0);
//         let bind_group = self
//             .gpu
//             .device
//             .create_bind_group(&wgpu::BindGroupDescriptor {
//                 layout: &bind_group_layout,
//                 entries: &[
//                     wgpu::BindGroupEntry {
//                         binding: 0,
//                         resource: uniform_buffer.as_entire_binding(),
//                     },
//                     wgpu::BindGroupEntry {
//                         binding: 1,
//                         resource: output_buffer.as_entire_binding(),
//                     },
//                     wgpu::BindGroupEntry {
//                         binding: 2,
//                         resource: distance_buffer.as_entire_binding(),
//                     },
//                 ],
//                 label: Some("Watershed Bind Group"),
//             });

//         // Dispatch compute shader
//         let mut encoder = self
//             .gpu
//             .device
//             .create_command_encoder(&wgpu::CommandEncoderDescriptor {
//                 label: Some("Watershed Encoder"),
//             });

//         {
//             let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
//                 label: Some("Watershed Pass"),
//                 timestamp_writes: None,
//             });

//             compute_pass.set_pipeline(&self.watershed_pipeline);
//             compute_pass.set_bind_group(0, &bind_group, &[]);

//             let workgroup_size = 8;
//             let dispatch_x = (width + workgroup_size - 1) / workgroup_size;
//             let dispatch_y = (height + workgroup_size - 1) / workgroup_size;
//             compute_pass.dispatch_workgroups(dispatch_x, dispatch_y, 1);
//         }

//         encoder.copy_buffer_to_buffer(
//             &output_buffer,
//             0,
//             &staging_buffer,
//             0,
//             (pixel_count * 4) as u64,
//         );

//         self.gpu.queue.submit(std::iter::once(encoder.finish()));

//         // Read back results
//         let buffer_slice = staging_buffer.slice(..);
//         let (sender, receiver) = flume::unbounded();
//         buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
//             if let Err(e) = sender.send(result) {
//                 eprintln!("Failed to send GPU buffer mapping result: {}", e);
//             }
//         });

//         self.gpu.device.poll(wgpu::MaintainBase::Wait);
//         let map_result = receiver
//             .recv_async()
//             .await
//             .map_err(|e| anyhow::anyhow!("Failed to receive GPU buffer mapping result: {}", e))?;
//         map_result.map_err(|e| anyhow::anyhow!("Failed to map GPU buffer: {:?}", e))?;

//         let data = buffer_slice.get_mapped_range();
//         let markers: &[u32] = bytemuck::cast_slice(&data);

//         Ok(markers.to_vec())
//     }

//     fn watershed_transform(
//         &self,
//         distance: &[f32],
//         markers: &[u32],
//         binary: &[bool],
//         width: usize,
//         height: usize,
//     ) -> Vec<u32> {
//         let mut labels = markers.clone();
//         let mut queue = BinaryHeap::new();

//         // Priority queue element: (negative distance for max-heap behavior, x, y, label)
//         #[derive(Debug)]
//         struct QueueElement {
//             neg_distance: i32,
//             x: usize,
//             y: usize,
//             label: u32,
//         }

//         impl PartialEq for QueueElement {
//             fn eq(&self, other: &Self) -> bool {
//                 self.neg_distance == other.neg_distance
//             }
//         }

//         impl Eq for QueueElement {}

//         impl PartialOrd for QueueElement {
//             fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
//                 Some(self.cmp(other))
//             }
//         }

//         impl Ord for QueueElement {
//             fn cmp(&self, other: &Self) -> Ordering {
//                 self.neg_distance.cmp(&other.neg_distance)
//             }
//         }

//         // Initialize queue with marker pixels
//         for y in 0..height {
//             for x in 0..width {
//                 let idx = y * width + x;
//                 if markers[idx] > 0 {
//                     queue.push(QueueElement {
//                         neg_distance: -(distance[idx] * 1000.0) as i32,
//                         x,
//                         y,
//                         label: markers[idx],
//                     });
//                 }
//             }
//         }

//         let directions = [(0, 1), (1, 0), (0, -1), (-1, 0)];

//         // Process queue
//         while let Some(current) = queue.pop() {
//             let current_idx = current.y * width + current.x;

//             // Skip if already processed with a different label
//             if labels[current_idx] != current.label {
//                 continue;
//             }

//             // Check neighbors
//             for &(dx, dy) in &directions {
//                 let nx = current.x as i32 + dx;
//                 let ny = current.y as i32 + dy;

//                 if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
//                     let nx = nx as usize;
//                     let ny = ny as usize;
//                     let nidx = ny * width + nx;

//                     if binary[nidx] && labels[nidx] == 0 {
//                         labels[nidx] = current.label;
//                         queue.push(QueueElement {
//                             neg_distance: -(distance[nidx] * 1000.0) as i32,
//                             x: nx,
//                             y: ny,
//                             label: current.label,
//                         });
//                     }
//                 }
//             }
//         }

//         labels.to_vec()
//     }

//     fn extract_watershed_result(
//         &self,
//         binary: &[bool],
//         labels: &[u32],
//         width: usize,
//         height: usize,
//     ) -> Vec<bool> {
//         let mut result = vec![false; width * height];
//         let directions = [(0, 1), (1, 0), (0, -1), (-1, 0)];

//         for y in 0..height {
//             for x in 0..width {
//                 let idx = y * width + x;

//                 if !binary[idx] {
//                     continue;
//                 }

//                 let current_label = labels[idx];
//                 let mut is_edge = false;

//                 // Check if this pixel is on a watershed line (boundary between different labels)
//                 for &(dx, dy) in &directions {
//                     let nx = x as i32 + dx;
//                     let ny = y as i32 + dy;

//                     if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
//                         let nidx = ny as usize * width + nx as usize;
//                         if binary[nidx] && labels[nidx] != current_label && labels[nidx] > 0 {
//                             is_edge = true;
//                             break;
//                         }
//                     }
//                 }

//                 // Keep pixel if it's not on a watershed edge
//                 result[idx] = !is_edge;
//             }
//         }

//         // Apply morphological opening for cleanup
//         self.morphological_opening(&mut result, width, height);

//         result
//     }

//     fn morphological_opening(&self, image: &mut [bool], width: usize, height: usize) {
//         // Erosion followed by dilation
//         let temp = self.morphological_erosion(image, width, height);
//         let result = self.morphological_dilation(&temp, width, height);
//         image.copy_from_slice(&result);
//     }

//     fn morphological_erosion(&self, image: &[bool], width: usize, height: usize) -> Vec<bool> {
//         let mut result = vec![false; width * height];
//         let directions = [
//             (0, 1),
//             (1, 0),
//             (0, -1),
//             (-1, 0),
//             (1, 1),
//             (1, -1),
//             (-1, 1),
//             (-1, -1),
//         ];

//         for y in 1..height - 1 {
//             for x in 1..width - 1 {
//                 let idx = y * width + x;

//                 if image[idx] {
//                     let mut all_neighbors = true;

//                     for &(dx, dy) in &directions {
//                         let nx = x as i32 + dx;
//                         let ny = y as i32 + dy;
//                         let nidx = ny as usize * width + nx as usize;

//                         if !image[nidx] {
//                             all_neighbors = false;
//                             break;
//                         }
//                     }

//                     result[idx] = all_neighbors;
//                 }
//             }
//         }

//         result
//     }

//     fn morphological_dilation(&self, image: &[bool], width: usize, height: usize) -> Vec<bool> {
//         let mut result = vec![false; width * height];
//         let directions = [
//             (0, 1),
//             (1, 0),
//             (0, -1),
//             (-1, 0),
//             (1, 1),
//             (1, -1),
//             (-1, 1),
//             (-1, -1),
//         ];

//         for y in 1..height - 1 {
//             for x in 1..width - 1 {
//                 let idx = y * width + x;

//                 if image[idx] {
//                     result[idx] = true;

//                     // Dilate to neighbors
//                     for &(dx, dy) in &directions {
//                         let nx = x as i32 + dx;
//                         let ny = y as i32 + dy;

//                         if nx >= 0 && nx < width as i32 && ny >= 0 && ny < height as i32 {
//                             let nidx = ny as usize * width + nx as usize;
//                             result[nidx] = true;
//                         }
//                     }
//                 }
//             }
//         }

//         result
//     }

//     fn to_rgba_image(&self, binary: &[bool], width: u32, height: u32) -> Result<RgbaImage> {
//         let mut result = RgbaImage::new(width, height);

//         for y in 0..height {
//             for x in 0..width {
//                 let idx = (y * width + x) as usize;
//                 let value = if binary[idx] { 255 } else { 0 };
//                 result.put_pixel(x, y, image::Rgba([value, value, value, 255]));
//             }
//         }

//         println!("Applied watershed segmentation");
//         Ok(result)
//     }
// }
