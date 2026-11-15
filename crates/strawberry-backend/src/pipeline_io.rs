use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_value::Value;
use strawberry_types::{PipelineConfig, ToolAssets};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::file_io::write_artifact;

const CONFIG_FILE: &str = "pipeline.toml";

#[derive(Serialize)]
struct SavedConfig {
    #[serde(flatten)]
    config: PipelineConfig,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    assets: HashMap<String, Vec<String>>,
}

/// Concrete tool-state parsing is deferred until settings and assets are available.
#[derive(Deserialize)]
struct ArchiveConfig {
    tools: HashMap<String, Value>,
    #[serde(default)]
    assets: HashMap<String, Vec<String>>,
}

pub(crate) struct LoadedPipeline {
    pub tools: HashMap<String, Value>,
    pub indices: HashMap<String, u16>,
    pub assets: HashMap<String, ToolAssets>,
}

fn serialize_config(
    config: &PipelineConfig,
    indices: &HashMap<String, u16>,
    assets: &HashMap<String, ToolAssets>,
) -> Result<String> {
    let saved = SavedConfig {
        config: config.clone(),
        assets: assets
            .iter()
            .filter(|(_, files)| !files.is_empty())
            .map(|(tool, files)| (tool.clone(), files.keys().cloned().collect()))
            .collect(),
    };

    let mut document = toml::Value::try_from(saved)?;
    if let Some(tools) = document
        .get_mut("tools")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, tool) in tools {
            if let Some(index) = indices.get(name) {
                insert_tool_index(tool, *index)?;
            }
        }
    }
    Ok(toml::to_string(&document)?)
}

fn insert_tool_index(tool: &mut toml::Value, index: u16) -> Result<()> {
    let fields = tool
        .as_table_mut()
        .and_then(|variants| variants.iter_mut().next().map(|(_, value)| value))
        .and_then(toml::Value::as_table_mut)
        .context("Expected an externally tagged tool state")?;
    fields.insert("index".into(), toml::Value::Integer(index.into()));
    Ok(())
}

fn tool_index(tool: &Value) -> Option<u16> {
    let Value::Map(variants) = tool else {
        return None;
    };
    let Value::Map(fields) = variants.values().next()? else {
        return None;
    };
    let Value::I64(index) = fields.get(&Value::String("index".into()))? else {
        return None;
    };
    u16::try_from(*index).ok()
}

pub(crate) fn save(
    path: &Path,
    config: &PipelineConfig,
    indices: &HashMap<String, u16>,
    assets: &HashMap<String, ToolAssets>,
) -> Result<()> {
    let config = serialize_config(config, indices, assets)?;
    write_artifact(path, |file| write_archive(file, &config, assets))
}

fn write_archive(
    file: &mut File,
    config: &str,
    assets: &HashMap<String, ToolAssets>,
) -> Result<()> {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    let mut archive = ZipWriter::new(file);
    archive.start_file(CONFIG_FILE, options)?;
    archive.write_all(config.as_bytes())?;
    for (tool, files) in assets {
        for (name, bytes) in files {
            archive.start_file(format!("{tool}/{name}"), options)?;
            archive.write_all(bytes)?;
        }
    }
    archive.finish()?;
    Ok(())
}

pub(crate) fn load(path: &Path) -> Result<LoadedPipeline> {
    let mut archive =
        ZipArchive::new(File::open(path)?).context("Invalid .sbp pipeline archive")?;
    let config = read_config(&mut archive)?;
    let indices = config
        .tools
        .iter()
        .filter_map(|(name, tool)| tool_index(tool).map(|index| (name.clone(), index)))
        .collect();
    let assets = read_assets(&mut archive, &config)?;
    Ok(LoadedPipeline {
        tools: config.tools,
        indices,
        assets,
    })
}

fn read_config(archive: &mut ZipArchive<File>) -> Result<ArchiveConfig> {
    let mut content = String::new();
    archive
        .by_name(CONFIG_FILE)
        .context("Pipeline archive is missing pipeline.toml")?
        .read_to_string(&mut content)?;
    toml::from_str(&content).context("Invalid pipeline TOML")
}

fn read_assets(
    archive: &mut ZipArchive<File>,
    config: &ArchiveConfig,
) -> Result<HashMap<String, ToolAssets>> {
    let mut assets = HashMap::new();
    for (tool, names) in &config.assets {
        ensure!(
            config.tools.contains_key(tool),
            "Assets for unknown tool: {tool}"
        );
        let mut files = ToolAssets::new();
        for name in names {
            let entry = format!("{tool}/{name}");
            let mut bytes = Vec::new();
            archive
                .by_name(&entry)
                .with_context(|| format!("Pipeline archive is missing {entry}"))?
                .read_to_end(&mut bytes)?;
            ensure!(
                files.insert(name.clone(), bytes).is_none(),
                "Duplicate pipeline asset: {entry}"
            );
        }
        assets.insert(tool.clone(), files);
    }
    Ok(assets)
}
