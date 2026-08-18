use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

use super::super::CpuSnapshot;

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
        /// 对应 sys/time.h 的 timeval（tv_sec: long + tv_usec: int32 + 4 字节
        /// padding，共 16 字节）；注意不是 mach 的 time_value_t（i32+i32 共 8 字节）。
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

    extern "C" {
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

/// CPU 名称与逻辑核数在进程生命周期内不变；brand_string 只需 sysctl 一次。
static CPU_IDENTITY: OnceLock<(String, u32)> = OnceLock::new();

/// host 端口进程生命周期内稳定。mach_host_self() 每次调用都会给同一端口 +1 个
/// send right 引用（实测 1Hz 采样约 18 小时饱和 65535 上限），只取一次并持有
/// 进程级引用，避免按调用泄漏。
static HOST_PORT: OnceLock<ffi::MachPort> = OnceLock::new();

fn host_port() -> ffi::MachPort {
    *HOST_PORT.get_or_init(|| unsafe { ffi::mach_host_self() })
}

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Default)]
struct CpuSampleState {
    system: Option<SystemTicks>,
    process: Option<ProcessTicks>,
}

#[derive(Debug, Clone, Copy)]
struct SystemTicks {
    busy: u64,
    idle: u64,
}

#[derive(Debug, Clone, Copy)]
struct ProcessTicks {
    cpu_secs: f64,
    sampled_at: Instant,
}

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let (name, logical_processors) = cpu_identity();
    let system = read_system_ticks();
    let process = read_process_ticks();
    let state = CPU_SAMPLE_STATE.get_or_init(|| Mutex::new(CpuSampleState::default()));
    let mut state = state.lock();

    let total_usage_pct = match (state.system, system) {
        (Some(prev), Some(current)) => system_usage_pct(prev, current),
        _ => None,
    };
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

fn cpu_identity() -> (String, u32) {
    CPU_IDENTITY
        .get_or_init(|| {
            let logical_processors = std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(0);
            let name = read_brand_string().unwrap_or_else(|| "CPU".to_string());
            (name, logical_processors)
        })
        .clone()
}

fn read_brand_string() -> Option<String> {
    // machdep.cpu.brand_string 在 Intel 与 Apple Silicon 上都存在
    // （后者返回 "Apple M1"/"Apple M2 Pro" 等）；sysctlbyname(3) 是 libSystem
    // 纯 C 符号，直连免掉 spawn 子进程。
    let name = b"machdep.cpu.brand_string\0";
    let mut buf = [0u8; 128];
    let mut len = buf.len();
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
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn read_system_ticks() -> Option<SystemTicks> {
    let mut info = ffi::HostCpuLoadInfo::default();
    let mut count: u32 = info.cpu_ticks.len() as u32;
    let status = unsafe {
        ffi::host_statistics64(
            host_port(),
            HOST_CPU_LOAD_INFO,
            &mut info as *mut ffi::HostCpuLoadInfo as *mut i32,
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

fn read_process_ticks() -> Option<ProcessTicks> {
    let mut usage = ffi::Rusage::default();
    let status = unsafe { ffi::getrusage(RUSAGE_SELF, &mut usage) };
    if status != 0 {
        return None;
    }
    Some(ProcessTicks {
        cpu_secs: timevalue_secs(usage.ru_utime) + timevalue_secs(usage.ru_stime),
        sampled_at: Instant::now(),
    })
}

fn timevalue_secs(value: ffi::Timeval) -> f64 {
    value.seconds as f64 + value.microseconds as f64 / 1_000_000.0
}

fn system_usage_pct(prev: SystemTicks, current: SystemTicks) -> Option<f64> {
    let busy_delta = current.busy.checked_sub(prev.busy)?;
    let idle_delta = current.idle.saturating_sub(prev.idle);
    let total_delta = busy_delta.checked_add(idle_delta)?;
    if total_delta == 0 {
        return None;
    }
    Some(clamp_pct(busy_delta as f64 * 100.0 / total_delta as f64))
}

fn process_usage_pct(
    prev: ProcessTicks,
    current: ProcessTicks,
    logical_processors: u32,
) -> Option<f64> {
    if logical_processors == 0 {
        return None;
    }
    // f64 无 checked_sub：计数器回绕时差值为负，直接按无进展处理。
    let cpu_delta = current.cpu_secs - prev.cpu_secs;
    if cpu_delta <= 0.0 {
        return None;
    }
    let elapsed_secs = current
        .sampled_at
        .checked_duration_since(prev.sampled_at)?
        .as_secs_f64();
    if elapsed_secs <= 0.0 {
        return None;
    }
    Some(clamp_pct(
        cpu_delta * 100.0 / elapsed_secs / logical_processors as f64,
    ))
}

fn clamp_pct(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rusage_layout_matches_kernel_struct() {
        // 布局防线：struct rusage 在 darwin 64 位上恒为 144 字节（2 个 timeval
        // 各 16 + 14 个 long 各 8）。声明错小会让内核写越界到调用方栈上。
        assert_eq!(std::mem::size_of::<ffi::Rusage>(), 144);
        assert_eq!(std::mem::offset_of!(ffi::Rusage, ru_utime), 0);
        assert_eq!(std::mem::offset_of!(ffi::Rusage, ru_stime), 16);
        assert_eq!(std::mem::size_of::<ffi::Timeval>(), 16);
    }

    #[test]
    fn timevalue_converts_to_seconds() {
        let value = ffi::Timeval {
            seconds: 12,
            microseconds: 500_000,
        };
        assert_eq!(timevalue_secs(value), 12.5);
    }

    #[test]
    fn clamp_usage_pct_clamps_range() {
        assert_eq!(clamp_pct(-1.0), 0.0);
        assert_eq!(clamp_pct(42.5), 42.5);
        assert_eq!(clamp_pct(120.0), 100.0);
        assert_eq!(clamp_pct(f64::NAN), 0.0);
    }

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
    fn system_usage_from_deltas_returns_expected() {
        let prev = SystemTicks {
            busy: 100,
            idle: 400,
        };
        let current = SystemTicks {
            busy: 200,
            idle: 500,
        };
        assert_eq!(system_usage_pct(prev, current), Some(50.0));
    }

    #[test]
    fn system_usage_returns_none_without_elapsed_ticks() {
        let sample = SystemTicks {
            busy: 100,
            idle: 400,
        };
        assert_eq!(system_usage_pct(sample, sample), None);
    }

    #[test]
    fn system_usage_clamps_wraparound() {
        // 计数器回绕（busy 变小）时 checked_sub 返回 None，不产生负值。
        let prev = SystemTicks {
            busy: 500,
            idle: 100,
        };
        let current = SystemTicks {
            busy: 100,
            idle: 600,
        };
        assert_eq!(system_usage_pct(prev, current), None);
    }

    #[test]
    fn process_usage_accounts_for_logical_processors() {
        let now = Instant::now();
        let prev = ProcessTicks {
            cpu_secs: 0.0,
            sampled_at: now,
        };
        let current = ProcessTicks {
            cpu_secs: 1.0,
            sampled_at: now + Duration::from_secs(1),
        };
        assert_eq!(process_usage_pct(prev, current, 4), Some(25.0));
    }

    #[test]
    fn process_usage_returns_none_without_cpu_progress() {
        let now = Instant::now();
        let sample = ProcessTicks {
            cpu_secs: 1.0,
            sampled_at: now,
        };
        assert_eq!(process_usage_pct(sample, sample, 4), None);
    }

    #[test]
    fn cpu_snapshot_returns_basic_identity() {
        // 集成测试（macOS host）：host_statistics64/getrusage 在普通进程即可调用，
        // 快照始终携带身份信息；首次调用无使用率（需要两次采样）。
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
            "name={} cores={} total={:?} process={:?}",
            s.name, s.logical_processors, s.total_usage_pct, s.process_usage_pct
        );
        let v = s.total_usage_pct.expect("第二次采样应有系统使用率");
        assert!((0.0..=100.0).contains(&v), "使用率应在 0-100，实际 {v}");
    }
}
