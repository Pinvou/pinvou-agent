use std::sync::OnceLock;

use parking_lot::Mutex;

use super::super::CpuSnapshot;

static CPU_SAMPLE_STATE: OnceLock<Mutex<CpuSampleState>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTicks {
    total: u64,
    idle: u64,
}

#[derive(Debug, Default)]
struct CpuSampleState {
    previous: Option<CpuTicks>,
}

impl CpuSampleState {
    fn observe(&mut self, current: CpuTicks) -> Option<f64> {
        let usage = self
            .previous
            .and_then(|previous| usage_pct(previous, current));
        self.previous = Some(current);
        usage
    }
}

/// Read aggregate Linux CPU utilization without spawning an external process.
///
/// `/proc/stat` contains cumulative counters, so the first successful sample has
/// no utilization value. The process-local previous sample is retained for the
/// next call. A malformed read or a counter reset degrades to `None`.
pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let current = parse_proc_stat(&stat)?;
    let state = CPU_SAMPLE_STATE.get_or_init(|| Mutex::new(CpuSampleState::default()));
    let mut state = state.lock();
    let total_usage_pct = state.observe(current);

    let name = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|cpuinfo| parse_cpu_name(&cpuinfo))
        .unwrap_or_else(|| "Linux CPU".to_string());

    Some(CpuSnapshot {
        name,
        total_usage_pct,
        process_usage_pct: None,
        logical_processors: logical_processor_count(),
    })
}

fn parse_proc_stat(text: &str) -> Option<CpuTicks> {
    let line = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some("cpu"))?;
    let mut fields = line.split_whitespace();
    fields.next()?;

    // guest and guest_nice are already included in user and nice respectively,
    // so only the first eight counters participate in the aggregate total.
    let values = fields
        .take(8)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }

    let value = |index: usize| values.get(index).copied().unwrap_or(0);
    let idle = value(3).checked_add(value(4))?;
    let busy = [value(0), value(1), value(2), value(5), value(6), value(7)]
        .into_iter()
        .try_fold(0_u64, u64::checked_add)?;

    Some(CpuTicks {
        total: idle.checked_add(busy)?,
        idle,
    })
}

fn usage_pct(previous: CpuTicks, current: CpuTicks) -> Option<f64> {
    let total_delta = current.total.checked_sub(previous.total)?;
    let idle_delta = current.idle.checked_sub(previous.idle)?;
    if total_delta == 0 || idle_delta > total_delta {
        return None;
    }

    let busy_delta = total_delta - idle_delta;
    Some((busy_delta as f64 * 100.0 / total_delta as f64).clamp(0.0, 100.0))
}

fn parse_cpu_name(text: &str) -> Option<String> {
    ["model name", "hardware", "processor"]
        .into_iter()
        .find_map(|wanted| {
            text.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                (key.trim().eq_ignore_ascii_case(wanted))
                    .then(|| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
        })
}

fn logical_processor_count() -> u32 {
    std::thread::available_parallelism()
        .map(|count| count.get() as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aggregate_cpu_ticks_without_double_counting_guest() {
        let ticks =
            parse_proc_stat("cpu  100 20 30 400 50 6 7 8 90 10\ncpu0 1 2 3 4 5 6 7 8 9 10\n")
                .expect("aggregate ticks should parse");

        assert_eq!(ticks.idle, 450);
        assert_eq!(ticks.total, 621);
    }

    #[test]
    fn computes_usage_from_tick_deltas() {
        let previous = CpuTicks {
            total: 1_000,
            idle: 700,
        };
        let current = CpuTicks {
            total: 1_200,
            idle: 750,
        };

        assert_eq!(usage_pct(previous, current), Some(75.0));
    }

    #[test]
    fn process_local_state_returns_none_for_first_sample() {
        let mut state = CpuSampleState::default();
        assert_eq!(
            state.observe(CpuTicks {
                total: 1_000,
                idle: 700,
            }),
            None
        );
        assert_eq!(
            state.observe(CpuTicks {
                total: 1_200,
                idle: 750,
            }),
            Some(75.0)
        );
    }

    #[test]
    fn counter_reset_and_malformed_stat_degrade_to_none() {
        let previous = CpuTicks {
            total: 1_000,
            idle: 700,
        };
        let reset = CpuTicks {
            total: 900,
            idle: 600,
        };

        assert_eq!(usage_pct(previous, reset), None);
        assert!(parse_proc_stat("cpu  1 nope 3 4\n").is_none());
        assert!(parse_proc_stat("intr 1 2 3\n").is_none());
    }

    #[test]
    fn parses_cpu_name_with_safe_fallback_order() {
        let cpuinfo = "processor : 0\nmodel name : Intel(R) Core(TM) Ultra\n";
        assert_eq!(
            parse_cpu_name(cpuinfo).as_deref(),
            Some("Intel(R) Core(TM) Ultra")
        );
        assert_eq!(parse_cpu_name("processor : 0\n").as_deref(), Some("0"));
        assert!(parse_cpu_name("vendor_id : GenuineIntel\n").is_none());
    }
}
