use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;

use super::super::CpuSnapshot;

/// `/proc` 中 CPU 时间的基本频率。内核 USER_HZ 在 Tauri 支持的架构
/// （x86_64/aarch64/riscv64 等）上都恒为 100，唯一例外是已废弃的 alpha
/// （1024，非 Tauri 目标）；Rust std 无 sysconf 接口，按 100 换算与 procps/top 一致。
const USER_HZ: f64 = 100.0;

/// CPU 名称与逻辑核数在进程生命周期内不变，采样每秒一次也不必重复读 /proc。
static CPU_IDENTITY: OnceLock<(String, u32)> = OnceLock::new();

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Default)]
struct CpuSampleState {
    system: Option<SystemTicks>,
    process: Option<ProcessTicks>,
}

/// `/proc/stat` 首行的聚合节拍数。busy 含 user/nice/system/irq/softirq/steal
/// （steal 时间本机不可用，计入占用而非空闲）；idle 含 iowait（等 IO 视为空闲）。
/// 注意内核文档明言 iowait 本身口径不可靠（无任务可执行且有未完成 IO 才计入，
/// 等待中退出的任务会回退为 idle）；异常时本采样最多导致 total_delta 为 0 返回
/// None，不会产生失真数值。
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

/// 解析 `/proc/self/stat` 的 utime+stime 节拍数。comm 字段（第 2 列）可能含空格，
/// 必须从最后一个 `)` 之后取字段：剩余序列第 12/13 个 token 对应全字段序的
/// utime(14)/stime(15)。
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
        // comm 含空格/括号：必须从最后一个 ')' 之后定位字段。after 序列第 11/12
        // 个 token 是 utime(14)/stime(15)：下方 fixture 对应 100/200 → 合计 300。
        let text = "12345 (python3 -m t) R 1 2 3 4 5 6 7 8 9 10 100 200 0 0 \
                    20 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_proc_self_stat_cpu_ticks(text), Some(300));
        // 真实 /proc/self/stat 快照（comm 无空格）同样按最后 ')' 解析。
        let text = "4242 (pinvou3-tauri) S 4241 4242 4242 0 -1 4194304 12345 0 0 0 \
                    9876 5432 0 0 20 0 12 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_proc_self_stat_cpu_ticks(text), Some(9876 + 5432));
    }

    #[test]
    fn parse_proc_self_stat_rejects_truncated_input() {
        // ')' 之后字段不足 utime（12 个）时返回 None 而非 panic。
        let text = "1 (sh) S 0 1 1 0 -1 4194560";
        assert!(parse_proc_self_stat_cpu_ticks(text).is_none());
    }

    #[test]
    fn clamp_usage_pct_clamps_range() {
        assert_eq!(clamp_pct(-1.0), 0.0);
        assert_eq!(clamp_pct(42.5), 42.5);
        assert_eq!(clamp_pct(120.0), 100.0);
        assert_eq!(clamp_pct(f64::NAN), 0.0);
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
        // delta busy 100 / total 200 = 50%
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
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // 1 秒耗 1 CPU 秒 / 4 核 = 25%
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
