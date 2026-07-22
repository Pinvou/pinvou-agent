use std::time::Duration;

use serde_json::Value;

use super::super::GpuSnapshot;

pub fn gpu_snapshot() -> Option<GpuSnapshot> {
    let script = r#"
$cpuName = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
$gpuName = (Get-CimInstance Win32_VideoController | Where-Object { $_.Name -match 'Intel|Arc|Graphics|GPU' } | Select-Object -First 1 -ExpandProperty Name)
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
    let output = crate::platform::process::output_with_timeout(command, Duration::from_secs(15)).ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
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
        name: if cpu_name.is_empty() {
            gpu_name.to_string()
        } else {
            cpu_name.to_string()
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
