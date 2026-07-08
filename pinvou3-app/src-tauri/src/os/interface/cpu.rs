use crate::monitor::CpuSnapshot;

pub fn cpu_snapshot() -> Option<CpuSnapshot> {
    super::super::platform::cpu_snapshot()
}
