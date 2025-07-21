use crate::composite::GpuCompositor;

use super::Tool;
use async_trait::async_trait;

pub struct Compositor {}

#[async_trait]
impl Tool for Compositor {
    fn name(&self) -> &str {
        "Compositor"
    }
    fn is_enabled(&self) -> bool {
        true
    }
    fn id(&self) -> &str {
        "Compositor"
    }
    async fn process(
        &mut self,
        input: Vec<image::RgbaImage>,
    ) -> anyhow::Result<Vec<image::RgbaImage>> {
        let compositer = GpuCompositor::new().await;

        let composited = compositer.composite_images(&input).await.unwrap();

        Ok(vec![composited])
    }
}

impl Compositor {
    pub fn new() -> Self {
        Compositor {}
    }
}
