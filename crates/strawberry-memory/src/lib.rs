//! Measures the physical memory and disk space the operating system has left.
//!
//! This is the only crate in the workspace that may contain `unsafe`: the memory figures
//! come from system calls that have no safe wrapper.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::sample_memory;
#[cfg(target_os = "macos")]
pub use macos::sample_memory;
#[cfg(unix)]
pub use unix::get_available_disk_space;
#[cfg(windows)]
pub use windows::{get_available_disk_space, sample_memory};

/// The operating system's own verdict on memory pressure, on systems that give one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsPressure {
    Normal,
    Warning,
    Critical,
}

/// Physical memory as the operating system accounts for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySample {
    /// Installed physical memory in bytes.
    pub total: u64,
    /// Bytes the system can hand out without compressing or swapping. Compressed memory
    /// counts as used at the size it occupies in RAM, not at its size before compression.
    pub available: u64,
    /// This process's memory in bytes, including pages the system has compressed or swapped
    /// out, which resident-set figures leave out exactly when memory is short.
    pub footprint: u64,
    pub os_pressure: OsPressure,
}

#[cfg(test)]
mod tests {
    use super::{get_available_disk_space, sample_memory};
    use std::{env, hint::black_box};

    const TOUCHED_BYTES: usize = 256 * 1024 * 1024;

    #[test]
    fn sample_is_consistent_with_itself() {
        let sample = sample_memory().unwrap();
        assert!(sample.total > 0);
        assert!(sample.available > 0);
        assert!(sample.available <= sample.total);
        assert!(sample.footprint > 0);
        assert!(sample.footprint < sample.total);
    }

    #[test]
    fn footprint_counts_newly_touched_memory() {
        let before = sample_memory().unwrap().footprint;
        // A non-zero fill writes every page; a zeroed allocation would stay untouched.
        let touched = black_box(vec![1_u8; TOUCHED_BYTES]);
        let after = sample_memory().unwrap().footprint;
        let grown = after.saturating_sub(before);
        assert!(
            grown >= u64::try_from(TOUCHED_BYTES / 2).unwrap(),
            "footprint grew by {grown} bytes after touching {TOUCHED_BYTES}"
        );
        drop(touched);
    }

    #[test]
    fn temporary_directory_has_free_space() {
        assert!(get_available_disk_space(&env::temp_dir()).unwrap() > 0);
    }
}
