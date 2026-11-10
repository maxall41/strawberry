use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use strawberry_types::{PipelineConfig, ToolAssets};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const CONFIG_FILE: &str = "pipeline.toml";

/// Concrete tool-state parsing is deferred until all configuration and assets are available.
#[derive(Deserialize)]
struct ArchiveConfig {
    tools: HashMap<String, serde_value::Value>,
    #[serde(default)]
    assets: BTreeMap<String, Vec<String>>,
}

pub(crate) struct LoadedPipeline {
    pub tools: HashMap<String, serde_value::Value>,
    pub indices: HashMap<String, u16>,
    pub assets: HashMap<String, ToolAssets>,
}

pub(crate) fn save(
    path: &Path,
    config: &PipelineConfig,
    indices: &HashMap<String, u16>,
    assets: &BTreeMap<String, ToolAssets>,
) -> Result<()> {
    #[derive(Serialize)]
    struct SavedConfig<'a> {
        #[serde(flatten)]
        config: &'a PipelineConfig,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        assets: BTreeMap<&'a str, Vec<&'a str>>,
    }
    let saved = SavedConfig {
        config,
        assets: assets
            .iter()
            .filter(|(_, files)| !files.is_empty())
            .map(|(tool, files)| (tool.as_str(), files.keys().map(String::as_str).collect()))
            .collect(),
    };
    let mut value = toml::Value::try_from(saved)?;
    if let Some(tools) = value.get_mut("tools").and_then(toml::Value::as_table_mut) {
        for (name, tool) in tools {
            let Some(index) = indices.get(name) else {
                continue;
            };
            let Some(state) = tool
                .as_table_mut()
                .and_then(|variants| variants.iter_mut().next().map(|(_, value)| value))
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            state.insert("index".to_string(), toml::Value::Integer((*index).into()));
        }
    }
    let toml = toml::to_string(&value)?;
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(true);
    {
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        archive.start_file(CONFIG_FILE, options)?;
        archive.write_all(toml.as_bytes())?;
        for (tool, files) in assets {
            for (name, bytes) in files {
                archive.start_file(format!("{tool}/{name}"), options)?;
                archive.write_all(bytes)?;
            }
        }
        archive.finish()?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

pub(crate) fn load(path: &Path) -> Result<LoadedPipeline> {
    let mut archive =
        ZipArchive::new(File::open(path)?).context("Invalid .sbp pipeline archive")?;
    let mut content = String::new();
    archive
        .by_name(CONFIG_FILE)
        .context("Pipeline archive is missing pipeline.toml")?
        .read_to_string(&mut content)?;
    let config: ArchiveConfig = toml::from_str(&content).context("Invalid pipeline TOML")?;
    let indices = config
        .tools
        .iter()
        .filter_map(|(name, tool)| {
            use serde_value::Value;
            let Value::Map(variants) = tool else {
                return None;
            };
            let Value::Map(fields) = variants.values().next()? else {
                return None;
            };
            let Value::I64(index) = fields.get(&Value::String("index".to_string()))? else {
                return None;
            };
            u16::try_from(*index)
                .ok()
                .map(|index| (name.clone(), index))
        })
        .collect();
    let mut assets = HashMap::new();
    for (tool, names) in config.assets {
        anyhow::ensure!(
            config.tools.contains_key(&tool),
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
            anyhow::ensure!(
                files.insert(name, bytes).is_none(),
                "Duplicate pipeline asset: {entry}"
            );
        }
        assets.insert(tool, files);
    }
    Ok(LoadedPipeline {
        tools: config.tools,
        indices,
        assets,
    })
}
