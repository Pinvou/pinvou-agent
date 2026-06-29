use crate::monitor::RamSnapshot;

pub fn ram_snapshot() -> Option<RamSnapshot> {
    super::super::platform::ram_snapshot()
}
