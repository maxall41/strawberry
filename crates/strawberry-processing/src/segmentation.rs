//! GPU feature extraction and CPU random-forest pixel classification.
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
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureConfig {
    pub rgb: bool,
    pub gaussian: Vec<f32>,
    pub sobel: bool,
    pub dog: Vec<(f32, f32)>,
}
impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            rgb: false,
            gaussian: vec![1.0],
            sobel: true,
            dog: vec![(1.0, 2.0)],
        }
    }
}
impl FeatureConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.count() > 0 && self.count() <= 96,
            "Choose between 1 and 96 features"
        );
        for sigma in self
            .gaussian
            .iter()
            .copied()
            .chain(self.dog.iter().flat_map(|&(a, b)| [a, b]))
        {
            ensure!(
                sigma.is_finite() && sigma > 0.0 && sigma <= 64.0,
                "Sigma must be in (0,64]"
            );
        }
        for &(a, b) in &self.dog {
            ensure!(a < b, "DoG requires small sigma < large sigma");
        }
        Ok(())
    }
    #[must_use]
    pub fn count(&self) -> usize {
        (self.gaussian.len() + usize::from(self.sobel) + self.dog.len())
            * if self.rgb { 3 } else { 1 }
    }
    fn halo(&self) -> usize {
        let sigma = self
            .gaussian
            .iter()
            .copied()
            .chain(self.dog.iter().flat_map(|&(a, b)| [a, b]))
            .fold(0.0, f32::max);
        num_traits::ToPrimitive::to_usize(&(sigma * 3.0).ceil())
            .unwrap_or(192)
            .max(1)
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
            seed: 42,
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
            version: 1,
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
        crate::segmentation::write_artifact(path, &serde_json::to_vec(self)?)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let model: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(model.version == 1, "Unsupported segmentation model version");
        model.features.validate()?;
        model.predict(&vec![vec![0.0; model.features.count()]])?;
        Ok(model)
    }
}
/// Write beside the destination then rename, preserving an existing artifact on failure.
pub fn write_artifact(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = std::path::PathBuf::from(temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
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
            let mut gaussian = BTreeMap::new();
            for sigma in features
                .gaussian
                .iter()
                .copied()
                .chain(features.dog.iter().flat_map(|&(a, b)| [a, b]))
            {
                if !gaussian.contains_key(&sigma.to_bits()) {
                    gaussian.insert(
                        sigma.to_bits(),
                        self.filters.gaussian(w, h, &plane, sigma).await?,
                    );
                }
            }
            for sigma in &features.gaussian {
                columns.push(
                    gaussian
                        .get(&sigma.to_bits())
                        .ok_or_else(|| anyhow::anyhow!("Missing Gaussian"))?
                        .clone(),
                );
            }
            if features.sobel {
                columns.push(self.filters.sobel(w, h, &plane).await?);
            }
            for &(a, b) in &features.dog {
                columns.push(
                    self.filters
                        .difference(
                            w,
                            h,
                            gaussian
                                .get(&a.to_bits())
                                .ok_or_else(|| anyhow::anyhow!("Missing Gaussian"))?,
                            gaussian
                                .get(&b.to_bits())
                                .ok_or_else(|| anyhow::anyhow!("Missing Gaussian"))?,
                        )
                        .await?,
                );
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
