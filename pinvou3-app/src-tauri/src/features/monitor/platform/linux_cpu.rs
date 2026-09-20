use std::sync::OnceLock;

use parking_lot::Mutex;

use super::super::CpuSnapshot;
use super::super::cpu_math::{SystemDeltaState, SystemTicks};

/// `/proc` 中 CPU 时间的基本频率。内核 USER_HZ 在 Tauri 支持的架构
/// （x86_64/aarch64/riscv64 等）上都恒为 100，唯一例外是已废弃的 alpha
/// （1024，非 Tauri 目标）；Rust std 无 sysconf 接口，按 100 换算与 procps/top 一致。
const USER_HZ: f64 = 100.0;

/// CPU 名称在进程生命周期内不变，采样每秒一次也不必重复读 /proc。
static CPU_NAME: OnceLock<String> = OnceLock::new();

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
        .get_or_init(|| read_cpuinfo_model_name().unwrap_or_else(|| "CPU".to_string()))
        .clone()
}

fn read_system_ticks() -> Option<SystemTicks> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let line = text.lines().next()?;
    parse_proc_stat_total(line)
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
    }
}
