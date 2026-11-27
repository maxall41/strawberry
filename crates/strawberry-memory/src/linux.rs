use crate::{MemorySample, OsPressure};
use anyhow::{Context, Result};
use std::fs;

/// Return a `Name:   123 kB` field of a `/proc` status file in bytes.
fn get_field_bytes(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(name)?.strip_prefix(':')?;
        let kibibytes: u64 = value.trim().strip_suffix("kB")?.trim().parse().ok()?;
        kibibytes.checked_mul(1024)
    })
}

/// Build a sample from the text of `/proc/meminfo` and `/proc/self/status`.
fn parse_sample(meminfo: &str, status: &str) -> Result<MemorySample> {
    let total = get_field_bytes(meminfo, "MemTotal").context("/proc/meminfo has no MemTotal")?;
    let available =
        get_field_bytes(meminfo, "MemAvailable").context("/proc/meminfo has no MemAvailable")?;
    let resident = get_field_bytes(status, "VmRSS").context("/proc/self/status has no VmRSS")?;
    // Swapped pages still belong to the process; kernels built without swap omit the field.
    let swapped = get_field_bytes(status, "VmSwap").unwrap_or(0);
    Ok(MemorySample {
        total,
        available,
        footprint: resident.saturating_add(swapped),
        os_pressure: OsPressure::Normal,
    })
}

/// Measure physical memory from the kernel's own estimate of what is available.
///
/// # Errors
/// Returns an error if `/proc` cannot be read or lacks the expected fields.
pub fn sample_memory() -> Result<MemorySample> {
    let meminfo = fs::read_to_string("/proc/meminfo").context("Cannot read /proc/meminfo")?;
    let status =
        fs::read_to_string("/proc/self/status").context("Cannot read /proc/self/status")?;
    parse_sample(&meminfo, &status)
}

#[cfg(test)]
mod tests {
    use super::{get_field_bytes, parse_sample};
    use crate::OsPressure;

    const MEMINFO: &str = "MemTotal:       16303428 kB\n\
        MemFree:          254284 kB\n\
        MemAvailable:    9876540 kB\n\
        SwapTotal:       2097148 kB\n";
    const STATUS: &str = "Name:\tstrawberry\n\
        VmSize:\t 9999999 kB\n\
        VmRSS:\t  512000 kB\n\
        VmSwap:\t   64000 kB\n";

    #[test]
    fn sample_uses_available_memory_and_counts_swapped_pages() {
        let sample = parse_sample(MEMINFO, STATUS).unwrap();
        assert_eq!(sample.total, 16_303_428 * 1024);
        assert_eq!(sample.available, 9_876_540 * 1024);
        assert_eq!(sample.footprint, (512_000 + 64_000) * 1024);
        assert_eq!(sample.os_pressure, OsPressure::Normal);
    }

    #[test]
    fn a_missing_swap_field_counts_as_nothing_swapped() {
        let status = "VmRSS:\t  512000 kB\n";
        assert_eq!(
            parse_sample(MEMINFO, status).unwrap().footprint,
            512_000 * 1024
        );
    }

    #[test]
    fn a_missing_required_field_is_an_error() {
        assert!(parse_sample("MemTotal: 1 kB\n", STATUS).is_err());
        assert!(parse_sample(MEMINFO, "Name:\tstrawberry\n").is_err());
    }

    #[test]
    fn a_field_is_matched_by_its_whole_name() {
        assert_eq!(get_field_bytes(MEMINFO, "Mem"), None);
        assert_eq!(get_field_bytes(MEMINFO, "MemFree"), Some(254_284 * 1024));
    }
}
