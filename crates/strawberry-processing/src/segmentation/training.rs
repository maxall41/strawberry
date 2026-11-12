use super::cache::FeatureCache;
use super::{
    FeatureConfig, FeatureExtractor, Forest, ForestConfig, InferenceConfig, SegmentationModel,
};
use anyhow::{Context, Result, ensure};
use smartcore::{
    ensemble::random_forest_classifier::RandomForestClassifierParameters,
    linalg::basic::matrix::DenseMatrix,
};
use std::{borrow::Cow, collections::HashMap};

pub(super) struct TrainingData {
    rows: Vec<Vec<f32>>,
    labels: Vec<u8>,
    cache: Option<FeatureCache>,
    feature_count: usize,
}

impl TrainingData {
    pub async fn extract(
        extractor: &FeatureExtractor,
        features: &FeatureConfig,
        images: &[Cow<'_, image::RgbaImage>],
        selected: &HashMap<(usize, u32, u32), u8>,
        cache_features: bool,
    ) -> Result<Self> {
        let mut rows = Vec::new();
        let mut labels = Vec::new();
        let mut cache = if cache_features {
            Some(FeatureCache::new(features.count())?)
        } else {
            None
        };
        for (image_index, image) in images.iter().enumerate() {
            if !selected.keys().any(|&(i, _, _)| i == image_index) {
                continue;
            }
            for tile in extractor.tiles(image, features)? {
                if cache.is_none()
                    && !selected.keys().any(|&(i, x, y)| {
                        i == image_index
                            && x >= tile.core_x
                            && x < tile.core_x + tile.core_width
                            && y >= tile.core_y
                            && y < tile.core_y + tile.core_height
                    })
                {
                    continue;
                }
                let data = extractor.extract(image, features, &tile).await?;
                let mut samples = Vec::new();
                for (offset, row) in data.rows.iter().enumerate() {
                    let pixel = u32::try_from(offset)?;
                    let key = (
                        image_index,
                        data.x + pixel % data.width,
                        data.y + pixel / data.width,
                    );
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

    pub async fn add_context(
        &mut self,
        first_stage_forest: &Forest,
        extractor: &FeatureExtractor,
        features: &FeatureConfig,
        images: &[Cow<'_, image::RgbaImage>],
        inference: &InferenceConfig,
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
        self.feature_count += features.context_features().count();
        Ok(())
    }

    pub fn fit(&self, parameters: &ForestConfig) -> Result<(Forest, f64)> {
        let rows = &self.rows;
        let labels = &self.labels;
        ensure!(
            rows.len() == labels.len()
                && labels.contains(&0)
                && labels.contains(&1)
                && labels.iter().all(|&v| v <= 1),
            "Training requires foreground and background samples"
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
