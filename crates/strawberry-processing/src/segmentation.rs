use crate::{
    GpuImageProcessor,
    float_filters::GpuFloatFilters,
    tiling::{TileConfig, TileGrid, extract_rgba_tile},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use smartcore::{
    ensemble::random_forest_classifier::{
        RandomForestClassifier, RandomForestClassifierParameters,
    },
    linalg::basic::matrix::DenseMatrix,
};
use std::io::Write;
use std::{path::Path, sync::Arc};

const MODEL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureConfig {
    pub rgb: bool,
    pub gaussian: bool,
    pub sobel: bool,
    pub dog: bool,
}
impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            rgb: false,
            gaussian: true,
            sobel: true,
            dog: true,
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
        (usize::from(self.gaussian) + usize::from(self.sobel) + usize::from(self.dog))
            * if self.rgb { 3 } else { 1 }
    }
    fn halo(&self) -> usize {
        // Gaussian kernels use a radius of three times sigma; Sobel uses one pixel.
        if self.dog {
            6
        } else if self.gaussian {
            3
        } else {
            1
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForestConfig {
    pub trees: u16,
    pub depth: u16,
    pub seed: u64,
}
impl Default for ForestConfig {
    fn default() -> Self {
        Self {
            trees: 100,
            depth: 20,
            seed: 100,
        }
    }
}
type Forest = RandomForestClassifier<f32, u8, DenseMatrix<f32>, Vec<u8>>;
#[derive(Serialize, Deserialize)]
pub struct SegmentationModel {
    version: u32,
    pub features: FeatureConfig,
    pub parameters: ForestConfig,
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
        ensure!(
            parameters.trees > 0
                && parameters.trees <= 4096
                && parameters.depth > 0
                && parameters.depth <= 256,
            "Invalid forest parameters"
        );
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
                .with_seed(parameters.seed),
        )?;
        Ok(Self {
            version: MODEL_VERSION,
            features,
            parameters,
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
    pub fn tiles(
        &self,
        image: &image::RgbaImage,
        features: &FeatureConfig,
    ) -> Result<Vec<crate::tiling::TileInfo>> {
        features.validate()?;
        let mut config = self.config;
        config.overlap = features.halo();
        Ok(TileGrid::new(image.width(), image.height(), &config).tiles)
    }
    pub async fn extract(
        &self,
        image: &image::RgbaImage,
        features: &FeatureConfig,
        tile: &crate::tiling::TileInfo,
    ) -> Result<FeatureTile> {
        let input = extract_rgba_tile(image, tile)?;
        let (w, h) = input.dimensions();
        let mut columns = Vec::new();
        for channel in if features.rgb { vec![1, 2, 3] } else { vec![0] } {
            let plane = self.filters.channel(&input, channel).await?;
            let mut dog = None;
            if features.gaussian || features.dog {
                let small = self.filters.gaussian(w, h, &plane, 1.0).await?;
                if features.dog {
                    let large = self.filters.gaussian(w, h, &plane, 2.0).await?;
                    dog = Some(self.filters.difference(w, h, &small, &large).await?);
                }
                if features.gaussian {
                    columns.push(small);
                }
            }
            if features.sobel {
                columns.push(self.filters.sobel(w, h, &plane).await?);
            }
            if let Some(dog) = dog {
                columns.push(dog);
            }
        }
        let mut rows = Vec::new();
        for y in 0..tile.core_height {
            for x in 0..tile.core_width {
                let index = usize::try_from(
                    u64::from(y + tile.core_y - tile.y) * u64::from(w)
                        + u64::from(x + tile.core_x - tile.x),
                )?;
                rows.push(
                    columns
                        .iter()
                        .map(|c| {
                            c.get(index)
                                .copied()
                                .ok_or_else(|| anyhow::anyhow!("Feature tile index"))
                        })
                        .collect::<Result<Vec<_>>>()?,
                );
            }
        }
        Ok(FeatureTile {
            x: tile.core_x,
            y: tile.core_y,
            width: tile.core_width,
            height: tile.core_height,
            rows,
        })
    }
    pub async fn infer(
        &self,
        image: &image::RgbaImage,
        model: &SegmentationModel,
    ) -> Result<image::RgbaImage> {
        let mut output = image::RgbaImage::new(image.width(), image.height());
        for tile in self.tiles(image, &model.features)? {
            let features = self.extract(image, &model.features, &tile).await?;
            let labels = model.predict(&features.rows)?;
            for ((x, y, pixel), label) in image::RgbaImage::new(features.width, features.height)
                .enumerate_pixels()
                .zip(labels)
            {
                let _ = pixel;
                let value = if label == 1 { 255 } else { 0 };
                output.put_pixel(
                    features.x + x,
                    features.y + y,
                    image::Rgba([value, value, value, 255]),
                );
            }
        }
        Ok(output)
    }
}
