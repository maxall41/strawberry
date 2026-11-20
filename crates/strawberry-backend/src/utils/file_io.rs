use std::{
    fs::File,
    io::{Cursor, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use image::{ImageFormat, RgbaImage};
use tempfile::NamedTempFile;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// Create a temporary file beside `path`, so persisting it can atomically replace `path`.
fn create_temporary_beside(path: &Path) -> Result<NamedTempFile> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    NamedTempFile::new_in(directory)
        .with_context(|| format!("Cannot create temporary file in {}", directory.display()))
}

/// Flush a completed temporary file to disk and move it over `path`.
fn persist_temporary(temporary: NamedTempFile, path: &Path) -> Result<()> {
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("Cannot sync {}", path.display()))?;
    temporary
        .persist(path)
        .with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}

/// Write artifact bytes to the requested file path.
pub fn write_artifact(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let mut temporary = create_temporary_beside(path)?;
    write(temporary.as_file_mut()).with_context(|| format!("Cannot write {}", path.display()))?;
    persist_temporary(temporary, path)
}

/// Writes images one at a time as TIFF entries of a ZIP archive, which replaces its
/// destination once finished.
pub struct ImageArchiveWriter {
    archive: ZipWriter<NamedTempFile>,
    path: PathBuf,
    /// Entry names start with the archive's file stem.
    stem: String,
    /// Encoded TIFF bytes, reused between entries.
    tiff: Vec<u8>,
}

impl ImageArchiveWriter {
    /// Start an archive that will replace `path` when finished.
    pub fn create(path: &Path) -> Result<Self> {
        let stem = path.file_stem().map_or_else(
            || "images".into(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        Ok(Self {
            archive: ZipWriter::new(create_temporary_beside(path)?),
            path: path.to_path_buf(),
            stem,
            tiff: Vec::new(),
        })
    }

    /// Append an image as a TIFF named after the archive plus `suffix`.
    pub fn append(&mut self, suffix: &str, image: &RgbaImage) -> Result<()> {
        // Deflate keeps the archive readable by common unzip tools, unlike Zstd.
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        self.tiff.clear();
        // The TIFF encoder needs to seek, which a ZIP entry cannot do.
        image.write_to(&mut Cursor::new(&mut self.tiff), ImageFormat::Tiff)?;
        self.archive
            .start_file(format!("{}_{suffix}.tif", self.stem), options)?;
        self.archive
            .write_all(&self.tiff)
            .with_context(|| format!("Cannot write {}", self.path.display()))?;
        Ok(())
    }

    /// Complete the archive and move it over its destination.
    pub fn finish(self) -> Result<()> {
        let temporary = self
            .archive
            .finish()
            .with_context(|| format!("Cannot write {}", self.path.display()))?;
        persist_temporary(temporary, &self.path)
    }
}
