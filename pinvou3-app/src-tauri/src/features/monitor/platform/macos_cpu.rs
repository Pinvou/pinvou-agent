use std::sync::OnceLock;

use parking_lot::Mutex;

use super::super::CpuSnapshot;
use super::super::cpu_math::{SystemDeltaState, SystemTicks};

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

    unsafe extern "C" {
        pub fn mach_host_self() -> MachPort;
        pub fn host_statistics64(
            host: MachPort,
            flavor: i32,
            host_info64_out: *mut i32,
            host_info64_out_cnt: *mut u32,
        ) -> i32;
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

/// CPU 名称在进程生命周期内不变；brand_string 只需 sysctl 一次。
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

static CPU_SAMPLE_STATE: OnceLock<Mutex<SystemDeltaState>> = OnceLock::new();

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let name = cpu_name();
    let current = read_system_ticks();
    let state = CPU_SAMPLE_STATE.get_or_init(|| Mutex::new(SystemDeltaState::default()));
    let mut state = state.lock();

    Some(CpuSnapshot {
        name,
        total_usage_pct: state.absorb(current),
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
    fn cpu_snapshot_returns_basic_identity() {
        // 集成测试（macOS host）：host_statistics64 在普通进程即可调用，
        // 快照始终携带身份信息；首次调用无使用率（需要两次采样）。
        let snapshot = cpu_snapshot().expect("macOS CPU snapshot should include basic identity");
        assert!(!snapshot.name.trim().is_empty());
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
        println!("name={} total={:?}", s.name, s.total_usage_pct);
        let v = s.total_usage_pct.expect("第二次采样应有系统使用率");
        assert!((0.0..=100.0).contains(&v), "使用率应在 0-100，实际 {v}");
    }
}
