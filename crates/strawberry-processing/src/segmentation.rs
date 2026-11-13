use crate::{
    GpuImageProcessor,
    float_filters::GpuFloatFilters,
    pixel_count,
    tiling::{TileConfig, TileGrid, TileInfo, extract_rgba_tile},
};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use smartcore::{
    ensemble::random_forest_classifier::RandomForestClassifier,
    linalg::basic::{arrays::Array, matrix::DenseMatrix},
};
use std::{borrow::Cow, collections::HashMap, io::Write, path::Path, sync::Arc};

mod cache;
mod training;
use cache::FeatureCache;
use training::TrainingData;

const GAUSSIAN_SIGMA: f32 = 1.0;
const DOG_SMALL_SIGMA: f32 = 1.0;
const DOG_LARGE_SIGMA: f32 = 10.0;
const HESSIAN_SIGMA: f32 = 1.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[must_use]
    pub fn count(&self) -> usize {
        (1 + usize::from(self.gaussian)
            + usize::from(self.sobel)
            + usize::from(self.dog)
            + 2 * usize::from(self.hessian)
            + 6 * usize::from(self.membrane))
            * if self.rgb { 3 } else { 1 }
    }
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
    pub fn rescale_input<'a>(
        &self,
        image: &'a image::RgbaImage,
    ) -> Result<Cow<'a, image::RgbaImage>> {
        self.validate()?;
        ensure!(
            image.width() > 0 && image.height() > 0,
            "Empty segmentation image"
        );
        if self.input_scale == 1.0 {
            return Ok(Cow::Borrowed(image));
        }
        let dimension = |size| -> Result<u32> {
            let scaled = (f64::from(size) * f64::from(self.input_scale))
                .round()
                .max(1.0);
            num_traits::cast(scaled).ok_or_else(|| anyhow::anyhow!("Invalid scaled image size"))
        };
        Ok(Cow::Owned(image::imageops::resize(
            image,
            dimension(image.width())?,
            dimension(image.height())?,
            image::imageops::FilterType::Triangle,
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InferenceConfig {
    pub max_parallel_tiles: u16,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            max_parallel_tiles: 8,
        }
    }
}

impl InferenceConfig {
    fn batch_size(&self) -> usize {
        rayon::current_num_threads().clamp(1, usize::from(self.max_parallel_tiles))
    }

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
    first_stage_forest: Forest,
    second_stage_forest: Option<Forest>,
}
impl SegmentationModel {
    /// Train a complete model from labeled pixels in images at the configured input scale.
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

    fn prediction_feature_count(&self) -> usize {
        self.features.count()
            + if self.parameters.autocontext {
                self.features.context_features().count() * self.class_count
            } else {
                0
            }
    }

    /// Predict one byte probability plane per class, in model label order.
    pub async fn predict(
        &self,
        extractor: &FeatureExtractor,
        image: &image::RgbaImage,
        config: &InferenceConfig,
    ) -> Result<Vec<image::GrayImage>> {
        config.validate()?;
        let original_dimensions = image.dimensions();
        let scaled = self.parameters.rescale_input(image)?;
        let image = scaled.as_ref();
        let tiles = extractor.tiles(image, &self.features)?;
        let mut cache = if self.second_stage_forest.is_some() {
            Some(FeatureCache::new(self.features.count())?)
        } else {
            None
        };
        let mut output = Self::predict_batches(
            &self.first_stage_forest,
            self.class_count,
            image.dimensions(),
            self.features.count(),
            config,
            &tiles,
            async |batch: &[TileInfo]| {
                // Finish GPU work before predict_batches enters Rayon.
                let data = futures::future::try_join_all(
                    batch
                        .iter()
                        .map(|tile| extractor.extract(image, &self.features, tile)),
                )
                .await?;
                if let Some(cache) = &mut cache {
                    for (tile, features) in batch.iter().zip(&data) {
                        cache.store(0, tile, features, Vec::new())?;
                    }
                }
                Ok(data)
            },
        )
        .await?;
        if let (Some(forest), Some(mut cache)) = (&self.second_stage_forest, cache) {
            let context = output;
            let indices = cache.image_tiles(0);
            output = Self::predict_batches(
                forest,
                self.class_count,
                image.dimensions(),
                self.prediction_feature_count(),
                config,
                &indices,
                async |batch: &[usize]| {
                    let context_tasks = batch
                        .iter()
                        .map(|&index| {
                            let entry = cache.tiles.get(index).context("Missing cached tile")?;
                            Ok(extractor.extract_context(&context, &self.features, &entry.tile))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let context_data = futures::future::try_join_all(context_tasks).await?;
                    let mut data = cache.load_batch(batch)?;
                    for (tile, context_tile) in data.iter_mut().zip(context_data) {
                        for (row, context_row) in tile.rows.iter_mut().zip(context_tile.rows) {
                            row.extend(context_row);
                        }
                    }
                    Ok(data)
                },
            )
            .await?;
        }
        Ok(output
            .into_iter()
            .map(|plane| {
                if plane.dimensions() == original_dimensions {
                    plane
                } else {
                    image::imageops::resize(
                        &plane,
                        original_dimensions.0,
                        original_dimensions.1,
                        image::imageops::FilterType::Triangle,
                    )
                }
            })
            .collect())
    }

    async fn predict_batches<T>(
        forest: &Forest,
        class_count: usize,
        dimensions: (u32, u32),
        feature_count: usize,
        config: &InferenceConfig,
        tiles: &[T],
        mut load_batch: impl AsyncFnMut(&[T]) -> Result<Vec<FeatureTile>>,
    ) -> Result<Vec<image::GrayImage>> {
        let count = pixel_count(dimensions.0, dimensions.1)?;
        count
            .checked_mul(class_count)
            .context("Probability image is too large")?;
        let mut output = Vec::new();
        output.try_reserve_exact(class_count)?;
        for _ in 0..class_count {
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(count)?;
            bytes.resize(count, 0);
            output.push(
                image::GrayImage::from_raw(dimensions.0, dimensions.1, bytes)
                    .context("Invalid probability dimensions")?,
            );
        }
        for batch in tiles.chunks(config.batch_size()) {
            let data = load_batch(batch).await?;
            Self::predict_tiles(forest, data, feature_count, &mut output)?;
        }
        Ok(output)
    }

    fn predict_tiles(
        forest: &Forest,
        tiles: Vec<FeatureTile>,
        feature_count: usize,
        output: &mut [image::GrayImage],
    ) -> Result<()> {
        let predictions = tiles
            .into_par_iter()
            .map(|tile| {
                ensure!(
                    tile.rows.iter().all(|row| row.len() == feature_count),
                    "Feature mismatch"
                );
                let probabilities = forest.predict_proba(&DenseMatrix::from_2d_vec(&tile.rows)?)?;
                // SmartCore sorts contiguous numeric labels into probability columns.
                ensure!(
                    probabilities.shape() == (tile.rows.len(), output.len()),
                    "Invalid probability matrix"
                );
                Ok((tile.x, tile.y, tile.width, probabilities))
            })
            .collect::<Result<Vec<_>>>()?;
        for (x, y, width, probabilities) in predictions {
            for row in 0..probabilities.shape().0 {
                for (class, plane) in output.iter_mut().enumerate() {
                    let probability = *probabilities.get((row, class));
                    ensure!(
                        probability.is_finite() && (0.0..=1.0).contains(&probability),
                        "Invalid class probability"
                    );
                    let value = num_traits::cast::<f32, u8>((probability * 255.0).round())
                        .context("Invalid probability pixel")?;
                    let offset = u32::try_from(row)?;
                    plane.put_pixel(x + offset % width, y + offset / width, image::Luma([value]));
                }
            }
        }
        Ok(())
    }

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

pub fn write_artifact(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
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

impl FeatureTile {
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
    #[must_use]
    pub fn new(gpu: Arc<GpuImageProcessor>) -> Self {
        let mut config = TileConfig::from_limits(&gpu.device.limits());
        config.max_tile_pixels = config.max_tile_pixels.min(512 * 512);
        Self {
            filters: GpuFloatFilters::new(gpu),
            config,
        }
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
        let channels: &[u32] = if features.rgb { &[1, 2, 3] } else { &[0] };
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
