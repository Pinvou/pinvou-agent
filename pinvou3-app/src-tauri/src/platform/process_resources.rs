//! Cross-platform process-tree resource sampling.
//!
//! Callers register only root PIDs they created. The sampler returns aggregate
//! counts and resident memory; it deliberately never exposes command lines,
//! environment variables, working directories, or process names.

use std::collections::{HashMap, HashSet};
use std::sync::Once;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProcessTreeResources {
    pub process_count: usize,
    pub rss_bytes: u64,
}

pub(crate) struct ProcessResourceSampler {
    system: System,
}

impl Default for ProcessResourceSampler {
    fn default() -> Self {
        static CONFIGURE_SYSINFO: Once = Once::new();
        CONFIGURE_SYSINFO.call_once(|| {
            // Linux sysinfo otherwise retains up to half the process FD limit
            // as cached /proc stat handles. A periodic sampler should trade
            // that cache for a bounded descriptor footprint.
            let _ = sysinfo::set_open_files_limit(0);
        });
        Self {
            system: System::new(),
        }
    }
}

impl ProcessResourceSampler {
    /// Samples every process once, then attributes each process to the nearest
    /// registered ancestor. A single OS snapshot keeps totals internally
    /// consistent and avoids one full process-table scan per runtime.
    pub(crate) fn sample_trees(&mut self, root_pids: &[u32]) -> HashMap<u32, ProcessTreeResources> {
        if root_pids.is_empty() {
            return HashMap::new();
        }
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory().without_tasks(),
        );

        let roots = root_pids
            .iter()
            .copied()
            .map(|pid| (Pid::from_u32(pid), pid))
            .collect::<HashMap<_, _>>();
        let parents = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| (*pid, process.parent()))
            .collect::<HashMap<_, _>>();
        let memory = self
            .system
            .processes()
            .iter()
            .map(|(pid, process)| (*pid, process.memory()))
            .collect::<HashMap<_, _>>();

        aggregate_process_forest(&roots, &parents, &memory)
    }
}

fn aggregate_process_forest(
    roots: &HashMap<Pid, u32>,
    parents: &HashMap<Pid, Option<Pid>>,
    memory: &HashMap<Pid, u64>,
) -> HashMap<u32, ProcessTreeResources> {
    let mut totals = roots
        .values()
        .copied()
        .map(|root| (root, ProcessTreeResources::default()))
        .collect::<HashMap<_, _>>();

    for (pid, rss_bytes) in memory {
        let mut cursor = Some(*pid);
        let mut visited = HashSet::new();
        while let Some(current) = cursor {
            if let Some(root) = roots.get(&current) {
                let total = totals.entry(*root).or_default();
                total.process_count += 1;
                total.rss_bytes = total.rss_bytes.saturating_add(*rss_bytes);
                break;
            }
            if !visited.insert(current) {
                break;
            }
            cursor = parents.get(&current).copied().flatten();
        }
    }

    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn aggregates_each_process_into_the_nearest_registered_tree() {
        let root_a = Pid::from_u32(10);
        let child_a = Pid::from_u32(11);
        let grandchild_a = Pid::from_u32(12);
        let root_b = Pid::from_u32(20);
        let unrelated = Pid::from_u32(30);
        let roots = HashMap::from([(root_a, 10), (root_b, 20)]);
        let parents = HashMap::from([
            (root_a, None),
            (child_a, Some(root_a)),
            (grandchild_a, Some(child_a)),
            (root_b, None),
            (unrelated, None),
        ]);
        let memory = HashMap::from([
            (root_a, 100),
            (child_a, 200),
            (grandchild_a, 300),
            (root_b, 400),
            (unrelated, 500),
        ]);

        let totals = aggregate_process_forest(&roots, &parents, &memory);
        assert_eq!(
            totals.get(&10),
            Some(&ProcessTreeResources {
                process_count: 3,
                rss_bytes: 600,
            })
        );
        assert_eq!(
            totals.get(&20),
            Some(&ProcessTreeResources {
                process_count: 1,
                rss_bytes: 400,
            })
        );
    }

    #[test]
    fn nested_registered_roots_do_not_double_count() {
        let outer = Pid::from_u32(10);
        let inner = Pid::from_u32(11);
        let child = Pid::from_u32(12);
        let roots = HashMap::from([(outer, 10), (inner, 11)]);
        let parents = HashMap::from([(outer, None), (inner, Some(outer)), (child, Some(inner))]);
        let memory = HashMap::from([(outer, 100), (inner, 200), (child, 300)]);

        let totals = aggregate_process_forest(&roots, &parents, &memory);
        assert_eq!(totals.get(&10).map(|total| total.rss_bytes), Some(100));
        assert_eq!(totals.get(&11).map(|total| total.rss_bytes), Some(500));
    }

    #[test]
    fn malformed_parent_cycle_is_bounded() {
        let root = Pid::from_u32(10);
        let first = Pid::from_u32(20);
        let second = Pid::from_u32(21);
        let roots = HashMap::from([(root, 10)]);
        let parents = HashMap::from([(root, None), (first, Some(second)), (second, Some(first))]);
        let memory = HashMap::from([(root, 100), (first, 200), (second, 300)]);

        let totals = aggregate_process_forest(&roots, &parents, &memory);
        assert_eq!(
            totals.get(&10),
            Some(&ProcessTreeResources {
                process_count: 1,
                rss_bytes: 100,
            })
        );
    }

    #[test]
    fn live_sampler_attributes_a_spawned_descendant() {
        if std::env::var_os("PINVOU_RESOURCE_SAMPLER_CHILD").is_some() {
            std::thread::sleep(Duration::from_secs(10));
            return;
        }

        let test_name =
            "platform::process_resources::tests::live_sampler_attributes_a_spawned_descendant";
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", test_name])
            .env("PINVOU_RESOURCE_SAMPLER_CHILD", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sampler child");

        let root_pid = std::process::id();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sampler = ProcessResourceSampler::default();
        let mut sample_durations = Vec::new();
        let observed = loop {
            let sample_started = Instant::now();
            let sample = sampler.sample_trees(&[root_pid]);
            sample_durations.push(sample_started.elapsed());
            if sample
                .get(&root_pid)
                .is_some_and(|resources| resources.process_count >= 2 && resources.rss_bytes > 0)
            {
                break sample[&root_pid];
            }
            if Instant::now() >= deadline {
                panic!("spawned child was not attributed before the deadline");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        child.kill().expect("kill sampler child");
        child.wait().expect("reap sampler child");

        assert!(observed.process_count >= 2);
        assert!(observed.rss_bytes > 0);
        sample_durations.sort_unstable();
        eprintln!(
            "resource sampler: attempts={} p50_us={} max_us={} processes={} rss_bytes={}",
            sample_durations.len(),
            sample_durations[sample_durations.len() / 2].as_micros(),
            sample_durations
                .last()
                .expect("sample duration")
                .as_micros(),
            observed.process_count,
            observed.rss_bytes,
        );
    }
}
