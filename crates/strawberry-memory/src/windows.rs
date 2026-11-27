#![expect(
    unsafe_code,
    reason = "Windows reports memory through system calls that have no safe wrapper"
)]

use crate::{MemorySample, OsPressure};
use anyhow::{Result, ensure};
use std::{io, iter, os::windows::ffi::OsStrExt, path::Path, ptr};
use windows_sys::Win32::{
    Storage::FileSystem::GetDiskFreeSpaceExW,
    System::{
        ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::GetCurrentProcess,
    },
};

/// Read installed and available physical memory. The compression store is a working set,
/// so it already counts as used at the size it occupies in RAM.
fn read_system_memory() -> Result<MEMORYSTATUSEX> {
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(size_of::<MEMORYSTATUSEX>())?,
        ..Default::default()
    };
    // SAFETY: `status` is live and its `dwLength` states its size, as the call requires.
    let succeeded = unsafe { GlobalMemoryStatusEx(&raw mut status) };
    ensure!(
        succeeded != 0,
        "Cannot read memory status: {}",
        io::Error::last_os_error()
    );
    Ok(status)
}

/// Read this process's private commit, which keeps counting pages after the system has
/// compressed or paged them out of the working set.
fn read_footprint() -> Result<u64> {
    let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>())?;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size,
        ..Default::default()
    };
    // SAFETY: the call takes no arguments and returns a pseudo-handle that is always valid
    // and never needs closing.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: `counters` is live and `size` is its size, so at most that many bytes are
    // written. The call accepts the extended struct through the base struct's pointer type
    // and tells them apart by `size`.
    let succeeded = unsafe {
        K32GetProcessMemoryInfo(
            process,
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            size,
        )
    };
    ensure!(
        succeeded != 0,
        "Cannot read process memory: {}",
        io::Error::last_os_error()
    );
    Ok(u64::try_from(counters.PrivateUsage)?)
}

/// Measure physical memory. Windows gives no pressure verdict of its own here.
///
/// # Errors
/// Returns an error if the system refuses one of the readings.
pub fn sample_memory() -> Result<MemorySample> {
    let status = read_system_memory()?;
    Ok(MemorySample {
        total: status.ullTotalPhys,
        available: status.ullAvailPhys,
        footprint: read_footprint()?,
        os_pressure: OsPressure::Normal,
    })
}

/// Return the bytes this user may still write to the volume holding `path`.
///
/// # Errors
/// Returns an error if the volume cannot be queried.
pub fn get_available_disk_space(path: &Path) -> Result<u64> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let mut available = 0_u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call, `available` is a live `u64`,
    // and the two totals that are not wanted are passed as null, which the call allows.
    let succeeded = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &raw mut available,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    ensure!(
        succeeded != 0,
        "Cannot read the free space of {}: {}",
        path.display(),
        io::Error::last_os_error()
    );
    Ok(available)
}
