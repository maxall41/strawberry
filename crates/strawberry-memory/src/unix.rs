use anyhow::{Context, Result};
use rustix::fs::statvfs;
use std::path::Path;

/// Return the bytes an unprivileged process may still write to the volume holding `path`.
///
/// # Errors
/// Returns an error if the volume cannot be queried.
pub fn get_available_disk_space(path: &Path) -> Result<u64> {
    let volume = statvfs(path)
        .with_context(|| format!("Cannot read the free space of {}", path.display()))?;
    Ok(volume.f_bavail.saturating_mul(volume.f_frsize))
}
