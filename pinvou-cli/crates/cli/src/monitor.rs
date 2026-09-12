//! `monitor` family: one-shot backend/model probe and system sample,
//! mirroring `pinvou3-app/src-tauri/src/app/commands/monitor.rs`
//! (`get_backend_status` / `get_monitor_snapshot`).
//!
//! Host decision: both GUI commands are async and the feature functions
//! (`features::monitor::active_model_snapshot` / `::sample_all`) are
//! standalone — they read prefs + the credential store and probe the
//! configured model endpoint, and the sampler additionally runs local
//! `nvidia-smi` / resource queries. None of it touches Tauri state, but the
//! CLI crate has no async runtime of its own, so both subcommands run inside
//! the windowless product host (`run_windowless_host`, the same wiring
//! `memory organize` uses; requires a display — xvfb on headless Linux).
//! Unlike the GUI, the CLI prints one-shot text instead of live gauges.
//!
//! Probes hit the configured model endpoint (default
//! `http://127.0.0.1:8000/v1` when no model is configured). Without a model
//! the status is reported as offline — a clean zero state, not an error.

use crate::support::{render, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::features::monitor::{self, VllmStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorCommand {
    Status,
    Snapshot,
}

const USAGE: &str = "usage: pinvou monitor <status|snapshot>";

pub fn parse(values: &[String]) -> Result<MonitorCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    match subcommand.as_str() {
        "status" => {
            if values.len() > 2 {
                return Err(CliError::usage("monitor status accepts no options"));
            }
            Ok(MonitorCommand::Status)
        }
        "snapshot" => {
            if values.len() > 2 {
                return Err(CliError::usage("monitor snapshot accepts no options"));
            }
            Ok(MonitorCommand::Snapshot)
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

pub fn execute(command: MonitorCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    match command {
        MonitorCommand::Status => status(output),
        MonitorCommand::Snapshot => snapshot(output),
    }
}

/// Mirror of `get_backend_status`: probe the active model only and derive the
/// live dot plus the real context window.
fn status(output: OutputMode) -> Result<CliOutcome, CliError> {
    // The host's work closure must resolve to `anyhow::Result`; the plain
    // probe value is lifted with `Ok(...)` so this module never names the
    // host's feature-gated error type.
    let snapshot = pinvou3_lib::headless_bridge::run_windowless_host(|_pool, _store| {
        let work = async move { monitor::active_model_snapshot().await };
        async move { Ok(work.await) }
    })
    .map_err(|error| CliError::failed(format!("monitor status failed: {}", redact(&error))))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let Some(snapshot) = snapshot else {
        let value = serde_json::json!({
            "vllm_online": false,
            "last_check_ms": now_ms,
            "max_model_len": null,
            "health_status": "unavailable",
        });
        return Ok(success(render(
            output,
            format!("Online: false\nContextWindow: -\nChecked: {now_ms}"),
            &value,
        )));
    };
    let online = snapshot.health_status == "verified"
        && matches!(snapshot.status, VllmStatus::Ready | VllmStatus::Busy);
    let value = serde_json::json!({
        "vllm_online": online,
        "last_check_ms": now_ms,
        "max_model_len": snapshot.max_model_len,
        "status": snapshot.status,
        "health_status": snapshot.health_status,
        "provider": snapshot.provider,
        "model": snapshot.model,
        "configured_model": snapshot.configured_model,
        "upstream": snapshot.upstream,
        "target_kind": snapshot.target_kind,
        "diagnostic": snapshot.diagnostic,
    });
    let human = format!(
        "Online: {online}\nHealth: {}\nStatus: {}\nProvider: {}\nModel: {}\nConfigured: {}\nEndpoint: {}\nTarget: {}\nContextWindow: {}\nChecked: {now_ms}",
        snapshot.health_status,
        status_label(snapshot.status),
        snapshot.provider,
        snapshot.model.as_deref().unwrap_or("-"),
        snapshot.configured_model.as_deref().unwrap_or("-"),
        snapshot.upstream,
        snapshot.target_kind,
        snapshot
            .max_model_len
            .map(|len| len.to_string())
            .unwrap_or_else(|| "-".to_owned()),
    );
    Ok(success(render(output, human, &value)))
}

/// Mirror of `get_monitor_snapshot`: one full sample (GPU/CPU/RAM/vLLM/self
/// metrics) exactly as the GUI's monitor page would render it once.
fn snapshot(output: OutputMode) -> Result<CliOutcome, CliError> {
    let snapshot = pinvou3_lib::headless_bridge::run_windowless_host(|_pool, _store| {
        let work = async move {
            let state = monitor::MonitorState::new();
            monitor::sample_all(
                &state,
                &monitor::vllm_base_url(),
                monitor::vllm_configured_model(),
            )
            .await
        };
        async move { Ok(work.await) }
    })
    .map_err(|error| CliError::failed(format!("monitor snapshot failed: {}", redact(&error))))?;
    let gpu_line = match &snapshot.gpu {
        Some(gpu) => format!(
            "Gpu: {}\t{} MiB / {} MiB\t{}%",
            gpu.name, gpu.vram_used_mib, gpu.vram_total_mib, gpu.utilization_pct
        ),
        None => "Gpu: unavailable".to_owned(),
    };
    let ram_line = match &snapshot.ram {
        Some(ram) => format!(
            "Ram: {} MiB / {} MiB used",
            ram.used_kib / 1024,
            ram.total_kib / 1024
        ),
        None => "Ram: unavailable".to_owned(),
    };
    let vllm_line = match &snapshot.vllm {
        Some(vllm) => format!(
            "Backend: {}\thealth={}\tmodel={}\twindow={}",
            status_label(vllm.status),
            vllm.health_status,
            vllm.model.as_deref().unwrap_or("-"),
            vllm.max_model_len
                .map(|len| len.to_string())
                .unwrap_or_else(|| "-".to_owned()),
        ),
        None => "Backend: unavailable".to_owned(),
    };
    let value = serde_json::to_value(&snapshot)
        .map_err(|error| CliError::failed(format!("monitor snapshot: {error}")))?;
    let human = format!(
        "GeneratedAt: {}\n{gpu_line}\n{ram_line}\n{vllm_line}\nSelfGenTokens: {}\nAppVersion: {}",
        snapshot.generated_at_ms, snapshot.self_perf.gen_tokens_total, snapshot.app.pinvou3_version,
    );
    Ok(success(render(output, human, &value)))
}

fn status_label(status: VllmStatus) -> &'static str {
    match status {
        VllmStatus::Offline => "offline",
        VllmStatus::Ready => "ready",
        VllmStatus::Busy => "busy",
        VllmStatus::Mismatch => "mismatch",
    }
}

/// Probe failures may embed request URLs but never secrets; still, the
/// redaction pass keeps any credential that leaked into an error chain out of
/// CLI output (same guard the memory organize lane uses). Generic over the
/// error type so this module never needs to name the host's error
/// type (which is feature-gated upstream).
fn redact(error: &(impl std::fmt::Display + std::fmt::Debug)) -> String {
    pinvou3_lib::platform::credential_store::redact_secret(&format!("{error:#}"))
}
