//! Pure tick math for CPU sampling — shared by the linux/macos samplers.
//!
//! FFI (`host_statistics64`) and `/proc` reads stay in the platform files
//! (`platform/{macos,linux}_cpu.rs`); this module keeps only the
//! platform-independent part: aggregated tick structures, the two-sample
//! delta → usage conversion, percentage clamping, and the cross-call delta
//! state machines. The Windows system side uses a different gauge shape
//! (`SystemTimes`: 100ns granularity, busy derived as total−idle), so the
//! system delta state machine is not shared; the process side
//! (`ProcessSample`/`ProcessDeltaState`) and `clamp_pct` are shared by all
//! three platforms.
// architecture-guard: allow-target-cfg -- the system delta-state half below is consumed exclusively by the cfg-gated linux/macos samplers (platform/{linux,macos}_cpu.rs); it is cfg-gated in this shared pure-math module so Windows lib builds carry no dead system-delta machinery, while clamp_pct and the process-delta half (ProcessSample/ProcessDeltaState/logical_processors) stay universal for all three samplers. The system delta path is exercised by this module's unit tests on linux/macos; the process path by the universal tests below.
use std::time::Instant;

/// `/proc/stat` 首行 / `host_statistics64` 的聚合节拍。busy 含 user/nice/system/
/// irq/softirq/steal(steal 时间本机不可用,计入占用而非空闲);idle 含 iowait
/// (等 IO 视为空闲)。注意内核文档明言 iowait 本身口径不可靠(无任务可执行且有
/// 未完成 IO 才计入,等待中退出的任务会回退为 idle);异常时本采样最多导致
/// total_delta 为 0 返回 None,不会产生失真数值。
#[derive(Debug, Clone, Copy)]
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) struct SystemTicks {
    pub busy: u64,
    pub idle: u64,
}

/// 两次采样的差分状态机(进程级单例由各平台文件持 Mutex 保护):
/// `absorb` 收下当前采样,返回"上一采样 → 当前采样"的系统使用率;
/// 首次调用无上一采样,返回 None。
#[derive(Debug, Default)]
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) struct SystemDeltaState {
    system: Option<SystemTicks>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl SystemDeltaState {
    pub(crate) fn absorb(&mut self, current: Option<SystemTicks>) -> Option<f64> {
        let usage_pct = match (self.system, current) {
            (Some(prev), Some(current)) => system_usage_pct(prev, current),
            _ => None,
        };
        if current.is_some() {
            self.system = current;
        }
        usage_pct
    }
}

/// 两次采样间的系统使用率百分比。计数器回绕(busy 变小)时 `checked_sub`
/// 返回 None,不产生负值。
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn system_usage_pct(prev: SystemTicks, current: SystemTicks) -> Option<f64> {
    let busy_delta = current.busy.checked_sub(prev.busy)?;
    let idle_delta = current.idle.saturating_sub(prev.idle);
    let total_delta = busy_delta.checked_add(idle_delta)?;
    if total_delta == 0 {
        return None;
    }
    Some(clamp_pct(busy_delta as f64 * 100.0 / total_delta as f64))
}

/// One sample of this process's own CPU time (user+system, aggregated
/// across all threads), normalized to seconds. Shared by all three platform
/// samplers; each platform's read path converts its raw counter into
/// `cpu_secs` before constructing this.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessSample {
    pub cpu_secs: f64,
    pub sampled_at: Instant,
}

/// Two-sample delta state for the process gauge, mirroring
/// [`SystemDeltaState`]: `absorb` returns the usage between the previous and
/// the current sample; the first call has no predecessor and returns `None`.
/// A failed probe (`None` argument) keeps the previous sample so the next
/// successful pair still spans the full elapsed interval. A rejected pair
/// (counter wraparound, or a non-monotonic timestamp from an out-of-order
/// caller that sampled before locking the shared state) never regresses the
/// baseline: the temporally newer sample is kept, so the next reading
/// measures a clean interval from the true latest observation.
#[derive(Debug, Default)]
pub(crate) struct ProcessDeltaState {
    process: Option<ProcessSample>,
}

impl ProcessDeltaState {
    pub(crate) fn absorb(
        &mut self,
        current: Option<ProcessSample>,
        logical_processors: u32,
    ) -> Option<f64> {
        let usage_pct = match (self.process, current) {
            (Some(prev), Some(current)) => {
                let usage_pct = process_usage_pct(prev, current, logical_processors);
                if usage_pct.is_none() {
                    // Rejected pair: keep the temporally newer sample as the
                    // baseline. A stale sample arriving out of order (each
                    // platform samples before locking this state) would
                    // otherwise overwrite the newer observation and make the
                    // next reading measure an interval that overlaps it.
                    if current.sampled_at < prev.sampled_at {
                        return None;
                    }
                }
                self.process = Some(current);
                usage_pct
            }
            (None, Some(current)) => {
                self.process = Some(current);
                None
            }
            _ => None,
        };
        usage_pct
    }
}

/// Per-process usage percentage between two samples, normalized per core.
/// A negative CPU delta means the raw counter moved backwards (wraparound):
/// the pair is unusable, so the answer is `None`. An exact zero delta is a
/// real idle interval and reports `Some(0.0)` — a measured reading, not
/// missing data. The remaining `None` causes are a probe-failed denominator
/// (`0` cores), zero elapsed wall time, and a non-monotonic sample pair
/// (`checked_duration_since` rejects a current timestamp before the
/// previous one — defensive; `Instant` is monotonic).
pub(crate) fn process_usage_pct(
    prev: ProcessSample,
    current: ProcessSample,
    logical_processors: u32,
) -> Option<f64> {
    if logical_processors == 0 {
        return None;
    }
    // f64 has no checked_sub: a negative delta means the counter moved
    // backwards (wraparound); the pair is unusable, not "idle".
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

/// Parallelism visible to this process (`std::thread::available_parallelism`:
/// respects the affinity mask and, on Linux, cgroup CPU quotas — it can be
/// lower than the host's logical core count). Re-read on every snapshot so
/// live affinity/quota changes stay in sync with the process gauge's
/// numerator; `0` when probing fails.
pub(crate) fn logical_processors() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(0)
}

/// 使用率钳到 0-100;非有限值(NaN 等)按 0 处理。
pub(crate) fn clamp_pct(value: f64) -> f64 {
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
    fn clamp_usage_pct_clamps_range() {
        assert_eq!(clamp_pct(-1.0), 0.0);
        assert_eq!(clamp_pct(42.5), 42.5);
        assert_eq!(clamp_pct(120.0), 100.0);
        assert_eq!(clamp_pct(f64::NAN), 0.0);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn system_usage_returns_none_without_elapsed_ticks() {
        let sample = SystemTicks {
            busy: 100,
            idle: 400,
        };
        assert_eq!(system_usage_pct(sample, sample), None);
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn system_usage_clamps_wraparound() {
        // 计数器回绕(busy 变小)时 checked_sub 返回 None,不产生负值。
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
        let prev = ProcessSample {
            cpu_secs: 0.0,
            sampled_at: now,
        };
        let current = ProcessSample {
            cpu_secs: 1.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // 1 CPU-second over 1 second / 4 cores = 25%
        assert_eq!(process_usage_pct(prev, current, 4), Some(25.0));
    }

    #[test]
    fn process_usage_reports_zero_for_idle_interval() {
        let now = Instant::now();
        let prev = ProcessSample {
            cpu_secs: 2.0,
            sampled_at: now,
        };
        let current = ProcessSample {
            cpu_secs: 2.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // A fully idle interval is a measured 0%, not missing data.
        assert_eq!(process_usage_pct(prev, current, 4), Some(0.0));
    }

    #[test]
    fn process_usage_returns_none_on_wraparound() {
        let now = Instant::now();
        let prev = ProcessSample {
            cpu_secs: 2.0,
            sampled_at: now,
        };
        let current = ProcessSample {
            cpu_secs: 1.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // The documented wraparound contract: a backwards counter is an
        // unusable pair, never a percentage.
        assert_eq!(process_usage_pct(prev, current, 4), None);
    }

    #[test]
    fn absorb_keeps_newer_baseline_for_a_stale_out_of_order_sample() {
        let now = Instant::now();
        let newer = ProcessSample {
            cpu_secs: 2.0,
            sampled_at: now,
        };
        let stale = ProcessSample {
            cpu_secs: 1.0,
            sampled_at: now - std::time::Duration::from_millis(500),
        };
        let mut state = ProcessDeltaState {
            process: Some(newer),
        };
        // A stale sample arriving out of order is a rejected pair; the
        // baseline must stay the newer observation.
        assert_eq!(state.absorb(Some(stale), 4), None);
        assert_eq!(state.process.as_ref().unwrap().cpu_secs, 2.0);
        // The next forward sample measures from the preserved baseline.
        let next = ProcessSample {
            cpu_secs: 2.5,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        assert_eq!(state.absorb(Some(next), 4), Some(12.5));
        assert_eq!(state.process.as_ref().unwrap().cpu_secs, 2.5);
    }

    #[test]
    fn absorb_installs_a_forward_sample_after_a_rejected_pair() {
        let now = Instant::now();
        let baseline = ProcessSample {
            cpu_secs: 2.0,
            sampled_at: now,
        };
        let wrapped = ProcessSample {
            cpu_secs: 1.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        let mut state = ProcessDeltaState {
            process: Some(baseline),
        };
        // A counter wraparound rejects the pair, but the wrapped sample is
        // temporally newer: it becomes the forward baseline for the next
        // reading.
        assert_eq!(state.absorb(Some(wrapped), 4), None);
        assert_eq!(state.process.as_ref().unwrap().cpu_secs, 1.0);
    }

    #[test]
    fn process_usage_returns_none_without_elapsed_wall_time() {
        let now = Instant::now();
        let sample = ProcessSample {
            cpu_secs: 1.0,
            sampled_at: now,
        };
        // Identical samples mean zero elapsed wall time, unusable regardless
        // of the CPU delta.
        assert_eq!(process_usage_pct(sample, sample, 4), None);
    }

    #[test]
    fn process_usage_returns_none_without_parallelism() {
        let now = Instant::now();
        let prev = ProcessSample {
            cpu_secs: 0.0,
            sampled_at: now,
        };
        let current = ProcessSample {
            cpu_secs: 1.0,
            sampled_at: now + std::time::Duration::from_secs(1),
        };
        // A probe-failed denominator (0) cannot normalize a percentage.
        assert_eq!(process_usage_pct(prev, current, 0), None);
    }

    #[test]
    fn process_delta_state_skips_first_sample_and_keeps_latest() {
        let now = Instant::now();
        let mut state = ProcessDeltaState::default();
        // First call: no predecessor -> None, but the sample is absorbed.
        assert_eq!(
            state.absorb(
                Some(ProcessSample {
                    cpu_secs: 0.0,
                    sampled_at: now
                }),
                4
            ),
            None
        );
        // Second call produces the interval usage: 1 CPU-second over
        // 1 second / 4 cores = 25%.
        assert_eq!(
            state.absorb(
                Some(ProcessSample {
                    cpu_secs: 1.0,
                    sampled_at: now + std::time::Duration::from_secs(1)
                }),
                4
            ),
            Some(25.0)
        );
        // A failed probe (None) keeps the previous sample and returns None...
        assert_eq!(state.absorb(None, 4), None);
        // ...so the next successful pair spans the full elapsed interval:
        // 0.5 CPU-seconds over 2 seconds / 4 cores = 6.25%.
        assert_eq!(
            state.absorb(
                Some(ProcessSample {
                    cpu_secs: 1.5,
                    sampled_at: now + std::time::Duration::from_secs(3)
                }),
                4
            ),
            Some(6.25)
        );
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn delta_state_skips_first_sample_and_keeps_latest() {
        let mut state = SystemDeltaState::default();
        // 首次采样无上一采样 → None,但采样本身被吸收
        assert_eq!(
            state.absorb(Some(SystemTicks {
                busy: 100,
                idle: 400
            })),
            None
        );
        // 第二次采样产出使用率
        assert_eq!(
            state.absorb(Some(SystemTicks {
                busy: 200,
                idle: 500
            })),
            Some(50.0)
        );
        // 采样失败(None)不更新状态:沿用上一采样,返回 None
        assert_eq!(state.absorb(None), None);
        assert_eq!(
            state.absorb(Some(SystemTicks {
                busy: 300,
                idle: 600
            })),
            Some(50.0)
        );
    }
}
