use super::annotations::rasterize_labels;
use anyhow::{Result, ensure};
use image::RgbaImage;
use std::{borrow::Cow, collections::HashMap, sync::Arc};
use strawberry_processing::segmentation::{
    FeatureConfig, FeatureExtractor, ForestConfig, InferenceConfig, SegmentationModel,
};
use strawberry_types::{AnnotationKind, AnnotationStroke, ClassId};

pub(crate) struct TrainingSample {
    pub(crate) image: Arc<RgbaImage>,
    pub(crate) annotations: Vec<(AnnotationKind, AnnotationStroke)>,
}

pub(crate) async fn train(
    samples: &[TrainingSample],
    class_ids: &[ClassId],
    features: FeatureConfig,
    forest: ForestConfig,
    inference: &InferenceConfig,
    extractor: &FeatureExtractor,
) -> Result<SegmentationModel> {
    let images = samples
        .iter()
        .map(|sample| forest.rescale_input(&sample.image))
        .collect::<Result<Vec<_>>>()?;
    let candidates = collect_candidates(samples, &images, class_ids);
    let selected = select_training_pixels(&candidates, forest.balance_classes)?;
    SegmentationModel::train(features, forest, extractor, &images, &selected, inference).await
}

fn collect_candidates(
    samples: &[TrainingSample],
    images: &[Cow<'_, RgbaImage>],
    class_ids: &[ClassId],
) -> Vec<Vec<(usize, u32, u32)>> {
    let mut candidates = vec![Vec::new(); class_ids.len()];
    for (image_index, (sample, image)) in samples.iter().zip(images).enumerate() {
        let labels = rasterize_labels(
            sample.image.dimensions(),
            image.dimensions(),
            sample
                .annotations
                .iter()
                .map(|(kind, stroke)| (*kind, stroke)),
            class_ids,
        );
        for ((y, x), label) in labels {
            if let Some(class) = candidates.get_mut(usize::from(label)) {
                class.push((image_index, x, y));
            }
        }
    }
    candidates
}

fn select_training_pixels(
    candidates: &[Vec<(usize, u32, u32)>],
    balance_classes: bool,
) -> Result<HashMap<(usize, u32, u32), u8>> {
    let minority_count = candidates.iter().map(Vec::len).min().unwrap_or(0);
    ensure!(minority_count > 0, "Annotate every class before training");
    let mut selected = HashMap::new();
    for (label, class) in candidates.iter().enumerate() {
        let count = if balance_classes {
            minority_count
        } else {
            class.len()
        };
        for i in 0..count {
            let index = if count == class.len() {
                i
            } else {
                i * class.len() / count
            };
            if let Some(&key) = class.get(index) {
                selected.insert(key, u8::try_from(label)?);
            }
        }
    }
    Ok(selected)
}
