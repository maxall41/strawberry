use crate::{
    GpuImageProcessor,
    float_filters::{FilterChannel, GpuFloatFilters},
    pixel_count,
    tiling::{TileConfig, TileGrid, TileInfo, extract_rgba_tile},
};
use anyhow::{Context, Result, anyhow, ensure};
use futures::{
    channel::oneshot,
    stream::{FuturesUnordered, StreamExt},
};
use image::imageops::{self, FilterType};
use serde::{Deserialize, Serialize};
use smartcore::{
    ensemble::random_forest_classifier::RandomForestClassifier,
    linalg::basic::{arrays::Array, matrix::DenseMatrix},
};
use std::{
    borrow::Cow,
    collections::HashMap,
    mem,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

mod cache;
mod training;
use cache::FeatureCache;
use training::TrainingData;

const GAUSSIAN_SIGMA: f32 = 1.0;
const DOG_SMALL_SIGMA: f32 = 1.0;
const DOG_LARGE_SIGMA: f32 = 10.0;
const HESSIAN_SIGMA: f32 = 1.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Feature families are independently selectable"
)]
pub struct FeatureConfig {
    pub rgb: bool,
    pub gaussian: bool,
    pub sobel: bool,
    pub dog: bool,
    pub hessian: bool,
    pub membrane: bool,
    pub gaussian_sigma: f32,
    pub dog_small_sigma: f32,
    pub dog_large_sigma: f32,
}
impl Default for FeatureConfig {
    /// Enable the standard grayscale feature families with their default scales.
    fn default() -> Self {
        Self {
            rgb: false,
            gaussian: true,
            sobel: true,
            dog: true,
            hessian: true,
            membrane: false,
            gaussian_sigma: GAUSSIAN_SIGMA,
            dog_small_sigma: DOG_SMALL_SIGMA,
            dog_large_sigma: DOG_LARGE_SIGMA,
        }
    }
}
impl FeatureConfig {
    /// # Errors
    /// Returns an error if the configuration or model contains unsupported values.
    pub fn validate(&self) -> Result<()> {
        for (name, sigma) in [
            ("Gaussian", self.gaussian_sigma),
            ("DoG small", self.dog_small_sigma),
            ("DoG large", self.dog_large_sigma),
        ] {
            ensure!(
                sigma.is_finite() && (0.1..=64.0).contains(&sigma),
                "{name} sigma must be between 0.1 and 64"
            );
        }
        ensure!(
            self.dog_small_sigma < self.dog_large_sigma,
            "DoG requires small sigma < large sigma"
        );
        Ok(())
    }
    /// Count the feature columns produced by the enabled families and channels.
    #[must_use]
    pub fn count(&self) -> usize {
        [
            1,
            usize::from(self.gaussian),
            usize::from(self.sobel),
            usize::from(self.dog),
            usize::from(self.hessian).saturating_mul(2),
            usize::from(self.membrane).saturating_mul(6),
        ]
        .into_iter()
        .sum::<usize>()
        .saturating_mul(if self.rgb { 3 } else { 1 })
    }
    /// Calculate the surrounding pixels needed by all enabled feature filters.
    fn halo(&self) -> Result<usize> {
        let mut sigma = if self.gaussian {
            self.gaussian_sigma
        } else {
            0.0
        };
        if self.dog {
            sigma = sigma.max(self.dog_small_sigma).max(self.dog_large_sigma);
        }
        // Hessian differences need one pixel beyond the Gaussian smoothing radius.
        let hessian_radius = if self.hessian {
            (HESSIAN_SIGMA * 3.0).ceil() + 1.0
        } else {
            0.0
        };
        let radius = (sigma * 3.0)
            .ceil()
            .max(if self.sobel { 1.0 } else { 0.0 })
            .max(hessian_radius)
            .max(if self.membrane { 9.0 } else { 0.0 });
        num_traits::cast(radius).context("Invalid feature halo")
    }

    /// Use the same feature families with grayscale channels for probability context.
    fn context_features(&self) -> Self {
        Self {
            rgb: false,
            ..self.clone()
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForestConfig {
    pub trees: u16,
    pub depth: u16,
    pub seed: u64,
    pub input_scale: f32,
    pub balance_classes: bool,
    pub autocontext: bool,
}
impl Default for ForestConfig {
    /// Set the default forest size, depth, seed, and input-processing options.
    fn default() -> Self {
        Self {
            trees: 100,
            depth: 20,
            seed: 100,
            input_scale: 1.0,
            balance_classes: false,
            autocontext: false,
        }
    }
}
impl ForestConfig {
    /// # Errors
    /// Returns an error if the configuration or model contains unsupported values.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.trees > 0 && self.trees <= 4096 && self.depth > 0 && self.depth <= 256,
            "Invalid forest parameters"
        );
        ensure!(
            self.input_scale.is_finite() && (0.01..=1.0).contains(&self.input_scale),
            "Input scale must be between 0.01 and 1.0"
        );
        Ok(())
    }

    /// Use the same image scale for training features and prediction features.
    /// # Errors
    /// Returns an error if the configuration or image dimensions are invalid.
    pub fn rescale_input<'a>(
        &self,
        image: &'a image::RgbaImage,
    ) -> Result<Cow<'a, image::RgbaImage>> {
        self.validate()?;
        ensure!(
            image.width() > 0 && image.height() > 0,
            "Empty segmentation image"
        );
        if self.input_scale.to_bits() == 1.0_f32.to_bits() {
            return Ok(Cow::Borrowed(image));
        }
        let dimension = |size| -> Result<u32> {
            let scaled = (f64::from(size) * f64::from(self.input_scale))
                .round()
                .max(1.0);
            num_traits::cast(scaled).ok_or_else(|| anyhow!("Invalid scaled image size"))
        };
        Ok(Cow::Owned(imageops::resize(
            image,
            dimension(image.width())?,
            dimension(image.height())?,
            FilterType::Triangle,
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InferenceConfig {
    pub max_parallel_tiles: u16,
}

impl Default for InferenceConfig {
    /// Limit inference to eight parallel tiles by default.
    fn default() -> Self {
        Self {
            max_parallel_tiles: 8,
        }
    }
}

impl InferenceConfig {
    /// Choose how many tiles may predict at once, bounded by worker threads and the configured
    /// limit.
    fn get_parallel_tile_count(&self) -> usize {
        rayon::current_num_threads().clamp(1, usize::from(self.max_parallel_tiles))
    }

    /// # Errors
    /// Returns an error if the configuration or model contains unsupported values.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=256).contains(&self.max_parallel_tiles),
            "Maximum parallel tiles must be between 1 and 256"
        );
        Ok(())
    }
}

type Forest = RandomForestClassifier<f32, u8, DenseMatrix<f32>, Vec<u8>>;
#[derive(Serialize, Deserialize)]
pub struct SegmentationModel {
    pub class_count: usize,
    pub features: FeatureConfig,
    pub parameters: ForestConfig,
    #[serde(skip)]
    pub oob_error: Option<f64>,
    first_stage_forest: Arc<Forest>,
    second_stage_forest: Option<Arc<Forest>>,
}
impl SegmentationModel {
    /// Train a complete model from labeled pixels in images at the configured input scale.
    /// # Errors
    /// Returns an error if training data, feature extraction, or forest fitting fails.
    pub async fn train(
        features: FeatureConfig,
        parameters: ForestConfig,
        extractor: &FeatureExtractor,
        images: &[Cow<'_, image::RgbaImage>],
        selected: &HashMap<(usize, u32, u32), u8>,
        inference: &InferenceConfig,
    ) -> Result<Self> {
        features.validate()?;
        parameters.validate()?;
        inference.validate()?;
        let class_count = selected
            .values()
            .copied()
            .max()
            .map_or(0, |v| usize::from(v) + 1);
        ensure!(
            (2..=256).contains(&class_count)
                && (0..class_count).all(|i| selected.values().any(|&v| usize::from(v) == i)),
            "Every class needs training pixels"
        );
        let mut data = TrainingData::extract(
            extractor,
            &features,
            images,
            selected,
            parameters.autocontext,
        )
        .await?;
        let (first_stage_forest, mut oob_error) = data.fit(&parameters)?;
        let second_stage_forest = if parameters.autocontext {
            data.add_context(
                &first_stage_forest,
                extractor,
                &features,
                images,
                inference,
                class_count,
            )
            .await?;
            let (forest, error) = data.fit(&parameters)?;
            oob_error = error;
            Some(forest)
        } else {
            None
        };
        Ok(Self {
            class_count,
            features,
            parameters,
            oob_error: Some(oob_error),
            first_stage_forest,
            second_stage_forest,
        })
    }

    /// Count standard features and any per-class context features used for prediction.
    fn prediction_feature_count(&self) -> usize {
        self.features
            .count()
            .saturating_add(if self.parameters.autocontext {
                self.features
                    .context_features()
                    .count()
                    .saturating_mul(self.class_count)
            } else {
                0
            })
    }

    /// Predict one byte probability plane per class, in model label order, for each image.
    /// # Errors
    /// Returns an error if configuration, feature extraction, or inference fails.
    pub async fn predict(
        &self,
        extractor: &FeatureExtractor,
        images: &[Arc<image::RgbaImage>],
        config: &InferenceConfig,
    ) -> Result<Vec<Vec<image::GrayImage>>> {
        self.predict_with_progress(extractor, images, config, |_| {}, || false)
            .await
    }

    /// Report the cumulative number of input planes whose final prediction tiles are written,
    /// stopping before the next tile once `is_cancelled` returns true.
    /// # Errors
    /// Returns an error if configuration, feature extraction, or inference fails, or if the
    /// prediction is cancelled.
    pub async fn predict_with_progress(
        &self,
        extractor: &FeatureExtractor,
        images: &[Arc<image::RgbaImage>],
        config: &InferenceConfig,
        mut report_completed_planes: impl FnMut(usize) + Send,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<Vec<Vec<image::GrayImage>>> {
        config.validate()?;
        // Every image's tiles share one queue, so the next image's first tiles fill the cores
        // that the previous image's last tiles leave idle.
        let mut completed = 0usize;
        let mut predictor = TilePredictor::new(config, self.class_count, || {
            completed = completed.saturating_add(1);
            report_completed_planes(completed);
        });
        let passes = if self.second_stage_forest.is_some() {
            2
        } else {
            1
        };
        for original in images {
            let scaled = self.parameters.rescale_input(original)?;
            let image = scaled.as_ref();
            let tiles = extractor.tiles(image, &self.features)?;
            let tile_count = tiles
                .len()
                .checked_mul(passes)
                .context("Too many prediction tiles")?;
            let index = predictor.add_image(image.dimensions(), tile_count)?;
            let mut cache = if self.second_stage_forest.is_some() {
                Some(FeatureCache::new(self.features.count())?)
            } else {
                None
            };
            for tile in tiles {
                ensure!(!is_cancelled(), "Prediction was cancelled");
                let data = extractor.extract(image, &self.features, &tile).await?;
                if let Some(cache) = &mut cache {
                    cache.store(0, &tile, &data, Vec::new())?;
                }
                predictor
                    .submit(index, &self.first_stage_forest, data, self.features.count())
                    .await?;
            }
            if let (Some(forest), Some(mut cache)) = (&self.second_stage_forest, cache) {
                // Context features need this image's complete first-stage probabilities.
                predictor.finish_running().await?;
                let context = predictor.restart_image(index)?;
                for tile_index in cache.image_tiles(0) {
                    ensure!(!is_cancelled(), "Prediction was cancelled");
                    let entry = cache.tiles.get(tile_index).context("Missing cached tile")?;
                    let context_tile = extractor
                        .extract_context(&context, &self.features, &entry.tile)
                        .await?;
                    let mut data = cache.load(tile_index)?;
                    for (row, context_row) in data.rows.iter_mut().zip(context_tile.rows) {
                        row.extend(context_row);
                    }
                    predictor
                        .submit(index, forest, data, self.prediction_feature_count())
                        .await?;
                }
            }
        }
        predictor.finish_running().await?;
        Ok(predictor
            .into_outputs()
            .into_iter()
            .zip(images)
            .map(|(planes, original)| {
                planes
                    .into_iter()
                    .map(|plane| {
                        if plane.dimensions() == original.dimensions() {
                            plane
                        } else {
                            imageops::resize(
                                &plane,
                                original.width(),
                                original.height(),
                                FilterType::Triangle,
                            )
                        }
                    })
                    .collect()
            })
            .collect())
    }

    /// # Errors
    /// Returns an error if the configuration or model contains unsupported values.
    pub fn validate(&self) -> Result<()> {
        ensure!((2..=256).contains(&self.class_count), "Invalid class count");
        self.features.validate()?;
        self.parameters.validate()?;
        ensure!(
            self.parameters.autocontext == self.second_stage_forest.is_some(),
            "Auto-context setting does not match the saved forests"
        );
        Ok(())
    }
}

pub struct FeatureExtractor {
    filters: GpuFloatFilters,
    config: TileConfig,
}
pub struct FeatureTile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub rows: Vec<Vec<f32>>,
}
/// A predicted tile: its image, core origin and width, and each pixel's class probabilities.
struct TilePrediction {
    image: usize,
    x: u32,
    y: u32,
    width: u32,
    probabilities: DenseMatrix<f32>,
}

struct PredictionOutput {
    planes: Vec<image::GrayImage>,
    remaining_tiles: usize,
}

/// Predicts tiles from any number of images on the Rayon pool, with at most the configured
/// number predicting at once across all of them.
struct TilePredictor<F> {
    parallel_tiles: usize,
    class_count: usize,
    outputs: Vec<PredictionOutput>,
    running: FuturesUnordered<oneshot::Receiver<Result<TilePrediction>>>,
    on_plane_completed: F,
}

impl<F: FnMut() + Send> TilePredictor<F> {
    /// Create a predictor with no images, capped by the inference configuration.
    fn new(config: &InferenceConfig, class_count: usize, on_plane_completed: F) -> Self {
        Self {
            parallel_tiles: config.get_parallel_tile_count(),
            class_count,
            outputs: Vec::new(),
            running: FuturesUnordered::new(),
            on_plane_completed,
        }
    }

    /// Create one zeroed probability plane per class with the given dimensions.
    fn create_blank_planes(
        class_count: usize,
        (width, height): (u32, u32),
    ) -> Result<Vec<image::GrayImage>> {
        let count = pixel_count(width, height)?;
        count
            .checked_mul(class_count)
            .context("Probability image is too large")?;
        let mut planes = Vec::new();
        planes.try_reserve_exact(class_count)?;
        for _ in 0..class_count {
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(count)?;
            bytes.resize(count, 0);
            planes.push(
                image::GrayImage::from_raw(width, height, bytes)
                    .context("Invalid probability dimensions")?,
            );
        }
        Ok(planes)
    }

    /// Allocate an image's probability planes and track its expected tile count across passes.
    fn add_image(&mut self, dimensions: (u32, u32), tile_count: usize) -> Result<usize> {
        let index = self.outputs.len();
        self.outputs.push(PredictionOutput {
            planes: Self::create_blank_planes(self.class_count, dimensions)?,
            remaining_tiles: tile_count,
        });
        Ok(index)
    }

    /// Replace an image's probability planes with zeroed ones, returning the finished planes.
    fn restart_image(&mut self, image: usize) -> Result<Vec<image::GrayImage>> {
        let planes = &mut self
            .outputs
            .get_mut(image)
            .context("Missing tile image")?
            .planes;
        let dimensions = planes
            .first()
            .context("Missing probability planes")?
            .dimensions();
        let blank = Self::create_blank_planes(self.class_count, dimensions)?;
        Ok(mem::replace(planes, blank))
    }

    /// Start predicting a loaded tile of an image, first waiting for a free slot if every slot
    /// is busy.
    async fn submit(
        &mut self,
        image: usize,
        forest: &Arc<Forest>,
        tile: FeatureTile,
        feature_count: usize,
    ) -> Result<()> {
        if self.running.len() >= self.parallel_tiles
            && let Some(prediction) = self.running.next().await
        {
            self.write_tile(prediction)?;
        }
        let (sender, receiver) = oneshot::channel();
        let forest = Arc::clone(forest);
        let class_count = self.class_count;
        rayon::spawn(move || {
            if sender.is_canceled() {
                return;
            }
            // Rayon aborts the process when a spawned job panics, so report panics as errors.
            let result = catch_unwind(AssertUnwindSafe(|| {
                Self::predict_tile(&forest, image, &tile, feature_count, class_count)
            }))
            .unwrap_or_else(|_| Err(anyhow!("Tile prediction panicked")));
            let _ = sender.send(result);
        });
        self.running.push(receiver);
        Ok(())
    }

    /// Wait for every started tile and write its probabilities.
    async fn finish_running(&mut self) -> Result<()> {
        while let Some(prediction) = self.running.next().await {
            self.write_tile(prediction)?;
        }
        Ok(())
    }

    /// Return each image's probability planes in the order the images were added.
    fn into_outputs(self) -> Vec<Vec<image::GrayImage>> {
        self.outputs
            .into_iter()
            .map(|output| output.planes)
            .collect()
    }

    /// Predict the class probabilities of every pixel in a tile.
    fn predict_tile(
        forest: &Forest,
        image: usize,
        tile: &FeatureTile,
        feature_count: usize,
        class_count: usize,
    ) -> Result<TilePrediction> {
        ensure!(
            tile.rows.iter().all(|row| row.len() == feature_count),
            "Feature mismatch"
        );
        let probabilities = forest.predict_proba(&DenseMatrix::from_2d_vec(&tile.rows)?)?;
        // SmartCore sorts contiguous numeric labels into probability columns.
        ensure!(
            probabilities.shape() == (tile.rows.len(), class_count),
            "Invalid probability matrix"
        );
        Ok(TilePrediction {
            image,
            x: tile.x,
            y: tile.y,
            width: tile.width,
            probabilities,
        })
    }

    /// Write a tile's probabilities and report when all tiles for its input plane are complete.
    fn write_tile(
        &mut self,
        prediction: Result<Result<TilePrediction>, oneshot::Canceled>,
    ) -> Result<()> {
        let TilePrediction {
            image,
            x,
            y,
            width,
            probabilities,
        } = prediction.context("Tile prediction stopped unexpectedly")??;
        let output = self.outputs.get_mut(image).context("Missing tile image")?;
        for row in 0..probabilities.shape().0 {
            for (class, plane) in output.planes.iter_mut().enumerate() {
                let probability = *probabilities.get((row, class));
                ensure!(
                    probability.is_finite() && (0.0..=1.0).contains(&probability),
                    "Invalid class probability"
                );
                let value = num_traits::cast::<f32, u8>((probability * 255.0).round())
                    .context("Invalid probability pixel")?;
                let (x, y) = pixel_coordinates((x, y), width, row)?;
                let pixel = plane
                    .get_pixel_mut_checked(x, y)
                    .context("Probability pixel lies outside its image")?;
                *pixel = image::Luma([value]);
            }
        }
        output.remaining_tiles = output
            .remaining_tiles
            .checked_sub(1)
            .context("Unexpected prediction tile")?;
        if output.remaining_tiles == 0 {
            (self.on_plane_completed)();
        }
        Ok(())
    }
}

/// Convert a tile-row offset into checked image pixel coordinates.
fn pixel_coordinates((x, y): (u32, u32), width: u32, offset: usize) -> Result<(u32, u32)> {
    let offset = u32::try_from(offset)?;
    let column = offset.checked_rem(width).context("Empty feature tile")?;
    let row = offset.checked_div(width).context("Empty feature tile")?;
    Ok((
        x.checked_add(column)
            .context("Feature X coordinate overflow")?,
        y.checked_add(row)
            .context("Feature Y coordinate overflow")?,
    ))
}

impl FeatureTile {
    /// Convert feature planes into per-pixel rows for the tile core.
    fn from_columns(tile: &TileInfo, columns: &[Vec<f32>]) -> Result<Self> {
        let offset_x = usize::try_from(
            tile.core_x
                .checked_sub(tile.x)
                .context("Invalid tile core X")?,
        )?;
        let offset_y = usize::try_from(
            tile.core_y
                .checked_sub(tile.y)
                .context("Invalid tile core Y")?,
        )?;
        let width = usize::try_from(tile.width)?;
        let height = usize::try_from(tile.height)?;
        let core_width = usize::try_from(tile.core_width)?;
        let core_height = usize::try_from(tile.core_height)?;
        ensure!(
            core_width > 0
                && core_height > 0
                && offset_x
                    .checked_add(core_width)
                    .is_some_and(|end| end <= width)
                && offset_y
                    .checked_add(core_height)
                    .is_some_and(|end| end <= height),
            "Feature tile core lies outside tile"
        );
        let tile_pixel_count = pixel_count(tile.width, tile.height)?;
        ensure!(
            columns
                .iter()
                .all(|column| column.len() == tile_pixel_count),
            "Invalid feature plane dimensions"
        );

        let count = pixel_count(tile.core_width, tile.core_height)?;
        let mut rows: Vec<_> = (0..count)
            .map(|_| Vec::with_capacity(columns.len()))
            .collect();
        // Crop the halo from each plane and transpose into row-major pixel features.
        for column in columns {
            let values = column
                .chunks_exact(width)
                .skip(offset_y)
                .take(core_height)
                .flat_map(|row| row.iter().skip(offset_x).take(core_width));
            for (row, value) in rows.iter_mut().zip(values) {
                row.push(*value);
            }
        }
        Ok(Self {
            x: tile.core_x,
            y: tile.core_y,
            width: tile.core_width,
            height: tile.core_height,
            rows,
        })
    }
}

impl FeatureExtractor {
    /// # Errors
    /// Returns an error if GPU resources cannot be initialized.
    pub async fn new(gpu: Arc<GpuImageProcessor>) -> Result<Self> {
        let mut config = TileConfig::from_limits(&gpu.device.limits());
        config.max_tile_pixels = config.max_tile_pixels.min(512 * 512);
        Ok(Self {
            filters: GpuFloatFilters::new(gpu).await?,
            config,
        })
    }
    /// # Errors
    /// Returns an error if no features are selected or their halo is invalid.
    pub fn tiles(
        &self,
        image: &image::RgbaImage,
        features: &FeatureConfig,
    ) -> Result<Vec<TileInfo>> {
        features.validate()?;
        let mut config = self.config;
        config.overlap = features.halo()?;
        Ok(TileGrid::new(image.width(), image.height(), &config).tiles)
    }
    /// # Errors
    /// Returns an error for invalid features, tile dimensions, or GPU failures.
    pub async fn extract(
        &self,
        image: &image::RgbaImage,
        features: &FeatureConfig,
        tile: &TileInfo,
    ) -> Result<FeatureTile> {
        features.validate()?;
        let input = extract_rgba_tile(image, tile)?;
        self.extract_tile(&input, features, tile).await
    }

    /// Extract features from an already cropped tile, retaining its full-image coordinates.
    async fn extract_tile(
        &self,
        input: &image::RgbaImage,
        features: &FeatureConfig,
        tile: &TileInfo,
    ) -> Result<FeatureTile> {
        let (width, height) = input.dimensions();
        let channels: &[FilterChannel] = if features.rgb {
            &[
                FilterChannel::Red,
                FilterChannel::Green,
                FilterChannel::Blue,
            ]
        } else {
            &[FilterChannel::Luminance]
        };
        let mut columns = Vec::with_capacity(features.count());
        for &channel in channels {
            let channel_start = columns.len();
            let plane = self.filters.channel(input, channel).await?;
            let gaussian = if features.gaussian {
                Some(
                    self.filters
                        .gaussian(width, height, &plane, features.gaussian_sigma)
                        .await?,
                )
            } else {
                None
            };
            let hessian = if features.hessian {
                let smoothed = match gaussian.as_deref() {
                    Some(smoothed) if features.gaussian_sigma == HESSIAN_SIGMA => {
                        Cow::Borrowed(smoothed)
                    }
                    _ => Cow::Owned(
                        self.filters
                            .gaussian(width, height, &plane, HESSIAN_SIGMA)
                            .await?,
                    ),
                };
                Some(
                    self.filters
                        .hessian_eigenvalues(width, height, &smoothed)
                        .await?,
                )
            } else {
                None
            };
            if let Some(smoothed) = gaussian {
                columns.push(smoothed);
            }
            if let Some(eigenvalues) = hessian {
                columns.extend(eigenvalues);
            }
            if features.sobel {
                columns.push(self.filters.sobel(width, height, &plane).await?);
            }
            if features.dog {
                columns.push(
                    self.filters
                        .difference_of_gaussians(
                            width,
                            height,
                            &plane,
                            features.dog_small_sigma,
                            features.dog_large_sigma,
                        )
                        .await?,
                );
            }
            if features.membrane {
                columns.extend(
                    self.filters
                        .membrane_projections(width, height, &plane)
                        .await?,
                );
            }
            // Raw intensity is always first; move it after the filters finish borrowing it.
            columns.insert(channel_start, plane);
        }
        FeatureTile::from_columns(tile, &columns)
    }

    /// Extract and concatenate features from each class probability plane for a tile.
    async fn extract_context(
        &self,
        context: &[image::GrayImage],
        features: &FeatureConfig,
        tile: &TileInfo,
    ) -> Result<FeatureTile> {
        let features = features.context_features();
        features.validate()?;
        let mut result: Option<FeatureTile> = None;
        for plane in context {
            // Convert only this tile and its halo, matching extract_rgba_tile's black padding.
            let rgba = image::RgbaImage::from_fn(tile.width, tile.height, |x, y| {
                let value = tile
                    .x
                    .checked_add(x)
                    .zip(tile.y.checked_add(y))
                    .and_then(|(x, y)| plane.get_pixel_checked(x, y))
                    .map_or(0, |pixel| pixel[0]);
                image::Rgba([value, value, value, 255])
            });
            let data = self.extract_tile(&rgba, &features, tile).await?;
            if let Some(result) = &mut result {
                for (row, values) in result.rows.iter_mut().zip(data.rows) {
                    row.extend(values);
                }
            } else {
                result = Some(data);
            }
        }
        result.context("Missing context planes")
    }
}
