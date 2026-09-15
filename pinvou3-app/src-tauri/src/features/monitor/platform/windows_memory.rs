use super::super::RamSnapshot;

use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub fn ram_snapshot() -> Option<RamSnapshot> {
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;

    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return None;
    }

    snapshot_from_values(
        status.ullTotalPhys,
        status.ullAvailPhys,
        status.ullTotalPageFile,
        status.ullAvailPageFile,
    )
}

fn snapshot_from_values(
    total_phys_bytes: u64,
    avail_phys_bytes: u64,
    total_pagefile_bytes: u64,
    avail_pagefile_bytes: u64,
) -> Option<RamSnapshot> {
    if total_phys_bytes == 0 || avail_phys_bytes > total_phys_bytes {
        return None;
    }

    let used_phys_bytes = total_phys_bytes.saturating_sub(avail_phys_bytes);
    let used_commit_bytes = total_pagefile_bytes.saturating_sub(avail_pagefile_bytes);
    let swap_total_bytes = total_pagefile_bytes.saturating_sub(total_phys_bytes);
    let swap_used_bytes = used_commit_bytes
        .saturating_sub(used_phys_bytes)
        .min(swap_total_bytes);

    Some(RamSnapshot {
        total_kib: bytes_to_kib(total_phys_bytes),
        used_kib: bytes_to_kib(used_phys_bytes),
        swap_total_kib: bytes_to_kib(swap_total_bytes),
        swap_used_kib: bytes_to_kib(swap_used_bytes),
    })
}

fn bytes_to_kib(bytes: u64) -> u64 {
    bytes / 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_to_kib_rounds_down() {
        assert_eq!(bytes_to_kib(4096), 4);
        assert_eq!(bytes_to_kib(4095), 3);
    }

    #[test]
    fn snapshot_from_values_returns_physical_memory() {
        let snapshot = snapshot_from_values(16 * 1024, 4 * 1024, 24 * 1024, 8 * 1024).unwrap();
        assert_eq!(snapshot.total_kib, 16);
        assert_eq!(snapshot.used_kib, 12);
        assert_eq!(snapshot.swap_total_kib, 8);
        assert_eq!(snapshot.swap_used_kib, 4);
    }

    #[test]
    fn snapshot_from_values_rejects_invalid_available_memory() {
        assert!(snapshot_from_values(1024, 2048, 2048, 1024).is_none());
    }

    #[test]
    fn ram_snapshot_returns_physical_memory_on_windows() {
        let snapshot = ram_snapshot().expect("Windows memory status should be readable");
        assert!(snapshot.total_kib > 0);
        assert!(snapshot.used_kib <= snapshot.total_kib);
    }
}
