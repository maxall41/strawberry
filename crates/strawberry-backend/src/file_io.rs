use std::{fs::File, path::Path};

use anyhow::{Context, Result};

pub(crate) fn write_artifact(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
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
