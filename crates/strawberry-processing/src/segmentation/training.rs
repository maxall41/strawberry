use super::cache::FeatureCache;
use super::{
    FeatureConfig, FeatureExtractor, Forest, ForestConfig, InferenceConfig, SegmentationModel,
    pixel_coordinates,
};
use crate::tiling::TileInfo;
use anyhow::{Context, Result, ensure};
use smartcore::{
    ensemble::random_forest_classifier::RandomForestClassifierParameters,
    linalg::basic::matrix::DenseMatrix,
};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

pub(super) struct TrainingData {
    rows: Vec<Vec<f32>>,
    labels: Vec<u8>,
    cache: Option<FeatureCache>,
    feature_count: usize,
}

/// Validate selected training coordinates and group them by source image.
fn index_selected(
    images: &[Cow<'_, image::RgbaImage>],
    selected: &HashMap<(usize, u32, u32), u8>,
) -> Result<HashMap<usize, Vec<(u32, u32)>>> {
    let mut by_image: HashMap<usize, Vec<(u32, u32)>> = HashMap::new();
    for &(image_index, x, y) in selected.keys() {
        let image = images
            .get(image_index)
            .context("Training pixels lie outside their images")?;
        ensure!(
            x < image.width() && y < image.height(),
            "Training pixels lie outside their images"
        );
        by_image.entry(image_index).or_default().push((x, y));
    }
    Ok(by_image)
}

/// Keep tiles containing labeled pixels, or all tiles when context caching is needed.
fn training_tiles(
    mut tiles: Vec<TileInfo>,
    pixels: &[(u32, u32)],
    cache_features: bool,
) -> Result<Vec<TileInfo>> {
    // Auto-context needs complete feature planes, including unlabeled neighbors.
    if cache_features {
        return Ok(tiles);
    }
    let first = tiles.first().context("Missing training image tiles")?;
    let (width, height) = (first.core_width, first.core_height);
    ensure!(width > 0 && height > 0, "Invalid training tile dimensions");
    // TileGrid uses a regular core stride; only the final row/column can be smaller.
    let occupied: HashSet<_> = pixels
        .iter()
        .map(|&(x, y)| {
            (
                x.checked_div(width).unwrap_or_default(),
                y.checked_div(height).unwrap_or_default(),
            )
        })
        .collect();
    tiles.retain(|tile| {
        occupied.contains(&(
            tile.core_x.checked_div(width).unwrap_or_default(),
            tile.core_y.checked_div(height).unwrap_or_default(),
        ))
    });
    Ok(tiles)
}

impl TrainingData {
    /// Extract labeled feature rows and optionally cache complete tiles for context training.
    pub async fn extract(
        extractor: &FeatureExtractor,
        features: &FeatureConfig,
        images: &[Cow<'_, image::RgbaImage>],
        selected: &HashMap<(usize, u32, u32), u8>,
        cache_features: bool,
    ) -> Result<Self> {
        let selected_by_image = index_selected(images, selected)?;
        let mut rows = Vec::new();
        let mut labels = Vec::new();
        let mut cache = if cache_features {
            Some(FeatureCache::new(features.count())?)
        } else {
            None
        };
        for (image_index, image) in images.iter().enumerate() {
            let Some(pixels) = selected_by_image.get(&image_index) else {
                continue;
            };
            let tiles = training_tiles(extractor.tiles(image, features)?, pixels, cache_features)?;
            for tile in tiles {
                let data = extractor.extract(image, features, &tile).await?;
                let mut samples = Vec::new();
                for (offset, row) in data.rows.iter().enumerate() {
                    let (x, y) = pixel_coordinates((data.x, data.y), data.width, offset)?;
                    let key = (image_index, x, y);
                    if let Some(&label) = selected.get(&key) {
                        if cache.is_some() {
                            samples.push((offset, rows.len()));
                        }
                        rows.push(row.clone());
                        labels.push(label);
                    }
                }
                if let Some(cache) = &mut cache {
                    cache.store(image_index, &tile, &data, samples)?;
                }
            }
        }
        ensure!(
            rows.len() == selected.len(),
            "Training pixels lie outside their images"
        );
        Ok(Self {
            rows,
            labels,
            cache,
            feature_count: features.count(),
        })
    }

    /// Append features from first-stage class probabilities to the labeled training rows.
    pub async fn add_context(
        &mut self,
        first_stage_forest: &Forest,
        extractor: &FeatureExtractor,
        features: &FeatureConfig,
        images: &[Cow<'_, image::RgbaImage>],
        inference: &InferenceConfig,
        class_count: usize,
    ) -> Result<()> {
        let cache = self
            .cache
            .as_mut()
            .context("Missing standard feature cache")?;
        for (image_index, image) in images.iter().enumerate() {
            let indices = cache.image_tiles(image_index);
            if indices.is_empty() {
                continue;
            }
            // Include unlabeled neighbors when constructing the probability context map.
            let context = SegmentationModel::predict_batches(
                first_stage_forest,
                class_count,
                image.dimensions(),
                self.feature_count,
                inference,
                &indices,
                async |batch: &[usize]| cache.load_batch(batch),
            )
            .await?;
            for index in indices {
                let entry = cache.tiles.get(index).context("Missing cached tile")?;
                if entry.samples.is_empty() {
                    continue;
                }
                let context_data = extractor
                    .extract_context(&context, features, &entry.tile)
                    .await?;
                for &(offset, row_index) in &entry.samples {
                    let context_row = context_data
                        .rows
                        .get(offset)
                        .context("Missing context pixel")?;
                    self.rows
                        .get_mut(row_index)
                        .context("Missing training row")?
                        .extend_from_slice(context_row);
                }
            }
        }
        self.feature_count = self
            .feature_count
            .checked_add(
                features
                    .context_features()
                    .count()
                    .checked_mul(class_count)
                    .context("Too many context features")?,
            )
            .context("Too many training features")?;
        Ok(())
    }

    /// Train a random forest and return its out-of-bag classification error.
    pub fn fit(&self, parameters: &ForestConfig) -> Result<(Forest, f64)> {
        let rows = &self.rows;
        let labels = &self.labels;
        ensure!(
            rows.len() == labels.len() && labels.iter().copied().collect::<HashSet<_>>().len() >= 2,
            "Training requires at least two classes"
        );
        ensure!(
            rows.iter()
                .all(|r| r.len() == self.feature_count && r.iter().all(|v| v.is_finite())),
            "Invalid training feature matrix"
        );
        let matrix = DenseMatrix::from_2d_vec(rows)?;
        let forest = Forest::fit(
            &matrix,
            labels,
            RandomForestClassifierParameters::default()
                .with_n_trees(parameters.trees)
                .with_max_depth(parameters.depth)
                .with_keep_samples(true)
                .with_seed(parameters.seed),
        )?;
        let oob_predictions = forest.predict_oob(&matrix)?;
        let oob_error = 1.0 - smartcore::metrics::accuracy(labels, &oob_predictions);
        println!(
            "Segmentation training OOB error: {:.2}% ({} training samples)",
            oob_error * 100.0,
            labels.len(),
        );
        Ok((forest, oob_error))
    }
}
