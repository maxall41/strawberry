use anyhow::{Result, anyhow, ensure};
use image::RgbaImage;
use std::sync::Arc;

/// Equally sized layers that are composited into one displayed image.
#[derive(Debug, Clone, Default)]
pub struct ImageStack {
    pub layers: Vec<Arc<RgbaImage>>,
}

impl ImageStack {
    /// Group layers into a stack after checking that they share dimensions.
    ///
    /// # Errors
    /// Returns an error if no layers are given or their dimensions differ.
    pub fn new(layers: Vec<Arc<RgbaImage>>) -> Result<Self> {
        let first = layers
            .first()
            .ok_or_else(|| anyhow!("No input images provided"))?;
        ensure!(
            layers
                .iter()
                .all(|layer| layer.dimensions() == first.dimensions()),
            "All layers in a stack must have the same dimensions"
        );
        Ok(Self { layers })
    }
}

/// How several loaded images are arranged into stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageArrangement {
    /// Composite the images as layers of a single stack.
    Layers,
    /// Give each image its own stack.
    Stacks,
}

/// Every image stack moving through the pipeline; tools process each stack independently.
#[derive(Debug, Clone, Default)]
pub struct ImageCollection {
    pub stacks: Vec<ImageStack>,
}

impl ImageCollection {
    /// Arrange loaded images as the layers of one stack, or as one stack per image.
    ///
    /// # Errors
    /// Returns an error if no images are given or the layers of a stack differ in size.
    pub fn arrange(images: Vec<Arc<RgbaImage>>, arrangement: ImageArrangement) -> Result<Self> {
        ensure!(!images.is_empty(), "No input images provided");
        let stacks = match arrangement {
            ImageArrangement::Layers => vec![ImageStack::new(images)?],
            ImageArrangement::Stacks => images
                .into_iter()
                .map(|image| ImageStack {
                    layers: vec![image],
                })
                .collect(),
        };
        Ok(Self { stacks })
    }

    /// Return the number of stacks.
    #[must_use]
    pub const fn stack_count(&self) -> usize {
        self.stacks.len()
    }

    /// Return whether the collection holds no stacks.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.stacks.is_empty()
    }

    /// Return the number of layers in each stack; every stack is processed identically.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.stacks.first().map_or(0, |stack| stack.layers.len())
    }

    /// Collect one layer from every stack, in stack order.
    #[must_use]
    pub fn collect_layer(&self, layer: usize) -> Vec<Option<Arc<RgbaImage>>> {
        self.stacks
            .iter()
            .map(|stack| stack.layers.get(layer).cloned())
            .collect()
    }

    /// Iterate mutably over every layer of every stack.
    pub fn layers_mut(&mut self) -> impl Iterator<Item = &mut Arc<RgbaImage>> {
        self.stacks
            .iter_mut()
            .flat_map(|stack| stack.layers.iter_mut())
    }
}
