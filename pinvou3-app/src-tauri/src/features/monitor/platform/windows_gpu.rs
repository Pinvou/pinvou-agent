use std::time::Duration;

use serde_json::Value;

use super::super::GpuSnapshot;

pub fn gpu_snapshot() -> Option<GpuSnapshot> {
    let script = r#"
$cpuName = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
$gpuName = (Get-CimInstance Win32_VideoController | Where-Object { $_.Name -notmatch 'Microsoft Basic Display|Remote Display|Virtual|VMware|VirtualBox|QXL|Indirect' } | Select-Object -First 1 -ExpandProperty Name)
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
    let cpu_name = value
        .get("cpuName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let gpu_name = value
        .get("gpuName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if cpu_name.is_empty() && gpu_name.is_empty() {
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
        // GPU 名称优先；仅当探测不到任何真实 GPU（如纯 headless/无显示适配器）时才退回 CPU 名。
        name: if gpu_name.is_empty() {
            cpu_name.to_string()
        } else {
            gpu_name.to_string()
        },
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
    fn falls_back_to_cpu_name_when_no_gpu_found() {
        let snapshot = parse_gpu_json(&sample("AMD Ryzen 9 7950X", ""))
            .expect("snapshot should parse with cpu fallback");
        assert_eq!(snapshot.name, "AMD Ryzen 9 7950X");
    }

    #[test]
    fn uses_gpu_name_when_cpu_name_missing() {
        let snapshot = parse_gpu_json(&sample("", "Intel(R) UHD Graphics 770"))
            .expect("snapshot should parse with gpu name");
        assert_eq!(snapshot.name, "Intel(R) UHD Graphics 770");
    }

    #[test]
    fn returns_none_when_both_names_missing() {
        assert!(parse_gpu_json(&sample("", "")).is_none());
    }
}
