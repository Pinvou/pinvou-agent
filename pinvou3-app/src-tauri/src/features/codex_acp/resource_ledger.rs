use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use crate::platform::process_resources::ProcessResourceSampler;

const SAMPLE_WINDOW: usize = 180;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MIN_RSS_CHANGE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct RuntimeResourceRoot {
    pub pid: u32,
    pub role: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ResourceObservation {
    resident_runtimes: usize,
    sampled_runtimes: usize,
    process_count: usize,
    rss_bytes: u64,
}

#[derive(Default)]
pub(super) struct AcpResourceLedger {
    sampler: ProcessResourceSampler,
    recent_rss: VecDeque<u64>,
    peak_rss_bytes: u64,
    peak_process_count: usize,
    last_emitted: Option<(Instant, ResourceObservation)>,
}

impl AcpResourceLedger {
    pub(super) fn sample_and_record(&mut self, roots: &[RuntimeResourceRoot]) -> Option<String> {
        let root_pids = roots.iter().map(|root| root.pid).collect::<Vec<_>>();
        let samples = self.sampler.sample_trees(&root_pids);
        let mut by_role = BTreeMap::<&'static str, (usize, usize, u64)>::new();
        let mut observation = ResourceObservation {
            resident_runtimes: roots.len(),
            ..ResourceObservation::default()
        };
        for root in roots {
            let Some(sample) = samples.get(&root.pid) else {
                continue;
            };
            if sample.process_count == 0 {
                continue;
            }
            observation.sampled_runtimes += 1;
            observation.process_count = observation
                .process_count
                .saturating_add(sample.process_count);
            observation.rss_bytes = observation.rss_bytes.saturating_add(sample.rss_bytes);
            let role = by_role.entry(root.role).or_default();
            role.0 += 1;
            role.1 = role.1.saturating_add(sample.process_count);
            role.2 = role.2.saturating_add(sample.rss_bytes);
        }

        self.record_observation(observation);
        let now = Instant::now();
        if !should_emit(self.last_emitted, now, observation) {
            return None;
        }
        self.last_emitted = Some((now, observation));

        let roles = by_role
            .into_iter()
            .map(|(role, (runtimes, processes, rss_bytes))| {
                format!("{role}:runtimes={runtimes},processes={processes},rss_bytes={rss_bytes}")
            })
            .collect::<Vec<_>>()
            .join(";");
        Some(format!(
            "resident_runtimes={} sampled_runtimes={} process_count={} rss_bytes={} p95_rss_bytes={} peak_rss_bytes={} peak_process_count={} roles=[{}]",
            observation.resident_runtimes,
            observation.sampled_runtimes,
            observation.process_count,
            observation.rss_bytes,
            percentile_95(&self.recent_rss),
            self.peak_rss_bytes,
            self.peak_process_count,
            roles,
        ))
    }

    fn record_observation(&mut self, observation: ResourceObservation) {
        if self.recent_rss.len() == SAMPLE_WINDOW {
            self.recent_rss.pop_front();
        }
        self.recent_rss.push_back(observation.rss_bytes);
        self.peak_rss_bytes = self.peak_rss_bytes.max(observation.rss_bytes);
        self.peak_process_count = self.peak_process_count.max(observation.process_count);
    }
}

fn should_emit(
    last_emitted: Option<(Instant, ResourceObservation)>,
    now: Instant,
    current: ResourceObservation,
) -> bool {
    let Some((emitted_at, previous)) = last_emitted else {
        return true;
    };
    if now.saturating_duration_since(emitted_at) >= HEARTBEAT_INTERVAL
        || previous.resident_runtimes != current.resident_runtimes
        || previous.sampled_runtimes != current.sampled_runtimes
        || previous.process_count != current.process_count
    {
        return true;
    }
    let rss_change = previous.rss_bytes.abs_diff(current.rss_bytes);
    let relative_threshold = previous.rss_bytes / 10;
    rss_change >= MIN_RSS_CHANGE_BYTES.max(relative_threshold)
}

fn percentile_95(samples: &VecDeque<u64>) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.iter().copied().collect::<Vec<_>>();
    sorted.sort_unstable();
    let index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p95_uses_the_nearest_rank_without_exceeding_the_window() {
        let samples = (1..=100).collect::<VecDeque<_>>();
        assert_eq!(percentile_95(&samples), 95);
        assert_eq!(percentile_95(&VecDeque::from([7])), 7);
        assert_eq!(percentile_95(&VecDeque::new()), 0);
    }

    #[test]
    fn emission_gate_ignores_small_rss_noise_but_keeps_shape_changes() {
        let now = Instant::now();
        let baseline = ResourceObservation {
            resident_runtimes: 2,
            sampled_runtimes: 2,
            process_count: 4,
            rss_bytes: 200 * 1024 * 1024,
        };
        assert!(!should_emit(
            Some((now, baseline)),
            now + Duration::from_secs(60),
            ResourceObservation {
                rss_bytes: baseline.rss_bytes + 8 * 1024 * 1024,
                ..baseline
            },
        ));
        assert!(should_emit(
            Some((now, baseline)),
            now + Duration::from_secs(60),
            ResourceObservation {
                resident_runtimes: 3,
                ..baseline
            },
        ));
        assert!(should_emit(
            Some((now, baseline)),
            now + HEARTBEAT_INTERVAL,
            baseline,
        ));
    }

    #[test]
    fn sample_window_is_bounded_and_tracks_lifetime_peaks() {
        let mut ledger = AcpResourceLedger::default();
        for rss in 1..=(SAMPLE_WINDOW as u64 + 20) {
            ledger.record_observation(ResourceObservation {
                process_count: rss as usize,
                rss_bytes: rss,
                ..ResourceObservation::default()
            });
        }
        assert_eq!(ledger.recent_rss.len(), SAMPLE_WINDOW);
        assert_eq!(ledger.recent_rss.front().copied(), Some(21));
        assert_eq!(ledger.peak_rss_bytes, SAMPLE_WINDOW as u64 + 20);
        assert_eq!(ledger.peak_process_count, SAMPLE_WINDOW + 20);
    }
}
