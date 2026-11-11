use super::FeatureTile;
use crate::tiling::TileInfo;
use anyhow::{Context, Result};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

/// Keep standard feature planes on disk between stages, with only active tiles in RAM.
pub(super) struct FeatureCache {
    file: File,
    pub tiles: Vec<CachedTile>,
    feature_count: usize,
}

pub(super) struct CachedTile {
    pub image_index: usize,
    pub tile: TileInfo,
    // Pairs of tile pixel offsets and labeled training-row indices.
    pub samples: Vec<(usize, usize)>,
    offset: u64,
}

impl FeatureCache {
    pub fn new(feature_count: usize) -> Result<Self> {
        Ok(Self {
            file: tempfile::tempfile()?,
            tiles: Vec::new(),
            feature_count,
        })
    }

    pub fn store(
        &mut self,
        image_index: usize,
        tile: &TileInfo,
        features: &FeatureTile,
        samples: Vec<(usize, usize)>,
    ) -> Result<()> {
        let offset = self.file.stream_position()?;
        let values: Vec<f32> = features.rows.iter().flatten().copied().collect();
        self.file.write_all(bytemuck::cast_slice(&values))?;
        self.tiles.push(CachedTile {
            image_index,
            tile: tile.clone(),
            samples,
            offset,
        });
        Ok(())
    }

    pub fn load(&mut self, index: usize) -> Result<FeatureTile> {
        self.tiles
            .get(index)
            .context("Missing cached feature tile")?
            .load(&mut self.file, self.feature_count)
    }

    pub fn load_batch(&mut self, indices: &[usize]) -> Result<Vec<FeatureTile>> {
        indices.iter().map(|&index| self.load(index)).collect()
    }

    pub fn image_tiles(&self, image_index: usize) -> Vec<usize> {
        self.tiles
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (entry.image_index == image_index).then_some(index))
            .collect()
    }
}

impl CachedTile {
    fn load(&self, file: &mut File, feature_count: usize) -> Result<FeatureTile> {
        let count = crate::pixel_count(self.tile.core_width, self.tile.core_height)?
            .checked_mul(feature_count)
            .context("Feature cache size overflow")?;
        let mut values = vec![0.0f32; count];
        file.seek(SeekFrom::Start(self.offset))?;
        file.read_exact(bytemuck::cast_slice_mut(&mut values))?;
        Ok(FeatureTile {
            x: self.tile.core_x,
            y: self.tile.core_y,
            width: self.tile.core_width,
            height: self.tile.core_height,
            rows: values
                .chunks_exact(feature_count)
                .map(<[f32]>::to_vec)
                .collect(),
        })
    }
}
