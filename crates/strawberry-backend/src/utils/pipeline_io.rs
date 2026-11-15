use super::file_io::write_artifact;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_value::Value;
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
use strawberry_types::{BackendToolType, ToolAssets, ToolState};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};
const VERSION: u32 = 1;

#[derive(Serialize)]
struct SavedConfig<'a> {
    version: u32,
    tools: Vec<SavedEntry<'a>>,
}
#[derive(Serialize)]
struct SavedEntry<'a> {
    state: &'a ToolState,
    assets: Vec<&'a str>,
}
#[derive(Deserialize)]
struct ArchiveConfig {
    version: u32,
    tools: Vec<ArchiveEntry>,
}
#[derive(Deserialize)]
struct ArchiveEntry {
    state: Value,
    #[serde(default)]
    assets: Vec<String>,
}
pub struct LoadedEntry {
    pub state: Value,
    pub assets: ToolAssets,
}
impl LoadedEntry {
    pub fn kind(&self) -> Result<BackendToolType> {
        let Value::Map(variants) = &self.state else {
            anyhow::bail!("Expected tagged tool state");
        };
        ensure!(variants.len() == 1, "Expected one tool state variant");
        let Some(Value::String(tag)) = variants.keys().next() else {
            anyhow::bail!("Invalid tool state tag");
        };
        Ok(match tag.as_str() {
            "AutomaticSegmentationState" => BackendToolType::AutomaticSegmentation,
            "CompositorState" => BackendToolType::Compositor,
            "ContrastState" => BackendToolType::Contrast,
            "CounterState" => BackendToolType::Counter,
            "DecomposerState" => BackendToolType::Decomposer,
            "DifferenceOfGaussiansState" => BackendToolType::DifferenceOfGaussians,
            "GaussianBlurState" => BackendToolType::GaussianBlur,
            "GrayscaleState" => BackendToolType::Grayscale,
            "LocalMaximaState" => BackendToolType::LocalMaxima,
            "MorphologyState" => BackendToolType::Morphology,
            "NerveTracerState" => BackendToolType::NerveTracer,
            "ObjectSizeFilterState" => BackendToolType::ObjectSizeFilter,
            "RollingBallState" => BackendToolType::RollingBall,
            "SobelState" => BackendToolType::Sobel,
            "ThresholdState" => BackendToolType::Threshold,
            "WatershedState" => BackendToolType::Watershed,
            _ => anyhow::bail!("Unknown pipeline tool: {tag}"),
        })
    }
}
fn valid_asset(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']),
        "Invalid asset name: {name}"
    );
    Ok(())
}
pub fn save(path: &Path, entries: &[(ToolState, ToolAssets)]) -> Result<()> {
    let config = SavedConfig {
        version: VERSION,
        tools: entries
            .iter()
            .map(|(state, assets)| SavedEntry {
                state,
                assets: assets.keys().map(String::as_str).collect(),
            })
            .collect(),
    };
    let config = toml::to_string(&config)?;
    write_artifact(path, |file| {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        let mut archive = ZipWriter::new(file);
        archive.start_file("pipeline.toml", options)?;
        archive.write_all(config.as_bytes())?;
        for (index, (_, assets)) in entries.iter().enumerate() {
            for (name, bytes) in assets {
                valid_asset(name)?;
                archive.start_file(format!("tools/{index}/{name}"), options)?;
                archive.write_all(bytes)?;
            }
        }
        archive.finish()?;
        Ok(())
    })
}
pub fn load(path: &Path) -> Result<Vec<LoadedEntry>> {
    let mut archive =
        ZipArchive::new(File::open(path)?).context("Invalid .sbp pipeline archive")?;
    let mut text = String::new();
    archive
        .by_name("pipeline.toml")
        .context("Pipeline archive is missing pipeline.toml")?
        .read_to_string(&mut text)?;
    let document: toml::Value = toml::from_str(&text).context("Invalid pipeline TOML")?;
    ensure!(
        document.get("version").and_then(toml::Value::as_integer) == Some(i64::from(VERSION)),
        "Unsupported pipeline format version (unversioned archives are unsupported)"
    );
    let config: ArchiveConfig = document.try_into()?;
    ensure!(
        config.version == VERSION,
        "Unsupported pipeline format version"
    );
    config
        .tools
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let mut assets = ToolAssets::new();
            for name in entry.assets {
                valid_asset(&name)?;
                let path = format!("tools/{index}/{name}");
                let mut bytes = Vec::new();
                archive
                    .by_name(&path)
                    .with_context(|| format!("Pipeline archive is missing {path}"))?
                    .read_to_end(&mut bytes)?;
                ensure!(
                    assets.insert(name, bytes).is_none(),
                    "Duplicate pipeline asset: {path}"
                );
            }
            Ok(LoadedEntry {
                state: entry.state,
                assets,
            })
        })
        .collect()
}
