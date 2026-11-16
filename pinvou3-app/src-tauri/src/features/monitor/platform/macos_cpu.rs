use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

use super::super::CpuSnapshot;
use super::super::cpu_math::{
    ProcessDeltaState, ProcessSample, SystemDeltaState, SystemTicks, logical_processors,
};

// mach/BSD 原语直接走 libSystem FFI（Rust 对 macOS 默认链接），避免为一个
// 采样函数引入 libc/mach2 依赖——与 CodeWhale tui 的 CoreGraphics 直连风格一致
// （crates/tui/src/tui/display_refresh.rs）。
mod ffi {
    // mach 端口与 kern_return_t 的底层整数类型（darwin: natural_t/integer_t）。
    pub type MachPort = u32;

    #[repr(C)]
    #[derive(Default)]
    pub struct HostCpuLoadInfo {
        /// 聚合节拍数，按 CPU_STATE_* 索引：user/system/idle/nice。
        pub cpu_ticks: [u32; 4],
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct Timeval {
        /// Mirrors sys/time.h `struct timeval` (`tv_sec: long` + `tv_usec:
        /// int32` + 4 bytes of padding, 16 bytes total) — not mach's
        /// `time_value_t` (`i32` + `i32`, 8 bytes).
        pub seconds: i64,
        pub microseconds: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct Rusage {
        pub ru_utime: Timeval,
        pub ru_stime: Timeval,
        pub ru_maxrss: i64,
        pub ru_ixrss: i64,
        pub ru_idrss: i64,
        pub ru_isrss: i64,
        pub ru_minflt: i64,
        pub ru_majflt: i64,
        pub ru_nswap: i64,
        pub ru_inblock: i64,
        pub ru_oublock: i64,
        pub ru_msgsnd: i64,
        pub ru_msgrcv: i64,
        pub ru_nsignals: i64,
        pub ru_nvcsw: i64,
        pub ru_nivcsw: i64,
    }

    unsafe extern "C" {
        pub fn mach_host_self() -> MachPort;
        pub fn host_statistics64(
            host: MachPort,
            flavor: i32,
            host_info64_out: *mut i32,
            host_info64_out_cnt: *mut u32,
        ) -> i32;
        pub fn getrusage(who: i32, r_usage: *mut Rusage) -> i32;
        pub fn sysctlbyname(
            name: *const i8,
            oldp: *mut core::ffi::c_void,
            oldlenp: *mut usize,
            newp: *mut core::ffi::c_void,
            newlen: usize,
        ) -> i32;
    }
}

const HOST_CPU_LOAD_INFO: i32 = 3;
const CPU_STATE_USER: usize = 0;
const CPU_STATE_SYSTEM: usize = 1;
const CPU_STATE_IDLE: usize = 2;
const CPU_STATE_NICE: usize = 3;
const RUSAGE_SELF: i32 = 0;

/// The CPU name is stable for the process lifetime; brand_string is only
/// sysctl'd once.
static CPU_NAME: OnceLock<String> = OnceLock::new();

/// host 端口进程生命周期内稳定。mach_host_self() 每次调用都会给同一端口 +1 个
/// send right 引用（实测 1Hz 采样约 18 小时饱和 65535 上限），只取一次并持有
/// 进程级引用，避免按调用泄漏。
static HOST_PORT: OnceLock<ffi::MachPort> = OnceLock::new();

fn host_port() -> ffi::MachPort {
    // SAFETY: mach_host_self is a pure query function with no
    // preconditions; the returned send right is a bare u32 port name (not a
    // pointer), held for the process lifetime by a process-wide OnceLock and
    // never released, so there is no aliasing or dangling.
    *HOST_PORT.get_or_init(|| unsafe { ffi::mach_host_self() })
}

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Default)]
struct CpuSampleState {
    system: SystemDeltaState,
    process: ProcessDeltaState,
}

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let name = cpu_name();
    let logical_processors = logical_processors();
    let current = read_system_ticks();
    let process = read_process_ticks();
    let state = CPU_SAMPLE_STATE.get_or_init(|| Mutex::new(CpuSampleState::default()));
    let mut state = state.lock();

    Some(CpuSnapshot {
        name,
        total_usage_pct: state.system.absorb(current),
        process_usage_pct: state.process.absorb(process, logical_processors),
        logical_processors,
    })
}

fn cpu_name() -> String {
    CPU_NAME
        .get_or_init(|| read_brand_string().unwrap_or_else(|| "CPU".to_string()))
        .clone()
}

fn read_brand_string() -> Option<String> {
    // machdep.cpu.brand_string 在 Intel 与 Apple Silicon 上都存在
    // （后者返回 "Apple M1"/"Apple M2 Pro" 等）；sysctlbyname(3) 是 libSystem
    // 纯 C 符号，直连免掉 spawn 子进程。
    let name = b"machdep.cpu.brand_string\0";
    let mut buf = [0u8; 128];
    let mut len = buf.len();
    // SAFETY: name is a NUL-terminated literal; sysctlbyname performs a
    // read-only query (newp=NULL, newlen=0, no kernel state writes), oldp
    // points to a 128-byte stack buffer and the length declared via oldlenp
    // matches that buffer; the kernel updates len to the amount actually
    // written and does not write out of bounds.
    let status = unsafe {
        ffi::sysctlbyname(
            name.as_ptr().cast(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return None;
    }
    let name = std::ffi::CStr::from_bytes_until_nul(&buf)
        .ok()?
        .to_str()
        .ok()?
        .trim()
        .to_string();
    if name.is_empty() { None } else { Some(name) }
}

fn read_system_ticks() -> Option<SystemTicks> {
    let mut info = ffi::HostCpuLoadInfo::default();
    let mut count: u32 = info.cpu_ticks.len() as u32;
    // SAFETY: host_statistics64 writes at most `count` ints under
    // HOST_CPU_LOAD_INFO semantics (4 here, matching the cpu_ticks capacity)
    // into a sufficiently sized stack structure; host_port is this process's
    // send right from mach_host_self, valid for this flavor; the kernel
    // updates count to the amount actually written, never out of bounds, and
    // failure only returns a non-zero kern_return_t.
    let status = unsafe {
        ffi::host_statistics64(
            host_port(),
            HOST_CPU_LOAD_INFO,
            (&mut info as *mut ffi::HostCpuLoadInfo).cast::<i32>(),
            &mut count,
        )
    };
    if status != 0 {
        return None;
    }
    let ticks = info.cpu_ticks;
    let busy = ticks[CPU_STATE_USER] as u64
        + ticks[CPU_STATE_SYSTEM] as u64
        + ticks[CPU_STATE_NICE] as u64;
    Some(SystemTicks {
        busy,
        idle: ticks[CPU_STATE_IDLE] as u64,
    })
}

fn read_process_ticks() -> Option<ProcessSample> {
    let mut usage = ffi::Rusage::default();
    // SAFETY: getrusage only writes this process's statistics into usage
    // under RUSAGE_SELF semantics; Rusage matches the darwin 64-bit
    // struct rusage layout (the rusage_layout_matches_kernel_struct test
    // pins the 144-byte layout), the pointer targets a sufficiently sized
    // stack structure, and failure only returns -1.
    let status = unsafe { ffi::getrusage(RUSAGE_SELF, &mut usage) };
    if status != 0 {
        return None;
    }
    Some(ProcessSample {
        cpu_secs: process_secs(&usage),
        sampled_at: Instant::now(),
    })
}

/// The sampled quantity: user + system time in seconds for one rusage
/// reading. Extracted from the read path so the sum itself (not just the
/// per-field conversion) has a pin.
fn process_secs(usage: &ffi::Rusage) -> f64 {
    timevalue_secs(usage.ru_utime) + timevalue_secs(usage.ru_stime)
}

fn timevalue_secs(value: ffi::Timeval) -> f64 {
    value.seconds as f64 + value.microseconds as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_brand_string_matches_sysctl_output() {
        // 真机冒烟（macOS host）：直连 sysctlbyname 与 /usr/sbin/sysctl 输出一致。
        let expected = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(expected) = expected {
            assert_eq!(read_brand_string().as_deref(), Some(expected.as_str()));
        }
    }

    #[test]
    fn rusage_layout_matches_kernel_struct() {
        // Layout tripwire: `struct rusage` is exactly 144 bytes on 64-bit
        // darwin (two 16-byte timevals + 14 8-byte longs); a too-small
        // declaration would let the kernel write past the caller's stack.
        // The Timeval offsets additionally pin the `tv_sec` field width
        // (8 bytes) and the timeval split point.
        assert_eq!(std::mem::size_of::<ffi::Rusage>(), 144);
        assert_eq!(std::mem::offset_of!(ffi::Rusage, ru_utime), 0);
        assert_eq!(std::mem::offset_of!(ffi::Rusage, ru_stime), 16);
        assert_eq!(std::mem::size_of::<ffi::Timeval>(), 16);
        assert_eq!(std::mem::offset_of!(ffi::Timeval, seconds), 0);
        assert_eq!(std::mem::offset_of!(ffi::Timeval, microseconds), 8);
    }

    #[test]
    fn timevalue_converts_to_seconds() {
        let value = ffi::Timeval {
            seconds: 3,
            microseconds: 500_000,
        };
        assert_eq!(timevalue_secs(value), 3.5);
    }

    #[test]
    fn process_secs_sums_user_and_system_time() {
        // Pins the sum site: dropping ru_stime (or swapping the fields)
        // changes this answer, which the per-field conversion test alone
        // cannot see.
        let usage = ffi::Rusage {
            ru_utime: ffi::Timeval {
                seconds: 3,
                microseconds: 0,
            },
            ru_stime: ffi::Timeval {
                seconds: 0,
                microseconds: 500_000,
            },
            ..Default::default()
        };
        assert_eq!(process_secs(&usage), 3.5);
    }

    #[test]
    fn cpu_snapshot_returns_basic_identity() {
        // Integration test (macOS host): host_statistics64/getrusage are
        // callable from an ordinary process, the snapshot always carries
        // identity; the first call has no usage (two samples are needed).
        let snapshot = cpu_snapshot().expect("macOS CPU snapshot should include basic identity");
        assert!(!snapshot.name.trim().is_empty());
        assert!(snapshot.logical_processors > 0);
    }
}

#[cfg(test)]
mod smoke_tests {
    use super::*;

    /// 真机冒烟(#[ignore]):确认 FFI 采样链在真机走通且第二次采样产出使用率。
    /// 跑法:
    ///   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- \
    ///     --ignored --nocapture macos_cpu
    #[test]
    #[ignore]
    fn second_sample_yields_real_usage() {
        let _ = cpu_snapshot();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let s = cpu_snapshot().expect("snapshot");
        println!(
            "name={} logical={} total={:?} process={:?}",
            s.name, s.logical_processors, s.total_usage_pct, s.process_usage_pct
        );
        let v = s.total_usage_pct.expect("第二次采样应有系统使用率");
        assert!((0.0..=100.0).contains(&v), "使用率应在 0-100，实际 {v}");
        let p = s
            .process_usage_pct
            .expect("second sample should yield process usage");
        assert!(
            (0.0..=100.0).contains(&p),
            "process usage should be 0-100, got {p}"
        );
    }
}
