use std::{
    fs::File,
    io::{Cursor, Write},
    path::Path,
};

use anyhow::{Context, Result};
use image::{ImageFormat, RgbaImage};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

/// Write artifact bytes to the requested file path.
pub fn write_artifact(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .with_context(|| format!("Cannot create temporary file in {}", directory.display()))?;
    write(temporary.as_file_mut()).with_context(|| format!("Cannot write {}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("Cannot sync {}", path.display()))?;
    temporary
        .persist(path)
        .with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}

/// Write each image as a numbered TIFF, named after the archive, inside a ZIP archive.
pub fn write_stack_archive(path: &Path, images: &[RgbaImage]) -> Result<()> {
    let stem = path
        .file_stem()
        .map_or_else(|| "stack".into(), |stem| stem.to_string_lossy());
    let width = images.len().to_string().len();
    write_artifact(path, |file| {
        // Deflate keeps the archive readable by common unzip tools, unlike Zstd.
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .large_file(true);
        let mut archive = ZipWriter::new(file);
        let mut tiff = Vec::new();
        for (number, image) in (1..).zip(images) {
            tiff.clear();
            // The TIFF encoder needs to seek, which a ZIP entry cannot do.
            image.write_to(&mut Cursor::new(&mut tiff), ImageFormat::Tiff)?;
            archive.start_file(format!("{stem}_{number:0width$}.tif"), options)?;
            archive.write_all(&tiff)?;
        }
        archive.finish()?;
        Ok(())
    })
}
