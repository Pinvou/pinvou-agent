use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

use super::super::CpuSnapshot;
use super::super::cpu_math::{SystemDeltaState, SystemTicks, clamp_pct};

/// `/proc` 中 CPU 时间的基本频率。内核 USER_HZ 在 Tauri 支持的架构
/// （x86_64/aarch64/riscv64 等）上都恒为 100，唯一例外是已废弃的 alpha
/// （1024，非 Tauri 目标）；Rust std 无 sysconf 接口，按 100 换算与 procps/top 一致。
const USER_HZ: f64 = 100.0;

/// CPU name and parallelism are stable for the process lifetime; sampling at
/// 1 Hz need not re-read /proc for them.
static CPU_IDENTITY: OnceLock<(String, u32)> = OnceLock::new();

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Default)]
struct CpuSampleState {
    system: SystemDeltaState,
    process: Option<ProcessTicks>,
}

#[derive(Debug, Clone, Copy)]
struct ProcessTicks {
    cpu_secs: f64,
    sampled_at: Instant,
}

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let (name, logical_processors) = cpu_identity();
    let current = read_system_ticks();
    let process = read_process_ticks();
    let state = CPU_SAMPLE_STATE.get_or_init(|| Mutex::new(CpuSampleState::default()));
    let mut state = state.lock();

    let total_usage_pct = state.system.absorb(current);
    let process_usage_pct = match (state.process, process) {
        (Some(prev), Some(current)) => process_usage_pct(prev, current, logical_processors),
        _ => None,
    };
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
            let name = read_cpuinfo_model_name().unwrap_or_else(|| "CPU".to_string());
            (name, logical_processors)
        })
        .clone()
}

fn read_system_ticks() -> Option<SystemTicks> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    parse_proc_stat_total(line)
}

fn read_process_ticks() -> Option<ProcessTicks> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    let ticks = parse_proc_self_stat_cpu_ticks(&text)?;
    Some(ProcessTicks {
        cpu_secs: ticks as f64 / USER_HZ,
        sampled_at: Instant::now(),
    })
}

/// 解析 `/proc/stat` 首行 `cpu  user nice system idle iowait irq softirq steal ...`。
/// guest 时间已由内核计入 user，不重复累加。
fn parse_proc_stat_total(line: &str) -> Option<SystemTicks> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.first() != Some(&"cpu") || fields.len() < 5 {
        return None;
    }
    let values: Option<Vec<u64>> = fields[1..].iter().map(|f| f.parse::<u64>().ok()).collect();
    let values = values?;
    let user = values.first().copied().unwrap_or(0);
    let nice = values.get(1).copied().unwrap_or(0);
    let system = values.get(2).copied().unwrap_or(0);
    let idle = values.get(3).copied().unwrap_or(0);
    let iowait = values.get(4).copied().unwrap_or(0);
    let irq = values.get(5).copied().unwrap_or(0);
    let softirq = values.get(6).copied().unwrap_or(0);
    let steal = values.get(7).copied().unwrap_or(0);
    let busy = user + nice + system + irq + softirq + steal;
    Some(SystemTicks {
        busy,
        idle: idle + iowait,
    })
}

/// Parse the utime+stime tick counts from `/proc/self/stat`. The comm field
/// (2nd field) may contain spaces and parentheses, so fields must be located
/// after the LAST `)`; in the remaining token sequence, tokens 12/13
/// correspond to full-field utime(14)/stime(15).
fn parse_proc_self_stat_cpu_ticks(text: &str) -> Option<u64> {
    let after_comm = text.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(utime + stime)
}

fn read_cpuinfo_model_name() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    parse_cpuinfo_model_name(&text)
}

fn parse_cpuinfo_model_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| {
            let (key, value) = l.split_once(':')?;
            if key.trim() == "model name" {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
}

fn process_usage_pct(
    prev: ProcessTicks,
    current: ProcessTicks,
    logical_processors: u32,
) -> Option<f64> {
    if logical_processors == 0 {
        return None;
    }
    // f64 has no checked_sub: a negative delta means the counter moved
    // backwards (wraparound); the pair is unusable, not "idle". An exact
    // zero delta is a real idle interval and reports Some(0.0) — the same
    // convention as the system gauge and the Windows sampler.
    let cpu_delta = current.cpu_secs - prev.cpu_secs;
    if cpu_delta < 0.0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_stat_aggregates_busy_and_idle() {
        // user nice system idle iowait irq softirq steal guest guest_nice
        // guest/guest_nice 填非零值：内核已把 guest 计入 user，这里锁死"busy
        // 不得重复累加 guest"的语义（回归成 += guest 会算出 380）。
        let ticks = parse_proc_stat_total("cpu  100 10 200 500 50 5 35 20 7 3").unwrap();
        // busy = 100+10+200+5+35+20 = 370；idle = 500+50 = 550
        assert_eq!(ticks.busy, 370);
        assert_eq!(ticks.idle, 550);
    }

    #[test]
    fn parse_proc_stat_rejects_non_cpu_lines() {
        assert!(parse_proc_stat_total("cpu0 1 2 3 4 5 6 7 8").is_none());
        assert!(parse_proc_stat_total("cpu 1 2 3").is_none());
        assert!(parse_proc_stat_total("").is_none());
    }

    #[test]
    fn parse_proc_stat_tolerates_short_suffixes() {
        // 只到 idle 的最小行（4 值）也应可解析，缺失列按 0。
        let ticks = parse_proc_stat_total("cpu  10 0 20 70").unwrap();
        assert_eq!(ticks.busy, 30);
        assert_eq!(ticks.idle, 70);
    }

    #[test]
    fn parse_proc_self_stat_handles_spaced_comm() {
        // comm may contain spaces/parens: fields must be located after the
        // LAST ')'. Tokens 11/12 after ')' are utime(14)/stime(15): the
        // fixture below is 100/200 → total 300.
        let text = "12345 (python3 -m t) R 1 2 3 4 5 6 7 8 9 10 100 200 0 0 \
                    20 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_proc_self_stat_cpu_ticks(text), Some(300));
        // A real /proc/self/stat snapshot (comm without spaces) parses via
        // the last ')' the same way.
        let text = "4242 (pinvou3-tauri) S 4241 4242 4242 0 -1 4194304 12345 0 0 0 \
                    9876 5432 0 0 20 0 12 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_proc_self_stat_cpu_ticks(text), Some(9876 + 5432));
    }

    #[test]
    fn parse_proc_self_stat_comm_with_parens_uses_the_last_paren() {
        // The advertised comm edge: ')' inside comm. Only rsplit_once (last
        // ')') lands on utime/stime; splitting at the FIRST ')' would read
        // 10/100 instead of 100/200, so this fixture bites that regression.
        let text = "999 (a)b)c) R 1 2 3 4 5 6 7 8 9 10 100 200 0 0 \
                    20 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_proc_self_stat_cpu_ticks(text), Some(300));
    }

    #[test]
    fn parse_proc_self_stat_rejects_truncated_input() {
        // Fewer than 12 tokens after ')': no utime → None, never a panic.
        let text = "1 (sh) S 0 1 1 0 -1 4194560";
        assert!(parse_proc_self_stat_cpu_ticks(text).is_none());
    }

    #[test]
    fn parse_proc_self_stat_rejects_missing_stime() {
        // The 12/13-token boundary: with utime reachable but stime missing
        // the answer is None (not utime alone); one more token flips it.
        let twelve = "1 (sh) S 0 1 1 0 -1 4194560 7 0 0 0 50";
        assert!(parse_proc_self_stat_cpu_ticks(twelve).is_none());
        let thirteen = "1 (sh) S 0 1 1 0 -1 4194560 7 0 0 0 50 0";
        assert_eq!(parse_proc_self_stat_cpu_ticks(thirteen), Some(50));
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
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // 1 CPU-second over 1 second / 4 cores = 25%
        assert_eq!(process_usage_pct(prev, current, 4), Some(25.0));
    }

    #[test]
    fn process_usage_reports_zero_for_idle_interval() {
        let now = Instant::now();
        let prev = ProcessTicks {
            cpu_secs: 2.0,
            sampled_at: now,
        };
        let current = ProcessTicks {
            cpu_secs: 2.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // Same convention as the system gauge: a fully idle interval is a
        // measured 0%, not missing data (Windows sampler parity).
        assert_eq!(process_usage_pct(prev, current, 4), Some(0.0));
    }

    #[test]
    fn process_usage_returns_none_without_cpu_progress() {
        let now = Instant::now();
        let sample = ProcessTicks {
            cpu_secs: 1.0,
            sampled_at: now,
        };
        // Identical samples mean zero elapsed wall time, which is unusable
        // regardless of the CPU delta.
        assert_eq!(process_usage_pct(sample, sample, 4), None);
    }

    #[test]
    fn process_usage_returns_none_without_parallelism() {
        let now = Instant::now();
        let prev = ProcessTicks {
            cpu_secs: 0.0,
            sampled_at: now,
        };
        let current = ProcessTicks {
            cpu_secs: 1.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // A probe-failed denominator (0) cannot normalize a percentage.
        assert_eq!(process_usage_pct(prev, current, 0), None);
    }

    #[test]
    fn parse_cpuinfo_extracts_model_name() {
        let text =
            "processor\t: 0\nvendor_id\t: GenuineIntel\nmodel name\t: AMD Ryzen 9\nflags\t: fpu\n";
        assert_eq!(
            parse_cpuinfo_model_name(text),
            Some("AMD Ryzen 9".to_string())
        );
        // ARM 设备常无 model name 行 → None（调用方回退 "CPU"）。
        assert_eq!(parse_cpuinfo_model_name("Processor: A53\n"), None);
    }

    #[test]
    fn cpu_snapshot_returns_basic_identity() {
        // 集成测试（Linux host）：快照始终携带身份信息；首次调用无使用率
        // （需要两次采样），但结构本身可用。
        let snapshot = cpu_snapshot().expect("Linux CPU snapshot should include basic identity");
        assert!(!snapshot.name.trim().is_empty());
        assert!(snapshot.logical_processors > 0);
    }
}

#[cfg(test)]
mod smoke_tests {
    use super::*;

    /// Real-host smoke (#[ignore]): the /proc sampling chain works end to
    /// end and the second sample yields usage. Run:
    ///   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib -- \
    ///     --ignored --nocapture linux_cpu
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
        let v = s
            .total_usage_pct
            .expect("second sample should yield system usage");
        assert!((0.0..=100.0).contains(&v), "usage should be 0-100, got {v}");
        let p = s
            .process_usage_pct
            .expect("second sample should yield process usage");
        assert!(
            (0.0..=100.0).contains(&p),
            "process usage should be 0-100, got {p}"
        );
    }
}
