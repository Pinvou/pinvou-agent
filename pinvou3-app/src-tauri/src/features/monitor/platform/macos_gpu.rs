//! macOS GPU 采样:解析 `ioreg -r -c IOAccelerator -d 1` 输出。
//!
//! Apple Silicon(及 Intel Mac 的独显)IOAccelerator 字典自带:
//!   - `"model" = "Apple M3 Max"`                     → GPU 名称
//!   - `PerformanceStatistics` 里 `"Device Utilization %"=48` → 核心利用率(无需 root)
//!   - `"In use system memory"=1221361664`            → GPU 在用的统一内存(字节)
//! Apple Silicon 是统一内存架构,没有独立 VRAM:vram_* 置 0,前端据此切到
//! 「统一内存」显示(与 GB10 的 vram=[N/A] 同一通路)。温度/功耗无公开免 root
//! 接口(powermetrics 要 sudo),留 None,前端只渲染有数据的行。
//!
//! 解析失败(无 GPU 字典、输出格式变化)→ None,前端显示「状态不可用」,
//! 与现有 graceful degrade 原则一致。

use super::super::GpuSnapshot;

pub fn gpu_snapshot() -> Option<GpuSnapshot> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-r", "-c", "IOAccelerator", "-d", "1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_ioreg_gpu(&text)
}

fn parse_ioreg_gpu(text: &str) -> Option<GpuSnapshot> {
    // 至少要有 GPU 名称,否则视为无可用 GPU(不拿一个全空快照冒充可用)。
    let name = parse_quoted_property(text, "model")?;
    let utilization = parse_perf_stat(text, "Device Utilization %")
        .map(|v| v.min(100) as u32)
        .unwrap_or(0);
    let shared_memory_used_mib =
        parse_perf_stat(text, "In use system memory").map(|b| b / 1024 / 1024);
    Some(GpuSnapshot {
        name,
        vram_used_mib: 0,
        vram_total_mib: 0,
        utilization_pct: utilization,
        processor_utilization_pct: None,
        shared_memory_used_mib,
        temperature_c: None,
        power_w: None,
    })
}

/// 提取 `"key" = "value"` 形式的字符串属性(取首个匹配)。
fn parse_quoted_property(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\" = \"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find('"')? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// 提取 `PerformanceStatistics` 内 `"key"=<整数>` 的数值(取首个匹配)。
/// key 带完整引号匹配,`"In use system memory"` 不会误中
/// `"In use system memory (driver)"`。
fn parse_perf_stat(text: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"=");
    let start = text.find(&needle)? + needle.len();
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 本机真实输出节选(Apple M3 Max,ioreg -r -c IOAccelerator -d 1)。
    const REAL_EXCERPT: &str = r#"
    +-o AGXAcceleratorG15X  <class AGXAcceleratorG15X, id 0x100000455, registered, matched, active, busy 0 (396 ms), retain 56>
    {
      "IOClass" = "AGXAcceleratorG15X"
      "MetalPluginName" = "AGXMetalG15X_M1"
      "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=4869685248,"Tiler Utilization %"=48,"recoveryCount"=0,"Renderer Utilization %"=47,"Device Utilization %"=48,"In use system memory"=1221361664}
      "model" = "Apple M3 Max"
      "gpu-core-count" = 30
    }
"#;

    #[test]
    fn parses_apple_silicon_gpu() {
        let snap = parse_ioreg_gpu(REAL_EXCERPT).expect("应解析出 GPU 快照");
        assert_eq!(snap.name, "Apple M3 Max");
        assert_eq!(snap.utilization_pct, 48);
        // 统一内存:vram 恒 0,前端切统一内存显示。
        assert_eq!(snap.vram_total_mib, 0);
        assert_eq!(snap.vram_used_mib, 0);
        // "In use system memory"=1221361664 字节 = 1164 MiB;且不能误中 (driver) 变体(=0)。
        assert_eq!(snap.shared_memory_used_mib, Some(1221361664 / 1024 / 1024));
        assert_eq!(snap.temperature_c, None);
        assert_eq!(snap.power_w, None);
    }

    #[test]
    fn missing_model_returns_none() {
        // 无 "model" 属性(空输出 / 无 GPU)→ None,前端显示「状态不可用」。
        assert!(parse_ioreg_gpu("").is_none());
        assert!(parse_ioreg_gpu("{ \"IOClass\" = \"AGXAcceleratorG15X\" }").is_none());
    }

    #[test]
    fn missing_utilization_defaults_zero_but_keeps_name() {
        // 老系统可能没有 PerformanceStatistics:名称仍在,利用率按 0 显示,
        // 比整块「不可用」更有信息量。
        let text = r#"{ "model" = "Apple M1" }"#;
        let snap = parse_ioreg_gpu(text).unwrap();
        assert_eq!(snap.name, "Apple M1");
        assert_eq!(snap.utilization_pct, 0);
        assert_eq!(snap.shared_memory_used_mib, None);
    }

    #[test]
    fn utilization_clamped_to_100() {
        let text =
            r#"{ "PerformanceStatistics" = {"Device Utilization %"=137} "model" = "Apple M2" }"#;
        let snap = parse_ioreg_gpu(text).unwrap();
        assert_eq!(snap.utilization_pct, 100);
    }

    #[test]
    fn gpu_snapshot_real_host() {
        // 集成测试(本机 darwin):ioreg 在真实 macOS 上应解析出带名称的 GPU 快照。
        // mirror macos_memory::ram_snapshot_cache_serves_repeat_calls 的本机集成测试模式。
        let snap = gpu_snapshot().expect("gpu_snapshot 在 macOS host 上应返回 Some");
        assert!(!snap.name.is_empty());
        assert!(snap.utilization_pct <= 100);
    }
}
