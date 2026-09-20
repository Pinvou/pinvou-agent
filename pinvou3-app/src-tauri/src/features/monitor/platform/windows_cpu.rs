use std::sync::OnceLock;

use parking_lot::Mutex;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows_sys::Win32::System::Threading::GetSystemTimes;

use super::super::CpuSnapshot;

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
struct SystemTimes {
    idle_100ns: u64,
    total_100ns: u64,
}

#[derive(Debug, Default)]
struct CpuSampleState {
    system: Option<SystemTimes>,
    pdh: Option<PdhCpuCounter>,
}

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let name = cpu_name().unwrap_or_else(|| "CPU".to_string());
    let system = read_system_times();
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

    if system.is_some() {
        state.system = system;
    }

    Some(CpuSnapshot {
        name,
        total_usage_pct,
    })
}

#[derive(Debug)]
struct PdhCpuCounter {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

// SAFETY: PDH_HQUERY/PDH_HCOUNTER are opaque PDH handles owned by this struct;
// every PDH call on them happens while holding the CPU_SAMPLE_STATE Mutex, so
// the handles are never used concurrently from multiple threads.
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
        // SAFETY: PdhOpenQueryW accepts a null data source (real-time
        // monitoring), dwUserData is an opaque caller cookie (0), and &mut
        // query points to a live nullable PDH_HQUERY out-parameter.
        let open_status = unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut query) };
        if open_status != 0 {
            return None;
        }

        let path = wide_null(path);
        let mut counter = std::ptr::null_mut();
        // SAFETY: query was just opened successfully; path is a
        // NUL-terminated UTF-16 string produced by wide_null, as
        // PdhAddEnglishCounterW requires; &mut counter is a live out-parameter.
        let add_status = unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) };
        if add_status != 0 {
            // SAFETY: query was opened above and is not yet owned by any
            // PdhCpuCounter (this is the failure path before construction), so
            // closing it here is the single, exactly-once close.
            unsafe {
                PdhCloseQuery(query);
            }
            return None;
        }

        // SAFETY: query is open and counter was added to it above. PDH needs
        // one priming collection before formatted values are meaningful; this
        // establishes that baseline.
        unsafe {
            PdhCollectQueryData(query);
        }
        Some(Self { query, counter })
    }

    fn sample(&mut self) -> Option<f64> {
        // SAFETY: self.query is the open PDH query owned by self; &mut self
        // guarantees no concurrent use of the same handles.
        let collect_status = unsafe { PdhCollectQueryData(self.query) };
        if collect_status != 0 {
            return None;
        }

        let mut value_type = 0;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: self.counter belongs to self.query; value_type and value are
        // live writable out-parameters, and PDH_FMT_DOUBLE tells PDH which
        // union member of PDH_FMT_COUNTERVALUE to fill.
        let value_status = unsafe {
            PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, &mut value_type, &mut value)
        };
        if value_status != 0 || value.CStatus != 0 {
            return None;
        }

        // SAFETY: PDH_FMT_DOUBLE was requested above, so the union's
        // doubleValue member is the one PDH initialized.
        Some(clamp_pct(unsafe { value.Anonymous.doubleValue }))
    }
}

impl Drop for PdhCpuCounter {
    fn drop(&mut self) {
        // SAFETY: self.query was opened in open() and is still open (sample()
        // only reads from it); Drop runs exactly once, so the query is closed
        // exactly once.
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

fn read_system_times() -> Option<SystemTimes> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all three arguments are pointers to live, writable FILETIME
    // locals owned by this stack frame.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    Some(SystemTimes {
        idle_100ns: filetime_to_u64(idle),
        total_100ns: filetime_to_u64(kernel).saturating_add(filetime_to_u64(user)),
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

fn clamp_pct(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
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
    // SAFETY: root is one of the predefined HKEY_* constants (never freed);
    // key_path comes from wide_null and is therefore NUL-terminated UTF-16, as
    // RegOpenKeyExW requires; &mut key is a live out-parameter.
    let open_status = unsafe { RegOpenKeyExW(root, key_path.as_ptr(), 0, KEY_READ, &mut key) };
    if open_status != 0 {
        return None;
    }

    let result = query_registry_string(key, &value_name);
    // SAFETY: key was opened successfully above and this is its single close.
    unsafe {
        RegCloseKey(key);
    }
    result
}

fn query_registry_string(key: HKEY, value_name: &[u16]) -> Option<String> {
    let mut value_type = 0;
    let mut byte_len = 0;
    // SAFETY: key is open; value_name is NUL-terminated UTF-16 from wide_null.
    // Per MSDN, lpData may be null on this size-query pass, which requests the
    // value's byte length without copying; value_type and byte_len are live
    // writable out-parameters.
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
    // SAFETY: key is still open; value_name is NUL-terminated UTF-16; bytes is
    // a live buffer whose capacity is exactly the byte_len the preceding
    // size-query pass reported, and byte_len is re-passed by writable
    // reference as the in/out length parameter.
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
    fn cpu_snapshot_returns_basic_identity() {
        let snapshot = cpu_snapshot().expect("Windows CPU snapshot should include basic identity");
        assert!(!snapshot.name.trim().is_empty());
    }
}
