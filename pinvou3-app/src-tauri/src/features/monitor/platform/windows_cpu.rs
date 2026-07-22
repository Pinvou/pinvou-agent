use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    REG_EXPAND_SZ, REG_SZ,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes};

use crate::monitor::CpuSnapshot;

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
struct SystemTimes {
    idle_100ns: u64,
    total_100ns: u64,
}

#[derive(Debug, Clone, Copy)]
struct ProcessTimes {
    cpu_100ns: u64,
    sampled_at: Instant,
}

#[derive(Debug, Default)]
struct CpuSampleState {
    system: Option<SystemTimes>,
    process: Option<ProcessTimes>,
    pdh: Option<PdhCpuCounter>,
}

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let logical_processors = logical_processor_count();
    let name = cpu_name().unwrap_or_else(|| "CPU".to_string());
    let system = read_system_times();
    let process = read_process_times();
    let state = CPU_SAMPLE_STATE.get_or_init(|| Mutex::new(CpuSampleState::default()));
    let mut state = state.lock();

    if state.pdh.is_none() {
        state.pdh = PdhCpuCounter::new();
    }

    let system_usage = match (state.system, system) {
        (Some(prev), Some(current)) => system_usage_pct(prev, current),
        _ => None,
    };
    let total_usage_pct = state
        .pdh
        .as_mut()
        .and_then(PdhCpuCounter::sample)
        .or(system_usage);
    let process_usage_pct = match (state.process, process) {
        (Some(prev), Some(current)) => process_usage_pct(prev, current, logical_processors),
        _ => None,
    };

    if system.is_some() {
        state.system = system;
    }
    if process.is_some() {
        state.process = process;
    }

    Some(CpuSnapshot {
        name,
        total_usage_pct,
        process_usage_pct,
        logical_processors,
    })
}

#[derive(Debug)]
struct PdhCpuCounter {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

unsafe impl Send for PdhCpuCounter {}

impl PdhCpuCounter {
    fn new() -> Option<Self> {
        [
            r"\Processor Information(_Total)\% Processor Utility",
            r"\Processor(_Total)\% Processor Time",
        ]
        .into_iter()
        .find_map(Self::open)
    }

    fn open(path: &str) -> Option<Self> {
        let mut query = std::ptr::null_mut();
        let open_status = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) };
        if open_status != 0 {
            return None;
        }

        let path = wide_null(path);
        let mut counter = std::ptr::null_mut();
        let add_status =
            unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) };
        if add_status != 0 {
            unsafe {
                PdhCloseQuery(query);
            }
            return None;
        }

        unsafe {
            PdhCollectQueryData(query);
        }
        Some(Self { query, counter })
    }

    fn sample(&mut self) -> Option<f64> {
        let collect_status = unsafe { PdhCollectQueryData(self.query) };
        if collect_status != 0 {
            return None;
        }

        let mut value_type = 0;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        let value_status = unsafe {
            PdhGetFormattedCounterValue(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut value_type,
                &mut value,
            )
        };
        if value_status != 0 || value.CStatus != 0 {
            return None;
        }

        Some(clamp_pct(unsafe { value.Anonymous.doubleValue }))
    }
}

impl Drop for PdhCpuCounter {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

fn read_system_times() -> Option<SystemTimes> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    Some(SystemTimes {
        idle_100ns: filetime_to_u64(idle),
        total_100ns: filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)),
    })
}

fn read_process_times() -> Option<ProcessTimes> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return None;
    }
    Some(ProcessTimes {
        cpu_100ns: filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)),
        sampled_at: Instant::now(),
    })
}

fn system_usage_pct(prev: SystemTimes, current: SystemTimes) -> Option<f64> {
    let total_delta = current.total_100ns.checked_sub(prev.total_100ns)?;
    if total_delta == 0 {
        return None;
    }
    let idle_delta = current.idle_100ns.saturating_sub(prev.idle_100ns);
    let busy_delta = total_delta.saturating_sub(idle_delta);
    Some(clamp_pct(busy_delta as f64 * 100.0 / total_delta as f64))
}

fn process_usage_pct(
    prev: ProcessTimes,
    current: ProcessTimes,
    logical_processors: u32,
) -> Option<f64> {
    if logical_processors == 0 {
        return None;
    }
    let cpu_delta = current.cpu_100ns.checked_sub(prev.cpu_100ns)?;
    let elapsed_100ns = current
        .sampled_at
        .checked_duration_since(prev.sampled_at)?
        .as_secs_f64()
        * 10_000_000.0;
    if elapsed_100ns <= 0.0 {
        return None;
    }
    Some(clamp_pct(
        cpu_delta as f64 * 100.0 / elapsed_100ns / logical_processors as f64,
    ))
}

fn clamp_pct(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn logical_processor_count() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(0)
}

fn cpu_name() -> Option<String> {
    read_registry_string(
        HKEY_LOCAL_MACHINE,
        r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
        "ProcessorNameString",
    )
    .or_else(|| std::env::var("PROCESSOR_IDENTIFIER").ok())
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn read_registry_string(root: HKEY, key_path: &str, value_name: &str) -> Option<String> {
    let key_path = wide_null(key_path);
    let value_name = wide_null(value_name);
    let mut key = std::ptr::null_mut();
    let open_status = unsafe { RegOpenKeyExW(root, key_path.as_ptr(), 0, KEY_READ, &mut key) };
    if open_status != 0 {
        return None;
    }

    let result = query_registry_string(key, &value_name);
    unsafe {
        RegCloseKey(key);
    }
    result
}

fn query_registry_string(key: HKEY, value_name: &[u16]) -> Option<String> {
    let mut value_type = 0;
    let mut byte_len = 0;
    let query_status = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut byte_len,
        )
    };
    if query_status != 0 || byte_len == 0 || (value_type != REG_SZ && value_type != REG_EXPAND_SZ) {
        return None;
    }

    let mut bytes = vec![0u8; byte_len as usize];
    let read_status = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            bytes.as_mut_ptr(),
            &mut byte_len,
        )
    };
    if read_status != 0 {
        return None;
    }

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16(&units[..end]).ok()
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn filetime_to_u64(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn clamp_usage_pct_clamps_range() {
        assert_eq!(clamp_pct(-1.0), 0.0);
        assert_eq!(clamp_pct(42.5), 42.5);
        assert_eq!(clamp_pct(120.0), 100.0);
    }

    #[test]
    fn system_usage_from_deltas_returns_expected() {
        let prev = SystemTimes {
            idle_100ns: 100,
            total_100ns: 1_000,
        };
        let current = SystemTimes {
            idle_100ns: 200,
            total_100ns: 2_000,
        };
        assert_eq!(system_usage_pct(prev, current), Some(90.0));
    }

    #[test]
    fn system_usage_returns_none_without_elapsed_total() {
        let sample = SystemTimes {
            idle_100ns: 100,
            total_100ns: 1_000,
        };
        assert_eq!(system_usage_pct(sample, sample), None);
    }

    #[test]
    fn process_usage_accounts_for_logical_processors() {
        let now = Instant::now();
        let prev = ProcessTimes {
            cpu_100ns: 0,
            sampled_at: now,
        };
        let current = ProcessTimes {
            cpu_100ns: 10_000_000,
            sampled_at: now + Duration::from_secs(1),
        };
        assert_eq!(process_usage_pct(prev, current, 4), Some(25.0));
    }

    #[test]
    fn process_usage_returns_none_without_elapsed_wall_time() {
        let now = Instant::now();
        let sample = ProcessTimes {
            cpu_100ns: 10,
            sampled_at: now,
        };
        assert_eq!(process_usage_pct(sample, sample, 4), None);
    }

    #[test]
    fn cpu_snapshot_returns_basic_identity() {
        let snapshot = cpu_snapshot().expect("Windows CPU snapshot should include basic identity");
        assert!(!snapshot.name.trim().is_empty());
        assert!(snapshot.logical_processors > 0);
    }
}
