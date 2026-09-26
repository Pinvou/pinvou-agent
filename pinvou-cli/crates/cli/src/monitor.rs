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
use pinvou3_lib::features::monitor::{self, MonitorSnapshot, VllmSnapshot, VllmStatus};

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
    // `None` on a pre-epoch clock rather than `0`: zero is a well-formed
    // timestamp (1970-01-01T00:00:00Z), so a consumer branching on
    // `last_check_ms` could not tell a broken host clock from a real reading.
    // The unknown case is reported as `null` / `-`, like every other
    // measurement this command could not take.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_millis() as u64);
    let (human, value) = status_payload(snapshot.as_ref(), now_ms);
    Ok(success(render(output, human, &value)))
}

/// The one key set `monitor status` emits, in both the model-present and the
/// no-model branch. Written down separately from the payload builder so the
/// guard test compares against a spelled-out list rather than against the
/// producer's own output (which would pass no matter how the shape drifts).
///
/// Test-only because [`status_payload`] is the single producer at runtime;
/// there is nothing for the release build to check it against.
#[cfg(test)]
const STATUS_KEYS: &[&str] = &[
    "vllm_online",
    "last_check_ms",
    "max_model_len",
    "status",
    "health_status",
    "model",
    "configured_model",
    "target_kind",
];

/// Builds both renderings of `monitor status` from the probe result.
///
/// Pure (no host, no clock, no I/O) so the shape contract can be asserted
/// hermetically: the end-to-end command needs a display host and a model
/// endpoint, which would otherwise force the guard test behind `#[ignore]`
/// and let the two branches drift unchecked.
///
/// Both branches go through the same `json!` literal and the same `format!`
/// template; a missing snapshot only changes values (`null` / `-`), never the
/// key set or the line set. `health_status` is the single exception that
/// carries a non-null placeholder, because "unavailable" is a real health
/// verdict the GUI dot renders, not an absent measurement.
///
/// `upstream` is deliberately gone. The old no-model branch emitted it as
/// `null` while the model-present branch structurally could not — it is not a
/// field of `VllmSnapshot` — so the "one stable shape" promise was false in
/// exactly the direction a script notices: the key existed only when there was
/// nothing to report. Reporting the configured endpoint is `models show`'s
/// job, not the health dot's.
fn status_payload(
    snapshot: Option<&VllmSnapshot>,
    now_ms: Option<u64>,
) -> (String, serde_json::Value) {
    let online = snapshot.is_some_and(|snapshot| {
        snapshot.health_status == "verified"
            && matches!(snapshot.status, VllmStatus::Ready | VllmStatus::Busy)
    });
    let health_status = snapshot
        .map(|snapshot| snapshot.health_status.clone())
        .unwrap_or_else(|| "unavailable".to_owned());
    // An unreadable clock renders like the other absent measurements: `null`
    // in JSON, `-` in the human row.
    let checked = now_ms
        .map(|ms| ms.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let value = serde_json::json!({
        "vllm_online": online,
        "last_check_ms": now_ms,
        "max_model_len": snapshot.and_then(|snapshot| snapshot.max_model_len),
        "status": snapshot.map(|snapshot| snapshot.status),
        "health_status": health_status,
        "model": snapshot.and_then(|snapshot| snapshot.model.clone()),
        "configured_model": snapshot.and_then(|snapshot| snapshot.configured_model.clone()),
        "target_kind": snapshot.map(|snapshot| snapshot.target_kind.clone()),
    });
    let human = format!(
        "Online: {online}\nHealth: {health_status}\nStatus: {}\nModel: {}\nConfigured: {}\nTarget: {}\nContextWindow: {}\nChecked: {checked}",
        snapshot
            .map(|snapshot| status_label(snapshot.status))
            .unwrap_or("-"),
        snapshot
            .and_then(|snapshot| snapshot.model.as_deref())
            .unwrap_or("-"),
        snapshot
            .and_then(|snapshot| snapshot.configured_model.as_deref())
            .unwrap_or("-"),
        snapshot
            .map(|snapshot| snapshot.target_kind.as_str())
            .unwrap_or("-"),
        snapshot
            .and_then(|snapshot| snapshot.max_model_len)
            .map(|len| len.to_string())
            .unwrap_or_else(|| "-".to_owned()),
    );
    (human, value)
}

/// Mirror of `get_monitor_snapshot` for the parts a one-shot process can
/// actually measure: GPU, CPU, RAM and the vLLM/model probe are sampled live
/// here exactly as the GUI's monitor page samples them.
///
/// Two groups of fields are **not** measurements in this lane and are reported
/// as such (see [`NOT_APPLICABLE_HEADLESS`]). The GUI holds a long-lived
/// `State<'_, MonitorState>` that the streaming forwarder writes into over the
/// app's whole lifetime; the CLI builds a fresh `MonitorState` per invocation,
/// so `self_perf.*` and `app.session_uptime_secs` are structurally the
/// `Default` zero and can never be anything else. Printing them as `0` next to
/// a real GPU reading claimed a measurement that was never taken — a reader
/// cannot tell "no tokens generated yet" from "this process does not and
/// cannot count tokens".
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
    let (human, value) = snapshot_payload(&snapshot)?;
    Ok(success(render(output, human, &value)))
}

/// JSON pointers whose values this lane cannot measure, listed verbatim in the
/// payload under `not_applicable_headless` so a script does not have to know
/// the rule. Every pointer here is nulled out in the JSON and rendered as
/// `n/a` in human mode.
const NOT_APPLICABLE_HEADLESS: &[&str] = &["/self_perf", "/app/session_uptime_secs"];

/// The complete key set of `monitor snapshot`, **in the order it is emitted**.
///
/// The order is written down rather than inherited from the serializer because
/// this crate's `serde_json` resolves with `preserve_order` (an `indexmap`
/// object), so object key order is insertion order: `cpu` carries
/// `skip_serializing_if = "Option::is_none"` upstream, so on a host where the
/// sampler found a CPU it lands in its struct position, and on a host where it
/// did not, re-inserting it afterwards appended it after `app`. Same keys, two
/// orders, decided by the hardware the command happened to run on — which a
/// consumer diffing two hosts' output sees and a key-set assertion does not.
/// Rebuilding the object against this list fixes the order on every host, and
/// pins the CLI-only `not_applicable_headless` key's position too.
const SNAPSHOT_KEYS: &[&str] = &[
    "generated_at_ms",
    "gpu",
    "cpu",
    "ram",
    "vllm",
    "self_perf",
    "app",
    "not_applicable_headless",
];

/// Builds both renderings of `monitor snapshot`.
///
/// Pure (takes the already-sampled snapshot) so the shape and the
/// not-applicable marking are unit-testable without a display host.
///
/// Three shape fixes live here:
/// - `self_perf` / `app.session_uptime_secs` are nulled and named in
///   `not_applicable_headless` (see [`snapshot`]) instead of being printed as
///   zeroes that look measured;
/// - `cpu` carries `#[serde(skip_serializing_if = "Option::is_none")]`
///   upstream, so the serialized key set changed with the host's hardware —
///   the key is re-inserted as `null` when sampling found no CPU, matching how
///   `gpu` / `ram` / `vllm` already behave;
/// - human mode gained the `Cpu:` line the JSON always had, so the two output
///   modes mirror each other field for field.
fn snapshot_payload(snapshot: &MonitorSnapshot) -> Result<(String, serde_json::Value), CliError> {
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
    let cpu_line = match &snapshot.cpu {
        Some(cpu) => format!(
            "Cpu: {}\t{}",
            cpu.name,
            cpu.total_usage_pct
                .map(|pct| format!("{pct:.1}%"))
                .unwrap_or_else(|| "-".to_owned()),
        ),
        None => "Cpu: unavailable".to_owned(),
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
    let sampled = serde_json::to_value(snapshot)
        .map_err(|error| CliError::failed(format!("monitor snapshot: {error}")))?;
    // `MonitorSnapshot` is a plain struct, so this cannot happen; failing
    // instead of silently skipping the fixes keeps the rule "a shape this
    // command cannot guarantee is an error, not a quiet half-shape".
    let serde_json::Value::Object(mut sampled) = sampled else {
        return Err(CliError::failed(
            "monitor snapshot: the sample is not a JSON object",
        ));
    };
    // Rebuild against [`SNAPSHOT_KEYS`] instead of patching the serializer's
    // object in place: that fixes the key ORDER as well as the key set, and
    // `cpu` — the one field that can be absent, because it is
    // `skip_serializing_if = "Option::is_none"` upstream — gets its `null` in
    // its own position rather than appended at the end.
    let mut ordered = serde_json::Map::new();
    for key in SNAPSHOT_KEYS {
        ordered.insert(
            (*key).to_owned(),
            sampled.remove(*key).unwrap_or(serde_json::Value::Null),
        );
    }
    // Anything left over is an upstream field added since this list was
    // written. Dropping it silently would publish a shape that quietly lost a
    // measurement — the same class of bug the not-applicable marking guards —
    // so it fails loud and names the field.
    if let Some(unexpected) = sampled.keys().next() {
        return Err(CliError::failed(format!(
            "monitor snapshot: the sample carries {unexpected}, which the CLI's \
             published key order does not list; the shape is out of date"
        )));
    }
    let mut value = serde_json::Value::Object(ordered);
    for pointer in NOT_APPLICABLE_HEADLESS {
        let slot = value.pointer_mut(pointer).ok_or_else(|| {
            // A renamed/removed upstream field must not silently stop being
            // marked: the alternative is publishing a structural zero as a
            // measurement again, which is exactly the bug this guards.
            CliError::failed(format!(
                "monitor snapshot: {pointer} is missing from the sample; \
                 the headless not-applicable marking is out of date"
            ))
        })?;
        *slot = serde_json::Value::Null;
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| CliError::failed("monitor snapshot: the sample is not a JSON object"))?;
    object.insert(
        "not_applicable_headless".to_owned(),
        serde_json::Value::Array(
            NOT_APPLICABLE_HEADLESS
                .iter()
                .map(|pointer| serde_json::Value::String((*pointer).to_owned()))
                .collect(),
        ),
    );
    let human = format!(
        "GeneratedAt: {}\n{gpu_line}\n{cpu_line}\n{ram_line}\n{vllm_line}\n\
         SelfPerf: n/a (headless; per-turn token/TTFT counters accumulate inside the running desktop app)\n\
         SessionUptime: n/a (headless; one process per invocation)\n\
         AppVersion: {}",
        snapshot.generated_at_ms, snapshot.app.pinvou3_version,
    );
    Ok((human, value))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermetic shape guards. The end-to-end `monitor status|snapshot` tests
    /// need a display host and a model endpoint and therefore live behind
    /// `#[ignore]` in `tests/misc_contract.rs`; the *shape* contract needs
    /// neither, so it is asserted here and always runs. That split is the
    /// point: while the shape assertion was `#[ignore]`d with the probe, the
    /// two `status` branches drifted to different key sets unnoticed.
    fn probe(status: VllmStatus, health: &str) -> VllmSnapshot {
        VllmSnapshot {
            status,
            model: Some("served-model".to_owned()),
            configured_model: Some("configured-model".to_owned()),
            target_kind: "local".to_owned(),
            metrics_applicable: true,
            health_status: health.to_owned(),
            max_model_len: Some(8192),
            prefix_cache_hits: Some(1.0),
            prefix_cache_queries: Some(2.0),
            ttft_sum_s: Some(0.5),
            ttft_count: Some(1.0),
        }
    }

    fn keys(value: &serde_json::Value) -> Vec<String> {
        value
            .as_object()
            .expect("payload must be a JSON object")
            .keys()
            .cloned()
            .collect()
    }

    /// Both branches must publish the same keys in the same order. Without the
    /// fix the no-model branch carries a ninth key (`upstream`) that the
    /// model-present branch structurally cannot produce.
    #[test]
    fn status_json_has_one_key_set_in_both_branches() {
        let (_, without_model) = status_payload(None, Some(7));
        let (_, with_model) = status_payload(Some(&probe(VllmStatus::Ready, "verified")), Some(7));
        assert_eq!(keys(&without_model), keys(&with_model));
        assert_eq!(keys(&with_model), STATUS_KEYS);
    }

    /// A clock that cannot be read has no timestamp to publish. Reporting `0`
    /// made "the host clock is broken" indistinguishable from a genuine
    /// 1970-01-01 reading, which a consumer checking freshness cannot detect.
    #[test]
    fn status_reports_an_unreadable_clock_as_unknown_rather_than_epoch_zero() {
        let (human, value) = status_payload(None, None);
        assert!(
            value["last_check_ms"].is_null(),
            "an unknown check time must be null, not 0: {value}"
        );
        assert!(human.contains("Checked: -"), "{human}");
        // The key set does not move with the clock.
        assert_eq!(keys(&value), STATUS_KEYS);
        // A real reading still renders as the number it is.
        let (human, value) = status_payload(None, Some(0));
        assert_eq!(value["last_check_ms"], serde_json::json!(0));
        assert!(human.contains("Checked: 0"), "{human}");
    }

    /// `VllmStatus` already derives `#[serde(rename_all = "lowercase")]`, and
    /// the human label is a hand-written copy of that mapping. The two are
    /// pinned equal here rather than merged, because the label is infallible
    /// and `'static` while the serde rendering is neither; a new variant makes
    /// `status_label` fail to compile, and this test then has to name it.
    #[test]
    fn status_label_matches_the_serde_rendering_of_every_variant() {
        for status in [
            VllmStatus::Offline,
            VllmStatus::Ready,
            VllmStatus::Busy,
            VllmStatus::Mismatch,
        ] {
            assert_eq!(
                serde_json::Value::String(status_label(status).to_owned()),
                serde_json::to_value(status).expect("VllmStatus serializes"),
                "the human label must not drift from the serialized name"
            );
        }
    }

    /// `serde_json::json!` stores an explicit `null` as `Value::Null`, so
    /// `get("upstream")` on the old no-model branch returned `Some(Null)`, not
    /// `None` — the removal has to be asserted on the key set, not on
    /// `is_none()`, or the guard silently passes on a value that is present.
    #[test]
    fn status_json_never_carries_the_dropped_upstream_key() {
        for payload in [
            status_payload(None, Some(7)).1,
            status_payload(Some(&probe(VllmStatus::Offline, "offline")), Some(7)).1,
        ] {
            assert!(
                !keys(&payload).iter().any(|key| key == "upstream"),
                "upstream was dropped from the stable shape and must stay gone: {payload}"
            );
        }
    }

    /// Human mode is the other half of the "one stable shape" promise: the
    /// no-model branch used to print 3 lines where the model branch printed 8.
    #[test]
    fn status_human_has_one_line_set_in_both_branches() {
        let (without_model, _) = status_payload(None, Some(7));
        let (with_model, _) = status_payload(Some(&probe(VllmStatus::Busy, "verified")), Some(7));
        let labels = |text: &str| -> Vec<String> {
            text.lines()
                .map(|line| {
                    line.split_once(": ")
                        .map(|(label, _)| label.to_owned())
                        .unwrap_or_else(|| line.to_owned())
                })
                .collect()
        };
        assert_eq!(labels(&without_model), labels(&with_model));
        assert_eq!(
            labels(&with_model),
            [
                "Online",
                "Health",
                "Status",
                "Model",
                "Configured",
                "Target",
                "ContextWindow",
                "Checked",
            ]
        );
        // The values still differ — the branch is not collapsed, only its shape.
        assert!(without_model.contains("Online: false"));
        assert!(without_model.contains("Health: unavailable"));
        assert!(without_model.contains("Model: -"));
        assert!(with_model.contains("Online: true"));
        assert!(with_model.contains("Model: served-model"));
    }

    /// The process-local accumulators must not be published as if they were
    /// sampled. Without the fix `self_perf.gen_tokens_total` serializes as a
    /// measured-looking `0` and human mode prints `SelfGenTokens: 0`.
    #[test]
    fn snapshot_marks_process_local_accumulators_as_not_applicable() {
        let sample = MonitorSnapshot::default();
        let (human, value) = snapshot_payload(&sample).expect("a default sample must render");
        assert!(value["self_perf"].is_null(), "{value}");
        assert!(value["app"]["session_uptime_secs"].is_null(), "{value}");
        assert_eq!(
            value["not_applicable_headless"],
            serde_json::json!(NOT_APPLICABLE_HEADLESS)
        );
        assert!(
            !human.contains("SelfGenTokens"),
            "human mode must not present a structural zero as a token count: {human}"
        );
        assert!(human.contains("SelfPerf: n/a"), "{human}");
        assert!(human.contains("SessionUptime: n/a"), "{human}");
    }

    /// `cpu` is `skip_serializing_if = "Option::is_none"` upstream, so a host
    /// without a CPU reading used to drop the key entirely; human mode had no
    /// CPU line at all.
    ///
    /// The comparison is on the key vectors AS EMITTED, not on sorted copies:
    /// this crate's `serde_json` preserves insertion order, so re-inserting a
    /// missing `cpu` after the fact put it in a different position than the
    /// sampler-found case — a real divergence between two hosts that sorting
    /// the keys before comparing hid completely.
    #[test]
    fn snapshot_keeps_a_stable_key_set_with_and_without_a_cpu_reading() {
        let without_cpu = MonitorSnapshot::default();
        let with_cpu = MonitorSnapshot {
            cpu: Some(pinvou3_lib::features::monitor::CpuSnapshot {
                name: "test-cpu".to_owned(),
                total_usage_pct: Some(12.5),
            }),
            ..MonitorSnapshot::default()
        };
        let (human_without, value_without) = snapshot_payload(&without_cpu).expect("renders");
        let (human_with, value_with) = snapshot_payload(&with_cpu).expect("renders");
        assert_eq!(keys(&value_without), keys(&value_with));
        assert_eq!(
            keys(&value_with),
            SNAPSHOT_KEYS,
            "the published order is the written-down one, on every host"
        );
        assert!(value_without["cpu"].is_null(), "{value_without}");
        assert_eq!(human_without.lines().count(), human_with.lines().count());
        assert!(
            human_without.contains("Cpu: unavailable"),
            "{human_without}"
        );
        assert!(human_with.contains("Cpu: test-cpu"), "{human_with}");
    }
}
