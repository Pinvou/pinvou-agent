use super::super::RamSnapshot;

pub fn ram_snapshot() -> Option<RamSnapshot> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo(&text)
}

fn parse_meminfo(text: &str) -> Option<RamSnapshot> {
    let mut total = None;
    let mut available = None;
    let mut swap_total = None;
    let mut swap_free = None;
    for line in text.lines() {
        let (key, val) = line.split_once(':')?;
        let kib: u64 = val.trim().trim_end_matches(" kB").parse().ok().unwrap_or(0);
        match key {
            "MemTotal" => total = Some(kib),
            "MemAvailable" => available = Some(kib),
            "SwapTotal" => swap_total = Some(kib),
            "SwapFree" => swap_free = Some(kib),
            _ => {}
        }
    }
    let total = total?;
    let available = available?;
    Some(RamSnapshot {
        total_kib: total,
        used_kib: total.saturating_sub(available),
        swap_total_kib: swap_total.unwrap_or(0),
        swap_used_kib: swap_total
            .unwrap_or(0)
            .saturating_sub(swap_free.unwrap_or(0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meminfo_extracts_physical_and_swap_memory() {
        let text = "\
MemTotal:       16384000 kB
MemAvailable:   4096000 kB
SwapTotal:       2048000 kB
SwapFree:         512000 kB
";
        let snapshot = parse_meminfo(text).unwrap();
        assert_eq!(snapshot.total_kib, 16_384_000);
        assert_eq!(snapshot.used_kib, 12_288_000);
        assert_eq!(snapshot.swap_total_kib, 2_048_000);
        assert_eq!(snapshot.swap_used_kib, 1_536_000);
    }

    #[test]
    fn parse_meminfo_requires_total_and_available() {
        assert!(parse_meminfo("SwapTotal: 1 kB\n").is_none());
    }
}
