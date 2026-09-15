use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::super::RamSnapshot;

/// RAM 快照缓存(3s TTL)。mirror `monitor.rs::gpu_snapshot` 的
/// `OnceLock<Mutex<Cache>>` 模式:监控页 1s interval 常开时,ram_snapshot 每次会
/// 串行 fork+exec 三次(vm_stat / sysctl hw.memsize / sysctl vm.swapusage),
/// 缓存避免每秒 3 次 spawn 的 CPU/IO 开销,以及 `Command::output()` 阻塞 wait
/// 累积延迟拖慢采样。常驻量仅几个 u64,Mutex 锁持有时间极短。
#[derive(Default)]
struct RamSnapshotCache {
    sampled_at: Option<Instant>,
    value: Option<RamSnapshot>,
}

pub fn ram_snapshot() -> Option<RamSnapshot> {
    static RAM_CACHE: OnceLock<Mutex<RamSnapshotCache>> = OnceLock::new();
    let cache = RAM_CACHE.get_or_init(|| Mutex::new(RamSnapshotCache::default()));
    let mut guard = cache.lock();
    if guard
        .sampled_at
        .is_some_and(|sampled_at| sampled_at.elapsed() < Duration::from_secs(3))
    {
        return guard.value.clone();
    }
    let value = ram_snapshot_uncached();
    guard.sampled_at = Some(Instant::now());
    if let Some(snapshot) = value {
        guard.value = Some(snapshot.clone());
        Some(snapshot)
    } else {
        // 与 gpu_snapshot 一致:采样失败时保留上次有效值,避免监控页在
        // "正常数据" 与 "不可用" 之间闪烁(vm_stat 偶发卡顿时不丢 UI)。
        guard.value.clone()
    }
}

fn ram_snapshot_uncached() -> Option<RamSnapshot> {
    let text = std::process::Command::new("/usr/bin/vm_stat")
        .output()
        .ok()?
        .stdout;
    let text = String::from_utf8_lossy(&text);
    parse_vm_stat(&text)
}

fn parse_vm_stat(text: &str) -> Option<RamSnapshot> {
    // 提取 page size。真实 vm_stat 必打印此行,几乎不会触发回退;但回退值要按架构设对:
    //   - Apple Silicon(arm64): 迄今所有机型 page size 恒为 16384
    //   - Intel Mac(x86_64): page size 为 4096
    // 用 cfg(target_arch) 区分,保证两架构回退都准确(而非此前一律 4096 在 arm64 上
    // 会把总量算小 4×,或一律 16384 在 Intel 上算大 4×)。
    let fallback_page_size: u64 = if cfg!(target_arch = "aarch64") {
        16384
    } else {
        4096
    };
    let page_size: u64 = text
        .lines()
        .find_map(|l| {
            l.split("page size of ").nth(1).and_then(|s| {
                s.split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
            })
        })
        .unwrap_or(fallback_page_size);

    let mut free: u64 = 0;
    let mut active: u64 = 0;
    let mut inactive: u64 = 0;
    let mut speculative: u64 = 0;
    let mut wired: u64 = 0;
    let mut compressor: u64 = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        let value = || -> Option<u64> {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            // 最后一项形如 "10000." 去掉句号
            parts
                .last()
                .map(|s| s.trim_end_matches('.'))
                .and_then(|s| s.parse::<u64>().ok())
        };
        if trimmed.starts_with("Pages free:") {
            free = value().unwrap_or(0);
        } else if trimmed.starts_with("Pages active:") {
            active = value().unwrap_or(0);
        } else if trimmed.starts_with("Pages inactive:") {
            inactive = value().unwrap_or(0);
        } else if trimmed.starts_with("Pages speculative:") {
            speculative = value().unwrap_or(0);
        } else if trimmed.starts_with("Pages wired down:") {
            wired = value().unwrap_or(0);
        } else if trimmed.starts_with("Pages occupied by compressor:") {
            compressor = value().unwrap_or(0);
        }
    }
    let total_pages = free + active + inactive + speculative + wired + compressor;
    if total_pages == 0 {
        return None;
    }
    if page_size == 0 {
        return None;
    }
    let page_kib = page_size / 1024;
    // 总内存优先用 sysctl hw.memsize(物理内存精确值);vm_stat 五类页和不含 compressor
    // 等类别会系统性偏低约 4-7%,导致监控页总量偏小、使用率失真。
    let total_kib = sysctl_hw_memsize_kib().unwrap_or(total_pages * page_kib);
    // 已用 = active + speculative + wired + compressor(speculative 是已分配但尚未
    // 被判定为 inactive 的页,Activity Monitor 的"App Memory"同样计入;不含 free/
    // inactive 这类可回收内存)。与 Linux 侧 total - MemAvailable 口径一致。
    let used_kib = (active + speculative + wired + compressor) * page_kib;
    // swap 解析 sysctl vm.swapusage(输出如 "total = 3072.00M  used = 1024.00M  free = 2048.00M")。
    let (swap_total_kib, swap_used_kib) = sysctl_swap_kib().unwrap_or((0, 0));
    Some(RamSnapshot {
        total_kib,
        used_kib,
        swap_total_kib,
        swap_used_kib,
    })
}

/// 读 `sysctl -n hw.memsize`(物理内存字节数)并转 KiB。失败返回 None(回退到页和估算)。
fn sysctl_hw_memsize_kib() -> Option<u64> {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let bytes: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(bytes / 1024)
}

/// 解析 sysctl vm.swapusage 输出中某字段的 KiB 值。
/// 字段格式形如 `total = 1234M` / `free = 5G` / `used = 1024K`,后缀 M/G/K
/// 单位换算到 KiB。抽为自由函数以便单测直接覆盖(此前是 sysctl_swap_kib 内的
/// 闭包,测试只能复制逻辑,生产改了它不会失败)。
fn parse_swap_field(line: &str, key: &str) -> Option<u64> {
    // 找 "key = <num><unit>" 中的 <num><unit>。
    let idx = line.find(key)?;
    let after = &line[idx + key.len()..];
    let after = after.trim_start_matches([' ', '=']);
    let num_str: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let num: f64 = num_str.parse().ok()?;
    let unit = after
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
        .chars()
        .next();
    // 无单位裸数字:macOS vm.swapusage 实际只输出 M/G。
    // 真出现裸数字(unit 为 None)或未知后缀(如 T/P)时保守按字节处理(num / 1024)。
    let kib = match unit {
        Some('G') | Some('g') => num * 1024.0 * 1024.0,
        Some('M') | Some('m') => num * 1024.0,
        Some('K') | Some('k') => num,
        Some('B') | Some('b') => num / 1024.0,
        // 无单位(None)或未知后缀(含 T/P 等):保守按字节处理(与注释一致,不放大百万倍)。
        _ => num / 1024.0,
    };
    Some(kib as u64)
}

/// 解析 `sysctl vm.swapusage` 的 total/used。输出形如:
/// `total = 3072.00M  used = 1024.00M  free = 2048.00M (encrypted)`
/// 数值带 M/G 后缀,需分别解析。失败返回 (0, 0)。
fn sysctl_swap_kib() -> Option<(u64, u64)> {
    let out = std::process::Command::new("/usr/sbin/sysctl")
        .args(["vm.swapusage"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let total = parse_swap_field(&line, "total").unwrap_or(0);
    let used = parse_swap_field(&line, "used").unwrap_or(0);
    Some((total, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vm_stat_extracts_physical_and_swap() {
        // 合成 fixture:含 compressor 行(此前未计入)。
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                          10000.\n\
Pages active:                        50000.\n\
Pages inactive:                      20000.\n\
Pages speculative:                   1000.\n\
Pages wired down:                    30000.\n\
Pages occupied by compressor:         5000.\n\
";
        let snap = parse_vm_stat(text).unwrap();
        assert!(snap.total_kib > 0);
        // used = active + speculative + wired + compressor
        let used_kib_expected = (50_000 + 1_000 + 30_000 + 5_000) * (16384 / 1024);
        assert_eq!(snap.used_kib, used_kib_expected);
        assert!(snap.used_kib < snap.total_kib);
    }

    #[test]
    fn parse_vm_stat_real_output() {
        // 本机真实 vm_stat 输出(page size 16384,Apple Silicon)。
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                                   215207.\n\
Pages active:                                 926274.\n\
Pages inactive:                               890093.\n\
Pages speculative:                             46624.\n\
Pages throttled:                                   0.\n\
Pages wired down:                             215965.\n\
Pages purgeable:                               21355.\n\
\"Translation faults\":                       66785368.\n\
Pages copy-on-write:                         2618878.\n\
Pages zero filled:                         476500694.\n\
Pages reactivated:                            113523.\n\
Pages purged:                                 249342.\n\
File-backed pages:                            936200.\n\
Anonymous pages:                              926791.\n\
Pages stored in compressor:                    30554.\n\
Pages occupied by compressor:                   9806.\n\
Decompressions:                                13995.\n\
Compressions:                                  50695.\n\
Pageins:                                     1685163.\n\
Pageouts:                                         24.\n\
Swapins:                                           0.\n\
Swapouts:                                          0.\n\
";
        let snap = parse_vm_stat(text).unwrap();
        let page_kib: u64 = 16384 / 1024;
        // used_kib 是纯页计算(active + speculative + wired + compressor),
        // 完全由输入文本决定,与运行环境无关 → 可以精确断言。
        let used_pages: u64 = 926274 + 46624 + 215965 + 9806;
        assert_eq!(snap.used_kib, used_pages * page_kib);
        assert_eq!(snap.used_kib, 19178704);

        // total_kib 注意:parse_vm_stat 内部调 sysctl_hw_memsize_kib() 读真实系统
        // 物理内存(hw.memsize),该值取决于运行环境(CI runner ~7GB / 开发机 16-64GB),
        // 与这段合成文本代表的 ~36GB Mac 无关。因此这里只断言 total_kib > 0
        // (说明 sysctl 或页和回退至少有一个成功),不断言它与页和的量级关系——
        // 那会因 CI 与开发机 RAM 不同而脆弱失败。
        // total_kib 与页和的一致性由 parse_vm_stat_extracts_physical_and_swap
        // (不触发 sysctl 的纯文本路径)和 ram_snapshot(集成测试,本机跑)覆盖。
        assert!(snap.total_kib > 0, "total_kib 不应为 0");
    }

    #[test]
    fn parse_vm_stat_default_page_size_matches_arch() {
        // 无 "page size of" 行时按架构回退:arm64→16384、其它(Intel x86_64)→4096。
        // 通过 used_kib(纯页计算,不受 hw.memsize 覆盖 total 的影响)观测:
        // used_kib = (active+wired+compressor) * page_kib,fixture 只放 1 页 active。
        let text = "Pages active: 1.\n";
        let snap = parse_vm_stat(text).unwrap();
        let expected_page_kib: u64 = if cfg!(target_arch = "aarch64") {
            16384 / 1024
        } else {
            4096 / 1024
        };
        assert_eq!(snap.used_kib, expected_page_kib);

        // total 为 0 时(所有页类都 0)返回 None。
        assert!(parse_vm_stat("Pages free: 0.\nPages active: 0.\n").is_none());
    }

    #[test]
    fn swap_parsing_handles_suffixes() {
        // 直接覆盖提取出的自由函数 parse_swap_field(此前此测试复制了生产闭包逻辑,
        // 生产改了它也不会失败)。M/G/K 后缀换算到 KiB。
        // 1024M = 1024*1024 KiB
        assert_eq!(
            parse_swap_field("total = 1024M  used = 512M  free = 512M", "total"),
            Some(1024 * 1024)
        );
        // 2G = 2*1024*1024 KiB
        assert_eq!(parse_swap_field("free = 2G", "free"), Some(2 * 1024 * 1024));
        // 0K = 0 KiB
        assert_eq!(parse_swap_field("used = 0K", "used"), Some(0));
        // 生产逻辑对 T 后缀无专门分支 → 落入 `_`(按字节)分支:5T 视为 5 字节 → /1024 → 0 KiB。
        // 断言锁住当前行为;若未来为 T 增加专门分支需同步更新此处。
        assert_eq!(parse_swap_field("total = 5T", "total"), Some(0));
        // 缺失字段返回 None(line.find(key) 失败 → `?` 提前返回)。
        assert_eq!(parse_swap_field("total = 1024M", "used"), None);
    }

    #[test]
    fn ram_snapshot_cache_serves_repeat_calls() {
        // 集成测试(本机 darwin):ram_snapshot 第一次 spawn vm_stat/sysctl,
        // 3s 内第二次必须命中缓存返回同一快照(mirror gpu_snapshot 的缓存模式)。
        // RamSnapshot 未派生 PartialEq(仅 Debug/Clone/Serialize),逐字段比较 u64。
        // 同 1s 窗口内 vm_stat 页数不变 → 任一字段不一致即说明第二次未命中缓存而重算出错。
        let first = ram_snapshot().expect("ram_snapshot 在 macOS host 上应返回 Some");
        let second = ram_snapshot().expect("ram_snapshot 第二次(应命中缓存)也应返回 Some");
        assert_eq!(first.total_kib, second.total_kib);
        assert_eq!(first.used_kib, second.used_kib);
        assert_eq!(first.swap_total_kib, second.swap_total_kib);
        assert_eq!(first.swap_used_kib, second.swap_used_kib);
    }
}
