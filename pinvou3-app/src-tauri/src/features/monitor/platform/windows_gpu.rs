use std::time::Duration;

use serde_json::Value;

use super::super::GpuSnapshot;

pub fn gpu_snapshot() -> Option<GpuSnapshot> {
    let script = r#"
$cpuName = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
# 优先选真实 PCI 物理适配器（PNPDeviceID 以 PCI\ 开头）——IDD 虚拟/远程显示驱动
# （OrayIddDriver、GameViewer 等）是 ROOT\ 枚举，名称黑名单覆盖不全，只能靠 PNPDeviceID 区分；
# 若无 PCI 显卡（纯 headless / 虚拟机），回退到名称黑名单过滤。
$gpu = Get-CimInstance Win32_VideoController | Where-Object { $_.PNPDeviceID -like 'PCI\*' -and $_.Name -notmatch 'Microsoft Basic Display|Remote Display|Virtual|VMware|VirtualBox|QXL|Indirect' } | Select-Object -First 1
if (-not $gpu) { $gpu = Get-CimInstance Win32_VideoController | Where-Object { $_.Name -notmatch 'Microsoft Basic Display|Remote Display|Virtual|VMware|VirtualBox|QXL|Indirect' } | Select-Object -First 1 }
$gpuName = if ($gpu) { $gpu.Name } else { $null }
$cpu = $null
try { $cpu = (Get-Counter '\Processor Information(_Total)\% Processor Utility').CounterSamples[0].CookedValue } catch {}
$gpu = $null
try { $gpu = ((Get-Counter '\GPU Engine(*)\Utilization Percentage').CounterSamples | Measure-Object CookedValue -Sum).Sum } catch {}
$shared = $null
try { $shared = ((Get-Counter '\GPU Adapter Memory(*)\Shared Usage').CounterSamples | Measure-Object CookedValue -Sum).Sum } catch {}
$temp = $null
try {
  $tz = Get-CimInstance -Namespace root\wmi -ClassName MSAcpi_ThermalZoneTemperature | Select-Object -First 1
  if ($tz) { $temp = [math]::Round(($tz.CurrentTemperature / 10) - 273.15, 0) }
} catch {}
[pscustomobject]@{
  cpuName = $cpuName
  gpuName = $gpuName
  cpuPct = $cpu
  gpuPct = $gpu
  sharedBytes = $shared
  tempC = $temp
} | ConvertTo-Json -Compress
"#;
    let mut command = crate::platform::process::HiddenCommand::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    let output =
        crate::platform::process::output_with_timeout(command, Duration::from_secs(15)).ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    parse_gpu_json(&value)
}

/// 解析 PowerShell 输出的 JSON 并构造 GPU 快照。
/// 独立成纯函数便于在任何平台做单元测试（PowerShell 脚本本身只能在 Windows 跑）。
fn parse_gpu_json(value: &Value) -> Option<GpuSnapshot> {
    let gpu_name = value
        .get("gpuName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    // 没有真实 GPU（探测不到任何显示适配器）时返回 None，交给前端已有的
    // `snap.cpu` fallback（"本机处理器"卡片）接管；不能用 CPU 名构造 GPU 快照，
    // 否则前端因 `snap.gpu` 非空而置 gpuAvailable=true，仍把 CPU 显示在 GPU 卡片上。
    if gpu_name.is_empty() {
        return None;
    }
    let cpu_pct = value
        .get("cpuPct")
        .and_then(Value::as_f64)
        .map(|number| number.round().clamp(0.0, 100.0) as u32);
    let gpu_pct = value
        .get("gpuPct")
        .and_then(Value::as_f64)
        .map(|number| number.round().clamp(0.0, 100.0) as u32)
        .unwrap_or(0);
    let shared_mib = value
        .get("sharedBytes")
        .and_then(Value::as_f64)
        .map(|number| (number / 1024.0 / 1024.0).round().max(0.0) as u64);
    let temperature_c = value
        .get("tempC")
        .and_then(Value::as_f64)
        .map(|number| number.round().clamp(0.0, 120.0) as u32);

    Some(GpuSnapshot {
        // GPU 名称必须来自真实显示适配器；探测不到时外层已返回 None，
        // 不再退回 CPU 名（避免前端把 CPU 当 GPU 渲染）。
        name: gpu_name.to_string(),
        vram_used_mib: 0,
        vram_total_mib: 0,
        utilization_pct: gpu_pct,
        processor_utilization_pct: cpu_pct,
        shared_memory_used_mib: shared_mib,
        temperature_c,
        power_w: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(cpu: &str, gpu: &str) -> Value {
        json!({
            "cpuName": cpu,
            "gpuName": gpu,
            "cpuPct": 12.0,
            "gpuPct": 34.0,
            "sharedBytes": 123456789.0,
            "tempC": 45.0,
        })
    }

    #[test]
    fn gpu_name_takes_priority_over_cpu_name() {
        // 回归测试：GPU 卡片必须显示 GPU 型号，而不是 CPU 型号。
        let snapshot = parse_gpu_json(&sample(
            "Intel(R) Core(TM) i7-12700K",
            "NVIDIA GeForce RTX 4070",
        ))
        .expect("snapshot should parse");
        assert_eq!(snapshot.name, "NVIDIA GeForce RTX 4070");
        assert_eq!(snapshot.utilization_pct, 34);
        assert_eq!(snapshot.processor_utilization_pct, Some(12));
        assert_eq!(snapshot.shared_memory_used_mib, Some(118)); // 123456789 B / 1024^2 ≈ 117.7 → round = 118
        assert_eq!(snapshot.temperature_c, Some(45));
    }

    #[test]
    fn returns_none_when_no_gpu_name() {
        // 没有真实 GPU 时绝不能退回 CPU 名：前端因 snap.gpu 非空置 gpuAvailable=true，
        // 会把 CPU 型号继续显示在 GPU 卡片上。应返回 None，让已有 snap.cpu fallback 接管。
        // CPU 名缺失同样返回 None(原 returns_none_when_both_names_missing 的场景):
        // 名字与 CPU 名双缺时没有可展示的标识。
        assert!(parse_gpu_json(&sample("AMD Ryzen 9 7950X", "")).is_none());
        assert!(parse_gpu_json(&sample("", "")).is_none());
    }

    #[test]
    fn uses_gpu_name_when_cpu_name_missing() {
        let snapshot = parse_gpu_json(&sample("", "Intel(R) UHD Graphics 770"))
            .expect("snapshot should parse with gpu name");
        assert_eq!(snapshot.name, "Intel(R) UHD Graphics 770");
    }
}
