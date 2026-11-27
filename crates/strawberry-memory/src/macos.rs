#![expect(
    unsafe_code,
    reason = "macOS reports memory through system calls that have no safe wrapper"
)]

use crate::{MemorySample, OsPressure};
use anyhow::{Result, ensure};
use libc::{
    HOST_VM_INFO64, HOST_VM_INFO64_COUNT, KERN_SUCCESS, RUSAGE_INFO_V4, c_int, c_void,
    host_statistics64, mach_port_t, proc_pid_rusage, rusage_info_t, rusage_info_v4, sysctlbyname,
    vm_statistics64,
};
use mach2::mach_init::mach_host_self;
use std::{ffi::CStr, io, mem, process, ptr, sync::OnceLock};

/// The page counts that make up memory in use, added up the way Activity Monitor does.
struct PagesInUse {
    /// Anonymous pages, purgeable ones included.
    internal: u64,
    /// Pages the system may discard without asking; they are counted inside `internal`.
    purgeable: u64,
    wired: u64,
    /// Physical pages the compressor occupies, not the larger number of pages stored in it.
    compressor: u64,
}

impl PagesInUse {
    /// Return the number of pages that cannot be handed out without compressing or swapping.
    const fn get_total(&self) -> u64 {
        self.internal
            .saturating_sub(self.purgeable)
            .saturating_add(self.wired)
            .saturating_add(self.compressor)
    }
}

/// Return the bytes of `total` left once the pages in use are taken out.
const fn get_available_memory(total: u64, page_size: u64, pages: &PagesInUse) -> u64 {
    total.saturating_sub(pages.get_total().saturating_mul(page_size))
}

/// Translate the kernel's `kern.memorystatus_vm_pressure_level` into a pressure verdict.
const fn decode_pressure_level(level: c_int) -> OsPressure {
    match level {
        2 => OsPressure::Warning,
        4 => OsPressure::Critical,
        _ => OsPressure::Normal,
    }
}

/// Read the `N` bytes of a fixed-size value the kernel publishes under `name`.
fn read_sysctl<const N: usize>(name: &CStr) -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    let mut length = N;
    // SAFETY: `name` is NUL-terminated. `bytes` is live and `length` holds its size, so the
    // kernel writes at most `N` bytes into it. No new value is passed in.
    let status = unsafe {
        sysctlbyname(
            name.as_ptr(),
            bytes.as_mut_ptr().cast::<c_void>(),
            &raw mut length,
            ptr::null_mut(),
            0,
        )
    };
    ensure!(
        status == 0,
        "Cannot read {}: {}",
        name.to_string_lossy(),
        io::Error::last_os_error()
    );
    ensure!(
        length == N,
        "{} is {length} bytes, expected {N}",
        name.to_string_lossy()
    );
    Ok(bytes)
}

/// Return the port that names this host in the kernel's statistics calls.
fn get_host_port() -> mach_port_t {
    // Every call adds a reference to the port, so ask once and keep the answer.
    static HOST: OnceLock<mach_port_t> = OnceLock::new();
    // SAFETY: `mach_host_self` takes no arguments and only returns a port name.
    *HOST.get_or_init(|| unsafe { mach_host_self() })
}

/// Read the kernel's page counts for the whole system.
fn read_vm_statistics() -> Result<vm_statistics64> {
    // SAFETY: `vm_statistics64` holds only integers, for which all-zero bytes are valid.
    let mut statistics: vm_statistics64 = unsafe { mem::zeroed() };
    let mut count = HOST_VM_INFO64_COUNT;
    // SAFETY: `statistics` is live and `count` is its length in the kernel's units, so the
    // kernel writes inside it. An older system fills a shorter prefix and leaves the rest,
    // which is why the struct starts zeroed.
    let status = unsafe {
        host_statistics64(
            get_host_port(),
            HOST_VM_INFO64,
            (&raw mut statistics).cast::<c_int>(),
            &raw mut count,
        )
    };
    ensure!(
        status == KERN_SUCCESS,
        "Cannot read memory statistics (kernel error {status})"
    );
    Ok(statistics)
}

/// Read this process's physical footprint, which counts its compressed pages as well as
/// its resident ones.
fn read_footprint() -> Result<u64> {
    // SAFETY: `rusage_info_v4` holds only integers, for which all-zero bytes are valid.
    let mut usage: rusage_info_v4 = unsafe { mem::zeroed() };
    let pid = c_int::try_from(process::id())?;
    // SAFETY: `RUSAGE_INFO_V4` tells the kernel the buffer is a `rusage_info_v4`, which the
    // live `usage` is.
    let status = unsafe {
        proc_pid_rusage(
            pid,
            RUSAGE_INFO_V4,
            (&raw mut usage).cast::<rusage_info_t>(),
        )
    };
    ensure!(
        status == 0,
        "Cannot read process memory: {}",
        io::Error::last_os_error()
    );
    Ok(usage.ri_phys_footprint)
}

/// Measure physical memory, counting the compressor at the RAM it occupies.
///
/// # Errors
/// Returns an error if the kernel refuses one of the readings.
pub fn sample_memory() -> Result<MemorySample> {
    let total = u64::from_ne_bytes(read_sysctl(c"hw.memsize")?);
    // The page counts are in kernel pages, which differ from the process's pages when an
    // Intel build runs translated on Apple silicon.
    let page_size = u64::try_from(c_int::from_ne_bytes(read_sysctl(c"vm.pagesize")?))?;
    let statistics = read_vm_statistics()?;
    let pages = PagesInUse {
        internal: u64::from(statistics.internal_page_count),
        purgeable: u64::from(statistics.purgeable_count),
        wired: u64::from(statistics.wire_count),
        compressor: u64::from(statistics.compressor_page_count),
    };
    let level = c_int::from_ne_bytes(read_sysctl(c"kern.memorystatus_vm_pressure_level")?);
    Ok(MemorySample {
        total,
        available: get_available_memory(total, page_size, &pages),
        footprint: read_footprint()?,
        os_pressure: decode_pressure_level(level),
    })
}

#[cfg(test)]
mod tests {
    use super::{PagesInUse, decode_pressure_level, get_available_memory, read_sysctl};
    use crate::OsPressure;

    const PAGE_SIZE: u64 = 16_384;
    const TOTAL: u64 = 24 * 1024 * 1024 * 1024;

    #[test]
    fn used_memory_counts_the_compressor_at_its_physical_size() {
        // Figures from one `vm_stat` reading of a 24 GiB machine.
        let pages = PagesInUse {
            internal: 634_861,
            purgeable: 46_947,
            wired: 278_184,
            compressor: 226_527,
        };
        assert_eq!(pages.get_total(), 1_092_625);
        assert_eq!(
            get_available_memory(TOTAL, PAGE_SIZE, &pages),
            TOTAL - 1_092_625 * PAGE_SIZE
        );
    }

    #[test]
    fn available_memory_never_goes_below_zero() {
        let pages = PagesInUse {
            internal: u64::MAX,
            purgeable: 0,
            wired: u64::MAX,
            compressor: 1,
        };
        assert_eq!(get_available_memory(TOTAL, PAGE_SIZE, &pages), 0);
    }

    #[test]
    fn purgeable_pages_never_take_more_than_the_anonymous_pages() {
        let pages = PagesInUse {
            internal: 10,
            purgeable: 25,
            wired: 3,
            compressor: 4,
        };
        assert_eq!(pages.get_total(), 7);
    }

    #[test]
    fn pressure_levels_decode_as_the_kernel_numbers_them() {
        assert_eq!(decode_pressure_level(1), OsPressure::Normal);
        assert_eq!(decode_pressure_level(2), OsPressure::Warning);
        assert_eq!(decode_pressure_level(4), OsPressure::Critical);
        assert_eq!(decode_pressure_level(0), OsPressure::Normal);
    }

    #[test]
    fn a_value_of_the_wrong_size_is_rejected() {
        assert!(read_sysctl::<4>(c"hw.memsize").is_err());
        assert!(read_sysctl::<8>(c"hw.memsize").is_ok());
    }
}
