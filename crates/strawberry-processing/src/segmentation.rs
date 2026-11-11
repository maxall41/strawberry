use crate::{
    GpuImageProcessor,
    float_filters::GpuFloatFilters,
    tiling::{TileConfig, TileGrid, TileInfo, extract_rgba_tile},
};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use smartcore::{
    ensemble::random_forest_classifier::{
        RandomForestClassifier, RandomForestClassifierParameters,
    },
    linalg::basic::matrix::DenseMatrix,
};
use std::{borrow::Cow, io::Write, path::Path, sync::Arc};

const MODEL_VERSION: u32 = 1;
const GAUSSIAN_SIGMA: f32 = 1.0;
const DOG_SMALL_SIGMA: f32 = 1.0;
const DOG_LARGE_SIGMA: f32 = 10.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureConfig {
    pub rgb: bool,
    pub gaussian: bool,
    pub sobel: bool,
    pub dog: bool,
    pub hessian: bool,
}
impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            rgb: false,
            gaussian: true,
            sobel: true,
            dog: true,
            hessian: true,
        }
    }
}
impl FeatureConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.count() > 0, "Choose at least one feature");
        Ok(())
    }
    #[must_use]
    pub fn count(&self) -> usize {
        (usize::from(self.gaussian)
            + usize::from(self.sobel)
            + usize::from(self.dog)
            + 2 * usize::from(self.hessian))
            * if self.rgb { 3 } else { 1 }
    }
    fn halo(&self) -> Result<usize> {
        let mut sigma = if self.gaussian { GAUSSIAN_SIGMA } else { 0.0 };
        if self.dog {
            sigma = sigma.max(DOG_SMALL_SIGMA).max(DOG_LARGE_SIGMA);
        }
        // Hessian differences need one pixel beyond the Gaussian smoothing radius.
        let hessian_radius = if self.hessian {
            (GAUSSIAN_SIGMA * 3.0).ceil() + 1.0
        } else {
            0.0
        };
        let radius = (sigma * 3.0)
            .ceil()
            .max(if self.sobel { 1.0 } else { 0.0 })
            .max(hessian_radius);
        num_traits::cast(radius).context("Invalid feature halo")
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
}
impl Default for ForestConfig {
    fn default() -> Self {
        Self {
            trees: 100,
            depth: 20,
            seed: 100,
            input_scale: 1.0,
            balance_classes: false,
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
    version: u32,
    pub features: FeatureConfig,
    pub parameters: ForestConfig,
    #[serde(skip)]
    pub oob_error: Option<f64>,
    forest: Forest,
}
impl SegmentationModel {
    pub fn train(
        features: FeatureConfig,
        parameters: ForestConfig,
        rows: Vec<Vec<f32>>,
        labels: Vec<u8>,
    ) -> Result<Self> {
        features.validate()?;
        parameters.validate()?;
        ensure!(
            rows.len() == labels.len()
                && labels.contains(&0)
                && labels.contains(&1)
                && labels.iter().all(|&v| v <= 1),
            "Training requires foreground and background samples"
        );
        ensure!(
            rows.iter()
                .all(|r| r.len() == features.count() && r.iter().all(|v| v.is_finite())),
            "Invalid training feature matrix"
        );
        let matrix = DenseMatrix::from_2d_vec(&rows)?;
        let forest = Forest::fit(
            &matrix,
            &labels,
            RandomForestClassifierParameters::default()
                .with_n_trees(parameters.trees)
                .with_max_depth(parameters.depth)
                .with_keep_samples(true)
                .with_seed(parameters.seed),
        )?;
        let oob_predictions = forest.predict_oob(&matrix)?;
        let oob_error = 1.0 - smartcore::metrics::accuracy(&labels, &oob_predictions);
        println!(
            "Segmentation training OOB error: {:.2}% ({} training samples)",
            oob_error * 100.0,
            labels.len(),
        );
        Ok(Self {
            version: MODEL_VERSION,
            features,
            parameters,
            oob_error: Some(oob_error),
            forest,
        })
    }
    pub fn predict(&self, rows: &Vec<Vec<f32>>) -> Result<Vec<u8>> {
        ensure!(
            rows.iter().all(|r| r.len() == self.features.count()),
            "Feature mismatch"
        );
        Ok(self.forest.predict(&DenseMatrix::from_2d_vec(rows)?)?)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        crate::segmentation::write_artifact(path, &self.to_bytes()?)
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::from_bytes(&std::fs::read(path)?)
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let model: Self = serde_json::from_slice(bytes)?;
        ensure!(
            model.version == MODEL_VERSION,
            "Unsupported segmentation model version"
        );
        model.features.validate()?;
        model.parameters.validate()?;
        model.predict(&vec![vec![0.0; model.features.count()]])?;
        Ok(model)
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
        let pixel_count = crate::pixel_count(tile.width, tile.height)?;
        ensure!(
            columns.iter().all(|column| column.len() == pixel_count),
            "Invalid feature plane dimensions"
        );

        let count = crate::pixel_count(tile.core_width, tile.core_height)?;
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
        let (width, height) = input.dimensions();
        let channels: &[u32] = if features.rgb { &[1, 2, 3] } else { &[0] };
        let mut columns = Vec::with_capacity(features.count());
        for &channel in channels {
            let plane = self.filters.channel(&input, channel).await?;
            if features.gaussian || features.hessian {
                let smoothed = self
                    .filters
                    .gaussian(width, height, &plane, GAUSSIAN_SIGMA)
                    .await?;
                let hessian = if features.hessian {
                    Some(
                        self.filters
                            .hessian_eigenvalues(width, height, &smoothed)
                            .await?,
                    )
                } else {
                    None
                };
                if features.gaussian {
                    columns.push(smoothed);
                }
                if let Some(eigenvalues) = hessian {
                    columns.extend(eigenvalues);
                }
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
                            DOG_SMALL_SIGMA,
                            DOG_LARGE_SIGMA,
                        )
                        .await?,
                );
            }
        }
        FeatureTile::from_columns(tile, &columns)
    }
    pub async fn infer(
        &self,
        image: &image::RgbaImage,
        model: &SegmentationModel,
        config: &InferenceConfig,
    ) -> Result<image::RgbaImage> {
        config.validate()?;
        let original_dimensions = image.dimensions();
        let scaled = model.parameters.rescale_input(image)?;
        let image = scaled.as_ref();
        let mut output = image::RgbaImage::new(image.width(), image.height());
        let tiles = self.tiles(image, &model.features)?;
        // Bound concurrent feature matrices for both luminance and RGB extraction.
        let batch_size =
            rayon::current_num_threads().clamp(1, usize::from(config.max_parallel_tiles));
        for batch in tiles.chunks(batch_size) {
            // Await GPU extraction before entering Rayon, which may run on this thread.
            let features = futures::future::try_join_all(
                batch
                    .iter()
                    .map(|tile| self.extract(image, &model.features, tile)),
            )
            .await?;
            let predictions = features
                .into_par_iter()
                .map(|features| {
                    let labels = model.predict(&features.rows)?;
                    Ok((features.x, features.y, features.width, labels))
                })
                .collect::<Result<Vec<_>>>()?;
            for (x, y, width, labels) in predictions {
                for (offset, label) in labels.into_iter().enumerate() {
                    let offset = u32::try_from(offset)?;
                    let value = if label == 1 { 255 } else { 0 };
                    output.put_pixel(
                        x + offset % width,
                        y + offset / width,
                        image::Rgba([value, value, value, 255]),
                    );
                }
            }
        }
        if output.dimensions() == original_dimensions {
            Ok(output)
        } else {
            // Labels are categorical: interpolation would introduce nonbinary values.
            Ok(image::imageops::resize(
                &output,
                original_dimensions.0,
                original_dimensions.1,
                image::imageops::FilterType::Nearest,
            ))
        }
    }
}
