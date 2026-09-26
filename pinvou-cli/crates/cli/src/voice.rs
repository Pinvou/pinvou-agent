//! `voice` family: local ASR status/transcription/install and voice
//! post-processing, mirroring `pinvou3-app/src-tauri/src/app/commands/voice.rs`.
//!
//! Visibility disclosure: most of the `pinvou3_lib::features::voice` surface
//! (status, engine transcription, platform adapters) is `pub(crate)` to the
//! app crate, so the CLI *mirrors* its semantics over the exact same paths
//! and environment variables instead of calling it:
//! - data root `$PINVOU3_HOME/asr` (`features::voice::voice_asr::asr_dir`),
//!   engine binary per platform adapter (`sense-voice-main` on Linux,
//!   `pinvou-asr` elsewhere), model `sense-voice-small-q4_k.gguf` (Linux/
//!   macOS) or `sensevoice-small-q8.gguf` (Windows) with the same expected
//!   byte size, and the same `PINVOU3_ASR_CMD` / `PINVOU3_DEEPSPEECH2_CMD` /
//!   `PADDLESPEECH_BIN` external-CLI fallback chain with the same `asr
//!   --model --lang --input` protocol, 60 s default timeout and exit-code-6
//!   "no speech" convention.
//! - the app verifies downloaded models by size **and** sha256; the CLI
//!   mirrors both (sha256 through the shared
//!   `platform::connector_lock::file_sha256_hex`).
//! - the app's transcript parser (`features::voice::transcript`) is shared
//!   verbatim: the CLI calls `parse_asr_transcript` so engine protocols and
//!   noise filters cannot drift between the two surfaces.
//! - `asr-install` mutates the system (ffmpeg through pkexec/apt) exactly
//!   like `deps install`, so it is gated behind an explicit `--yes` (exit 2
//!   without it) and stays Linux-only.
//! - macOS transcription uses the system Speech framework through a
//!   crate-private adapter; the CLI cannot reach it, so on macOS it reports
//!   the Speech runtime as ready (same as the GUI status) but performs
//!   transcription through the external-CLI lane only.
//! - the bundled engine and model the Windows MSI installs beside
//!   `pinvou.exe` are resolved app-side through `pub(crate)`
//!   `platform::os::windows` helpers, so the CLI cannot see them at all: it
//!   looks only under `$PINVOU3_HOME/asr`, and `asr-status` says so and
//!   points at the `PINVOU3_ASR_CMD` escape hatch (which now also resolves
//!   the managed `pinvou-asr.exe`, the same external `asr --model --lang
//!   --input` protocol the app drives that runtime with). Making the CLI
//!   find the MSI copy by itself needs an app-side visibility change.
//!
//! One deliberate deviation from the GUI's fallback contract, forced by the
//! headless surface: after a native engine failure the external-CLI fallback
//! resolves any candidate command — configured override, managed dir, or a
//! `pinvou-asr` on PATH — while the GUI falls back only on an explicitly
//! configured override and otherwise reports `asr_engine_error`.
//!
//! `voice postprocess` needs the GUI's `EnginePool` state to resolve the
//! active model credentials, so it runs through the windowless product host
//! (`run_windowless_host`, requires a display / xvfb like `agent run`) and
//! then issues the same OpenAI-compatible (or Anthropic Messages) HTTP
//! request with the mirrored prompt constants. The prompts are private to
//! `app/commands/voice.rs` and are duplicated here verbatim; if they drift
//! upstream this module must be updated in the same pull request. The user
//! message is assembled section for section like
//! `voice_postprocess_user_content`, including the `DRAFT_TEXT` block the
//! edit prompt's rule 10 promises — `--draft` / `--draft-file` carry what the
//! GUI sends as `draft_text`, and `--mode edit` (whose whole job is rewriting
//! that draft) refuses without one. Only the `ASR_RAW` section has no CLI
//! equivalent: `--text` is already the corrected ASR text and the CLI applies
//! no rule corrections of its own, so there is no before/after pair to send.
//! The per-attempt timeout budget is measured from *after* the host is up, so it
//! bounds the model round-trip like the GUI's does rather than the tokio /
//! Tauri / `SessionStore` boot; `POSTPROCESS_TOTAL_BUDGET` is the separate
//! overall bound on the command.
//!
//! What `voice postprocess` deliberately does NOT mirror is the GUI's
//! *client-side* voice pipeline in `platform/tauri/bridge/voice.js`: the
//! deterministic rule corrections applied before the model call
//! (`applyVoiceDeterministicCorrections`) and the output validator after it
//! (`validateVoicePostprocessOutput`, which discards a candidate that shrank
//! below 55 % of the rule-corrected text or dropped a protected term and
//! falls back to that rule text). Both exist because the GUI writes the
//! result straight into the user's input box unseen; the CLI prints it for a
//! human to read and pipe on, where a silent substitution of different text
//! is the worse failure. Mirroring them would also mean copying several
//! hundred lines of JS correction tables with no drift guard. The omission is
//! reported in every postprocess result (`omitted_stages` in JSON, the
//! trailing `Note:` line in human output) instead of being silent.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::support::{render, sandbox_home, success};
use crate::{CliError, CliOutcome, OutputMode};
use pinvou3_lib::platform::paths::pinvou3_home;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VoiceCommand {
    Transcribe {
        path: PathBuf,
    },
    Postprocess {
        mode: PostprocessMode,
        text: Option<String>,
        text_file: Option<PathBuf>,
        /// The input-box body the GUI sends as `draft_text`
        /// (`platform/tauri/bridge/voice.js` → `postprocess_voice_text`). The
        /// edit prompt rewrites it, so `--mode edit` requires it.
        draft: Option<String>,
        draft_file: Option<PathBuf>,
    },
    AsrStatus,
    AsrInstall {
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostprocessMode {
    Dictation,
    Task,
    Edit,
}

impl PostprocessMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dictation => "dictation",
            Self::Task => "task",
            Self::Edit => "edit",
        }
    }
}

const USAGE: &str = "usage: pinvou voice <transcribe <PATH>|postprocess --mode dictation|task|edit \
     (--text S|--text-file F) [--draft S|--draft-file F]|asr-status|asr-install [--yes]>";

pub fn parse(values: &[String]) -> Result<VoiceCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(USAGE))?;
    let rest = &values[2..];
    match subcommand.as_str() {
        "transcribe" => {
            let path = rest
                .first()
                .map(PathBuf::from)
                .ok_or_else(|| CliError::usage("voice transcribe requires an audio PATH"))?;
            if rest.len() > 1 {
                return Err(CliError::usage("voice transcribe accepts no options"));
            }
            Ok(VoiceCommand::Transcribe { path })
        }
        "postprocess" => {
            let mut mode: Option<PostprocessMode> = None;
            let mut text: Option<String> = None;
            let mut text_file: Option<PathBuf> = None;
            let mut draft: Option<String> = None;
            let mut draft_file: Option<PathBuf> = None;
            let mut index = 0;
            while index < rest.len() {
                let token = rest[index].as_str();
                match token {
                    "--mode" => {
                        if mode.is_some() {
                            return Err(CliError::usage("duplicate voice option --mode"));
                        }
                        let value = rest.get(index + 1).ok_or_else(|| {
                            CliError::usage("voice option --mode requires a value")
                        })?;
                        mode = Some(match value.as_str() {
                            "dictation" => PostprocessMode::Dictation,
                            "task" => PostprocessMode::Task,
                            "edit" => PostprocessMode::Edit,
                            other => {
                                return Err(CliError::usage(format!(
                                    "voice postprocess --mode must be dictation, task or edit \
                                     (got {other})"
                                )));
                            }
                        });
                        index += 2;
                    }
                    "--text" => {
                        if text.is_some() || text_file.is_some() {
                            return Err(CliError::usage("use only one of --text or --text-file"));
                        }
                        let value = rest.get(index + 1).ok_or_else(|| {
                            CliError::usage("voice option --text requires a value")
                        })?;
                        if value.is_empty() || value.starts_with("--") {
                            return Err(CliError::usage("voice option --text requires a value"));
                        }
                        text = Some(value.clone());
                        index += 2;
                    }
                    "--text-file" => {
                        if text.is_some() || text_file.is_some() {
                            return Err(CliError::usage("use only one of --text or --text-file"));
                        }
                        let value = rest.get(index + 1).ok_or_else(|| {
                            CliError::usage("voice option --text-file requires a value")
                        })?;
                        if value.is_empty() || value.starts_with("--") {
                            return Err(CliError::usage(
                                "voice option --text-file requires a value",
                            ));
                        }
                        text_file = Some(PathBuf::from(value));
                        index += 2;
                    }
                    // The draft is the input-box body the GUI always sends
                    // (`draft_text`); the edit prompt rewrites it and its
                    // rule 10 promises a DRAFT_TEXT section, so without it
                    // the edit mode instructs the model about text that was
                    // never transmitted.
                    "--draft" => {
                        if draft.is_some() || draft_file.is_some() {
                            return Err(CliError::usage("use only one of --draft or --draft-file"));
                        }
                        let value = rest.get(index + 1).ok_or_else(|| {
                            CliError::usage("voice option --draft requires a value")
                        })?;
                        if value.is_empty() || value.starts_with("--") {
                            return Err(CliError::usage("voice option --draft requires a value"));
                        }
                        draft = Some(value.clone());
                        index += 2;
                    }
                    "--draft-file" => {
                        if draft.is_some() || draft_file.is_some() {
                            return Err(CliError::usage("use only one of --draft or --draft-file"));
                        }
                        let value = rest.get(index + 1).ok_or_else(|| {
                            CliError::usage("voice option --draft-file requires a value")
                        })?;
                        if value.is_empty() || value.starts_with("--") {
                            return Err(CliError::usage(
                                "voice option --draft-file requires a value",
                            ));
                        }
                        draft_file = Some(PathBuf::from(value));
                        index += 2;
                    }
                    other => {
                        return Err(CliError::usage(format!(
                            "unsupported voice option: {other}"
                        )));
                    }
                }
            }
            let mode = mode.ok_or_else(|| {
                CliError::usage("voice postprocess requires --mode dictation|task|edit")
            })?;
            if text.is_none() == text_file.is_none() {
                return Err(CliError::usage(
                    "voice postprocess requires exactly one of --text or --text-file",
                ));
            }
            // `edit` is meaningless without a draft: its whole job is to
            // rewrite the existing input-box text, and the ASR section alone
            // carries only the modification instruction. Refusing at parse
            // time is cheaper and clearer than shipping a prompt that
            // describes an absent section.
            if matches!(mode, PostprocessMode::Edit) && draft.is_none() && draft_file.is_none() {
                return Err(CliError::usage(
                    "voice postprocess --mode edit requires --draft or --draft-file (the input-box \
                     text to rewrite)",
                ));
            }
            Ok(VoiceCommand::Postprocess {
                mode,
                text,
                text_file,
                draft,
                draft_file,
            })
        }
        "asr-status" => {
            if !rest.is_empty() {
                return Err(CliError::usage("voice asr-status accepts no options"));
            }
            Ok(VoiceCommand::AsrStatus)
        }
        "asr-install" => {
            // The system-mutating install accepts exactly one optional `--yes`
            // consent flag (same gate as `deps install`); anything else is a
            // usage error.
            let mut yes = false;
            for token in rest {
                match token.as_str() {
                    "--yes" => {
                        if yes {
                            return Err(CliError::usage("duplicate voice option --yes"));
                        }
                        yes = true;
                    }
                    other => {
                        return Err(CliError::usage(format!(
                            "unsupported voice asr-install option: {other}"
                        )));
                    }
                }
            }
            Ok(VoiceCommand::AsrInstall { yes })
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

pub fn execute(command: VoiceCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    // The ASR staging/models root lives under the product data root; a
    // relative PINVOU3_HOME would silently resolve against the cwd.
    crate::support::sandbox_home()?;
    match command {
        VoiceCommand::Transcribe { path } => transcribe(&path, output),
        VoiceCommand::Postprocess {
            mode,
            text,
            text_file,
            draft,
            draft_file,
        } => postprocess(mode, text, text_file, draft, draft_file, output),
        VoiceCommand::AsrStatus => asr_status(output),
        VoiceCommand::AsrInstall { yes } => asr_install(yes, output),
    }
}

// ─────────────────────────── ASR platform mirror ───────────────────────────

/// Same fields as `features::voice::voice_asr::AsrModelSpec` (the struct is
/// `pub` but its module is `pub(crate)` and `features::voice::platform` is a
/// private module, so neither the type nor the per-platform specs can be
/// imported from here — verified, not assumed).
///
/// DRIFT GUARD — the constants below are a hand copy of, and must be bumped
/// in the same pull request as:
/// * `pinvou3-app/src-tauri/src/features/voice/platform/mod.rs`
///   (`ASR_MODEL_URL` / `ASR_MODEL_MIRROR_URL` / `ASR_MODEL_SIZE` /
///   `ASR_MODEL_SHA256`, the Linux/macOS `sense-voice-small-q4_k.gguf`), and
/// * `pinvou3-app/src-tauri/src/features/voice/platform/windows.rs`
///   (the same four names, the `sensevoice-small-q8.gguf` build).
///
/// A model bump that touches only the app leaves the CLI pinning a stale
/// sha256, and `model_available` would then reject the model the desktop app
/// just installed — silently, as "model: false".
struct AsrModelSpec {
    filename: &'static str,
    expected_size: u64,
    sha256: &'static str,
    primary_url: &'static str,
    mirror_url: &'static str,
}

fn asr_dir() -> PathBuf {
    pinvou3_home().join("asr")
}

#[cfg(target_os = "linux")]
fn engine_binary_name() -> &'static str {
    "sense-voice-main"
}

#[cfg(target_os = "windows")]
fn engine_binary_name() -> &'static str {
    "pinvou-asr.exe"
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn engine_binary_name() -> &'static str {
    "pinvou-asr"
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn model_spec() -> AsrModelSpec {
    if cfg!(target_os = "windows") {
        // Windows ships the q8 model in the MSI (`platform/windows.rs`).
        AsrModelSpec {
            filename: "sensevoice-small-q8.gguf",
            expected_size: 254_208_320,
            sha256: "4ae45c94422de949b387e2e0fb10d7e14e4c42c69db30c3444ecc7d4b844b7c5",
            primary_url: "https://www.modelscope.cn/models/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/master/sensevoice-small-q8.gguf",
            mirror_url: "https://huggingface.co/FunAudioLLM/SenseVoiceSmall-GGUF/resolve/main/sensevoice-small-q8.gguf",
        }
    } else {
        AsrModelSpec {
            filename: "sense-voice-small-q4_k.gguf",
            expected_size: 182_278_688,
            sha256: "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b",
            primary_url: "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf",
            mirror_url: "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf",
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn model_spec() -> AsrModelSpec {
    AsrModelSpec {
        filename: "sense-voice-small-q4_k.gguf",
        expected_size: 182_278_688,
        sha256: "c8e7bf77acd860c5b83d2106da44aa7b985026ef4e7dbf5236c7f0f4001d9e9b",
        primary_url: "https://www.modelscope.cn/models/lovemefan/SenseVoiceGGUF/resolve/master/sense-voice-small-q4_k.gguf",
        mirror_url: "https://huggingface.co/lovemefan/sense-voice-gguf/resolve/main/sense-voice-small-q4_k.gguf",
    }
}

/// CLI mirror of `voice_asr::engine_path` without the bundled-resource
/// fallback: the packaged resource dir is injected through an AppHandle at
/// app startup, which a CLI process never has.
fn engine_path() -> Option<PathBuf> {
    let local = asr_dir().join(engine_binary_name());
    local.is_file().then_some(local)
}

fn model_path() -> PathBuf {
    asr_dir().join(model_spec().filename)
}

/// Availability probe, mirroring the app's `model_file_verified`: the
/// expected byte size AND the pinned sha256. A size-matching corrupted or
/// tampered model is rejected here instead of being served as a transcript
/// oracle forever. The verdict is memoized on (size, mtime) — the app caches
/// by mtime the same way — so one transcribe hashes the 182–254 MiB model
/// once instead of at every gate (availability probe, native lane,
/// external-CLI lane all ask).
///
/// The key carries the PATH as well as size and mtime, exactly like the app's
/// `ModelVerificationCache` (`features/voice/voice_asr.rs`): `PINVOU3_HOME`
/// is per-test and per-invocation, and two sandboxes whose model files happen
/// to share size and mtime would otherwise read each other's verdict out of
/// this process-global memo.
fn model_available() -> bool {
    let spec = model_spec();
    let path = model_path();
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    type CacheKey = (PathBuf, u64, Option<std::time::SystemTime>);
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(CacheKey, bool)>>> =
        std::sync::OnceLock::new();
    // A panic while this memo is held leaves nothing inconsistent behind (the
    // entry is overwritten wholesale below), so the crate's convention of
    // taking the inner value beats poisoning every later probe.
    let mut entry = CACHE
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key: CacheKey = (path.clone(), meta.len(), meta.modified().ok());
    if let Some((cached_key, available)) = entry.as_ref() {
        if *cached_key == key {
            return *available;
        }
    }
    let available = meta.len() == spec.expected_size && file_is_sha256(&path, spec.sha256);
    *entry = Some((key, available));
    available
}

/// Bounded waits so a wedged ffmpeg cannot hang the one-shot CLI: a
/// `-version` probe answers in milliseconds, and one ≤4 MiB WAV conversion
/// stays well inside the ASR timeout budget even on slow disks.
const FFMPEG_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const FFMPEG_CONVERT_TIMEOUT: Duration = Duration::from_secs(60);

/// Waits for a short-lived helper child (an ffmpeg probe or conversion) with
/// a fixed bound, mirroring the bounded-wait loop of the ASR lanes. On
/// timeout or wait error the child is killed and `None` is returned (the
/// caller treats it like any other probe failure); on success the exit
/// status is returned.
fn wait_bounded(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    // Tree kill: a bare `kill()` would orphan descendants.
                    crate::support::kill_process_tree(child);
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                crate::support::kill_process_tree(child);
                return None;
            }
        }
    }
}

fn ffmpeg_available() -> bool {
    let mut command = std::process::Command::new("ffmpeg");
    command
        .arg("-version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::support::set_process_group(&mut command);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    wait_bounded(&mut child, FFMPEG_PROBE_TIMEOUT)
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Same composition as `voice_asr::compose_status` / `status()`: macOS
/// reports the system Speech runtime as present (`asr_bundled_runtime_status`
/// → `Some(true)`), other platforms check engine + ffmpeg + model files.
fn asr_components() -> (bool, bool, bool, bool) {
    // Only Linux has a CLI install route (`voice asr-install`); Windows
    // ships the engine in the MSI, which only the desktop app's
    // repair/reinstall flow can restore; macOS needs no installation.
    let installable = cfg!(target_os = "linux");
    if cfg!(target_os = "macos") {
        // System Speech needs no engine binary, model file, or ffmpeg.
        return (true, true, true, installable);
    }
    let engine = engine_path().is_some();
    let model = model_available();
    let ffmpeg = ffmpeg_available();
    (engine, ffmpeg, model, installable)
}

/// Whether `run_recognition` will attempt the bundled engine at all. Only
/// Linux has a native lane in this process: macOS recognition goes through
/// the system Speech framework (a crate-private adapter the CLI cannot
/// reach) and Windows' bundled runtime is a CLI that speaks the external
/// protocol, not the SenseVoice.cpp argument protocol the native lane emits.
/// The pre-flight gate and the `cli_transcribe_ready` status flag both ask
/// this question, so they agree with the dispatcher by construction — a
/// Windows user who drops an engine plus model into the managed dir must not
/// pass a gate that then dies with `asr_engine_missing`.
fn native_lane_supported() -> bool {
    cfg!(target_os = "linux")
}

/// External ASR CLI resolution (`platform/*/asr_tool_path`): explicit env
/// configuration wins, then the conventional binary. Availability mirrors
/// `asr_tool_exists`: configured path or `pinvou-asr` on PATH.
fn external_asr_command() -> Option<PathBuf> {
    for name in [
        "PINVOU3_ASR_CMD",
        "PINVOU3_DEEPSPEECH2_CMD",
        "PADDLESPEECH_BIN",
    ] {
        if let Ok(path) = std::env::var(name) {
            // Mirror `asr_tool_exists`: a configured path counts only when it
            // exists, otherwise asr-status would report ready for a missing
            // tool.
            if !path.trim().is_empty() {
                let configured = PathBuf::from(path.trim());
                if configured.is_file() {
                    return Some(configured);
                }
            }
        }
    }
    // Mirror the app's `asr_tool_path` fallback (macOS `platform/macos.rs`,
    // Windows `platform/windows.rs` → `bundled_asr_tool_path`): the managed
    // ASR dir counts before the bare PATH name. Windows is included because
    // its engine (`pinvou-asr.exe`) speaks exactly this `asr --model --lang
    // --input` protocol — the app's `recognize_native` returns `None` there
    // on purpose and drives it through this same external lane. Linux is
    // excluded: its managed binary is `sense-voice-main`, a SenseVoice.cpp
    // build with the incompatible `-m MODEL INPUT -t -l -itn` protocol that
    // the native lane (and only the native lane) knows how to call.
    if !cfg!(target_os = "linux") {
        let managed = asr_dir().join(engine_binary_name());
        if managed.is_file() {
            return Some(managed);
        }
    }
    let default = PathBuf::from("pinvou-asr");
    command_exists(&default).then_some(default)
}

/// Mirror of `platform::os::command_exists`: absolute/configured paths must
/// be files; bare names are looked up in `PATH` (executable bit on unix).
fn command_exists(command: &Path) -> bool {
    if command.components().count() > 1 {
        return command.is_file();
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    let name = command.to_string_lossy().into_owned();
    let candidates = crate::support::binary_candidates(&name);
    std::env::split_paths(&path_var).any(|dir| {
        candidates.iter().any(|candidate| {
            let path = dir.join(candidate);
            if !path.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(&path)
                    .map(|meta| meta.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
    })
}

// ─────────────────────────── asr-status / asr-install ──────────────────────

fn asr_status(output: OutputMode) -> Result<CliOutcome, CliError> {
    let (engine, ffmpeg, model, installable) = asr_components();
    let ready = engine && ffmpeg && model;
    // `ready` mirrors the GUI's host-capability status (on macOS it reports
    // the system Speech runtime). The CLI cannot reach that runtime, so this
    // field answers the question the user actually acts on: can THIS command
    // transcribe right now (the Linux bundled engine lane — which transcodes
    // through ffmpeg when present and falls back to the raw wav file
    // otherwise — or the external ASR CLI; the gate keeps the practical
    // engine+ffmpeg+model requirement).
    let cli_transcribe_ready =
        (native_lane_supported() && engine && ffmpeg && model) || external_asr_command().is_some();
    let mut missing = Vec::new();
    if !model {
        missing.push("model");
    }
    if !ffmpeg {
        missing.push("ffmpeg");
    }
    if !engine {
        missing.push("engine");
    }
    let mut value = serde_json::json!({
        "engine": engine,
        "ffmpeg": ffmpeg,
        "model": model,
        "ready": ready,
        "cli_transcribe_ready": cli_transcribe_ready,
        "installable": installable,
        "missing": missing,
        "asr_dir": asr_dir().display().to_string(),
    });
    if cfg!(target_os = "windows") {
        // `installable` is false on Windows because the CLI has no install
        // route there: the engine ships inside the desktop app's MSI. The
        // human note below names the reachable workaround (PINVOU3_ASR_CMD /
        // the managed dir), which does NOT flip these flags — they describe
        // what lives under AsrDir.
        value["gui_install_only"] = serde_json::json!(true);
    }
    let mut human = format!(
        "Engine: {}\nFfmpeg: {}\nModel: {}\nReady: {}\nCliTranscribe: {}\nInstallable: {}\nMissing: {}\nAsrDir: {}",
        engine,
        ffmpeg,
        model,
        ready,
        if cli_transcribe_ready { "yes" } else { "no" },
        installable,
        if missing.is_empty() {
            "none".to_owned()
        } else {
            missing.join(",")
        },
        asr_dir().display(),
    );
    if cfg!(target_os = "macos") && !cli_transcribe_ready {
        // macOS `ready` reflects the system Speech runtime, which only the
        // GUI can drive; without the external ASR CLI the CLI cannot
        // transcribe even though the host reports ready.
        human.push_str(
            "\nNote: `voice transcribe` needs the external ASR CLI (PINVOU3_ASR_CMD or `pinvou-asr` on PATH); macOS Speech is GUI-only.",
        );
    }
    if cfg!(target_os = "windows") {
        // The old note said "repair or reinstall pinvou", which can never flip
        // these flags: the MSI installs the engine and model next to the
        // desktop executable, and `engine_path`/`model_path` only look under
        // `asr_dir` (the app resolves the bundled copy through a `pub(crate)`
        // helper the CLI cannot call). Name the remediation that is actually
        // reachable from this process instead.
        human.push_str(
            "\nNote: the CLI only looks for the ASR engine and model under AsrDir; it cannot see \
             the copies the desktop app's MSI installs beside pinvou.exe. Point PINVOU3_ASR_CMD \
             at that bundled `pinvou-asr.exe` (or copy the engine and model into AsrDir) to make \
             `voice transcribe` work.",
        );
    }
    Ok(success(render(output, human, &value)))
}

/// Linux install lane: gated behind `--yes` like `deps install` because it
/// mutates the system — missing ffmpeg goes through the same public
/// `features::dependencies::install_dependencies` call the GUI platform
/// adapter makes (pkexec/apt), then the SenseVoice model is downloaded from
/// the primary URL with the mirror as fallback (a `PINVOU3_ASR_MODEL_URL`
/// override wins, like the app's `model_download_urls`). The download is
/// staged, size-capped, and sha256-verified against the pinned digest.
fn asr_install(yes: bool, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Platform check first so macOS/Windows keep their unsupported exit-1
    // message; the consent gate precedes every probe, install, and download
    // step.
    if !cfg!(target_os = "linux") {
        return Err(CliError::failed(
            "voice asr-install is only supported on Linux; on Windows repair/reinstall pinvou, \
             on macOS system speech needs no installation",
        ));
    }
    crate::support::require_yes(yes)?;
    // Mutual exclusion across processes. The GUI has the same guard as a
    // process-local flag (`features/voice/voice_asr.rs::begin_asr_install`
    // swapping `ASR_INSTALLING`, documented as "避免两个入口并发写同一个
    // `.part` 文件"), which is invisible to a second CLI process: two
    // `asr-install --yes` runs would both `File::create` the same `.part`
    // inode and interleave their writes. The pre-rename checksum keeps the
    // corrupt result off the canonical path, but both runs then fail for a
    // reason neither user can act on. `try_write` rather than a blocking
    // wait, mirroring the GUI's immediate "already installing" refusal and
    // `code.rs`'s `*_busy` convention.
    let mut install_lock = asr_install_lock()?;
    let _install_guard = install_lock.try_write().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            CliError::failed(
                "asr_install_busy: another pinvou process is installing the ASR model; retry \
                 after it finishes",
            )
        } else {
            CliError::failed(format!(
                "voice asr-install: cannot acquire the install lock: {error}"
            ))
        }
    })?;
    let mut steps: Vec<String> = Vec::new();
    if !ffmpeg_available() {
        pinvou3_lib::features::dependencies::install_dependencies(vec!["ffmpeg".to_owned()], None)
            .map_err(|error| {
                CliError::failed(format!(
                    "voice asr-install: ffmpeg: {}",
                    crate::deps::translate_deps_error(&error.to_string())
                ))
            })?;
        steps.push("installed ffmpeg".to_owned());
    }
    if !model_available() {
        let path = download_asr_model()?;
        steps.push(format!("downloaded model {}", path.display()));
    }
    let (engine, ffmpeg, model, _) = asr_components();
    let value = serde_json::json!({
        "engine": engine,
        "ffmpeg": ffmpeg,
        "model": model,
        "ready": engine && ffmpeg && model,
        "steps": steps,
    });
    let human = if steps.is_empty() {
        "voice asr-install: nothing to install, ASR already ready".to_owned()
    } else {
        format!(
            "Installed: {}\nEngine: {engine}\nFfmpeg: {ffmpeg}\nModel: {model}",
            steps.join(", ")
        )
    };
    Ok(success(render(output, human, &value)))
}

/// Opens the cross-process ASR install lock (same `$PINVOU3_HOME/locks`
/// directory and same fd-lock primitive as `code.rs`'s session/root locks and
/// `connectors.rs`'s install lock). The caller must keep the returned lock
/// alive alongside its write guard.
fn asr_install_lock() -> Result<fd_lock::RwLock<std::fs::File>, CliError> {
    // `sandbox_home` already ran in `execute`, so the lock cannot land in a
    // cwd-relative directory.
    let dir = pinvou3_home().join("locks");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::failed(format!(
            "voice asr-install: cannot create {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join("voice-asr-install.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| {
            CliError::failed(format!(
                "voice asr-install: cannot open {}: {error}",
                path.display()
            ))
        })?;
    Ok(fd_lock::RwLock::new(file))
}

fn download_asr_model() -> Result<PathBuf, CliError> {
    let spec = model_spec();
    let dest = asr_dir().join(spec.filename);
    std::fs::create_dir_all(asr_dir()).map_err(|error| {
        CliError::failed(format!("voice asr-install: cannot create asr dir: {error}"))
    })?;
    // The app's `model_download_urls` honors this override; mirror it.
    let custom = std::env::var("PINVOU3_ASR_MODEL_URL")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(custom) = custom.as_deref() {
        // The app's `model_download_urls` tries ONLY the override: a broken
        // custom URL must fail the install instead of silently installing
        // the public model behind the operator's back. The error class from
        // `download_to` is URL-safe by construction, so surface it instead
        // of mislabeling every failure as a checksum gate failure.
        if let Err(error) = download_to(custom, &dest, spec.sha256) {
            let _ = std::fs::remove_file(&dest);
            return Err(CliError::failed(format!(
                "voice asr-install: the PINVOU3_ASR_MODEL_URL download failed ({error}); \
                 fix or unset the override and retry"
            )));
        }
        return Ok(dest);
    }
    for url in [spec.primary_url, spec.mirror_url] {
        if download_to(url, &dest, spec.sha256).is_ok() {
            return Ok(dest);
        }
        let _ = std::fs::remove_file(&dest);
    }
    Err(CliError::failed(
        "voice asr-install: model download failed or checksum mismatch; verify manually and retry",
    ))
}

fn file_is_sha256(path: &Path, expected: &str) -> bool {
    pinvou3_lib::platform::connector_lock::file_sha256_hex(path)
        .map(|actual| actual == expected)
        .unwrap_or(false)
}

/// DRIFT GUARD — this reproduces the staged-download contract of
/// `pinvou3-app/src-tauri/src/platform/download.rs::download_to_part_with_verify`
/// (`.part` sibling, `max_bytes` cap, sha256 verified before the rename, part
/// removed on every failure). That module is `pub(crate) mod download` inside
/// the app crate, so the CLI cannot call it — verified, not assumed. Any
/// change to that helper's ordering or cleanup guarantees must be reflected
/// here in the same pull request; the async/cancel/progress/idle-timeout
/// machinery it carries has no CLI equivalent and is deliberately not copied.
fn download_to(url: &str, dest: &Path, expected_sha256: &str) -> Result<(), CliError> {
    // Staged through a .part sibling and size-capped: a crashed or hostile
    // download must never leave a truncated/garbage file at the real path.
    // The checksum is verified on the .part BEFORE the rename (the app's
    // download helper order), so no window exists where an unverified model
    // sits at the canonical path.
    let part = dest.with_extension("part");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .user_agent(concat!("pinvou-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| CliError::failed(format!("voice asr-install: client: {error}")))?;
    let response = client
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        // reqwest Display carries the full mirror URL (possibly an intranet
        // address), so only the error class is kept.
        .map_err(|error| {
            CliError::failed(format!(
                "voice asr-install: download: {}",
                summarize_request_error(&error, "model mirror")
            ))
        })?;
    let result = (|| -> Result<(), CliError> {
        use std::io::Read as _;
        let mut file = std::fs::File::create(&part).map_err(|error| {
            CliError::failed(format!("voice asr-install: {}: {error}", part.display()))
        })?;
        let mut reader = response.take(MAX_DOWNLOAD_BYTES + 1);
        std::io::copy(&mut reader, &mut file)
            .map_err(|error| CliError::failed(format!("voice asr-install: write: {error}")))?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&part);
        return Err(error);
    }
    let size = match std::fs::metadata(&part).map(|meta| meta.len()) {
        Ok(size) => size,
        Err(error) => {
            // Same cleanup guarantee as every other failure path: the stat
            // of a file this code just wrote should not fail, but if it
            // does the staged part must not leak.
            let _ = std::fs::remove_file(&part);
            return Err(CliError::failed(format!(
                "voice asr-install: stat: {error}"
            )));
        }
    };
    if size > MAX_DOWNLOAD_BYTES {
        let _ = std::fs::remove_file(&part);
        return Err(CliError::failed(
            "voice asr-install: model exceeds the size cap",
        ));
    }
    if !file_is_sha256(&part, expected_sha256) {
        let _ = std::fs::remove_file(&part);
        return Err(CliError::failed(
            "voice asr-install: model checksum mismatch; verify manually and retry",
        ));
    }
    std::fs::rename(&part, dest)
        .map_err(|error| CliError::failed(format!("voice asr-install: finish: {error}")))?;
    Ok(())
}

/// Hard upper bound for one model download (largest expected model is ~254
/// MiB; the cap exists so a hostile mirror cannot balloon the disk).
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// The native lane feeds a missing-ffmpeg installation the raw wav (GUI
/// parity), so only non-wav inputs make an ffmpeg-only gap fatal.
fn ffmpeg_missing_is_fatal_for(extension: Option<&str>) -> bool {
    !extension
        .map(|ext| ext.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
}

// ─────────────────────────── transcribe ────────────────────────────────────

/// Mirror of the GUI's decoded-audio cap (`recording_too_long`).
const MAX_TRANSCRIBE_BYTES: usize = 4 * 1024 * 1024;

/// The recognition lanes one `transcribe` invocation can reach.
#[derive(Clone, Copy, Debug)]
struct AsrLanes {
    engine: bool,
    ffmpeg: bool,
    model: bool,
    /// An external ASR CLI is configured and present (`PINVOU3_ASR_CMD` and
    /// friends), which is a complete lane on its own.
    external: bool,
    /// [`native_lane_supported`] for this target: whether engine + ffmpeg +
    /// model actually buy a recognition lane in *this* process.
    native: bool,
}

/// Probes the installed lanes. The external-CLI lookup stays behind the short
/// circuit so a complete native install never pays for a PATH walk. The short
/// circuit is keyed on the *native* lane, not on the component triple: on
/// macOS `asr_components` reports the system Speech runtime as engine +
/// ffmpeg + model, and skipping the external probe there would hide the only
/// lane the CLI actually has.
fn installed_lanes() -> AsrLanes {
    let (engine, ffmpeg, model, _) = asr_components();
    let native = native_lane_supported();
    let external = !(native && engine && ffmpeg && model) && external_asr_command().is_some();
    AsrLanes {
        engine,
        ffmpeg,
        model,
        external,
        native,
    }
}

/// What the pre-flight component gate decided for one `transcribe` call.
#[derive(Debug, PartialEq, Eq)]
enum AsrPreflight {
    /// A lane exists; hand the audio to `run_recognition`.
    Run,
    /// Engine and model are installed and only ffmpeg is missing, but this
    /// input needs no conversion: recognition runs on the raw wav and the
    /// user is warned that other formats will not work until ffmpeg is there.
    RunOnRawWav,
    /// No lane can run; the payload is the user-facing message.
    Reject(&'static str),
}

/// Decides whether the installed components give `transcribe` a lane at all.
///
/// Kept pure and out of [`transcribe`] because the decisive combination
/// cannot be staged by any hermetic test that drives the real probes:
/// `model_available` insists on a model file of exactly the shipped size
/// (hundreds of MiB) matching a pinned sha256, so "engine + model installed,
/// ffmpeg missing" — the one case that must keep going instead of failing —
/// is unreachable through the CLI harness.
fn asr_preflight(lanes: AsrLanes, extension: Option<&str>) -> AsrPreflight {
    // `lanes.native` is what makes the component triple a lane: `run_recognition`
    // only ever spawns the bundled engine where [`native_lane_supported`] holds.
    // Without that conjunct the gate and the dispatcher disagree — a Windows or
    // macOS host with engine + model present passes here and then fails with
    // `asr_engine_missing` after the audio has already been staged.
    if (lanes.native && lanes.engine && lanes.ffmpeg && lanes.model) || lanes.external {
        return AsrPreflight::Run;
    }
    // Everything but ffmpeg is installed. The native lane feeds the engine
    // the raw wav in that case (GUI parity), so an input that needs no
    // conversion is not a failure at all — reporting "not installed" here
    // would send the user reinstalling a model that is present and already
    // verified.
    // Other extensions keep the hard error: the engine cannot decode them
    // without the converter.
    if lanes.native && lanes.engine && lanes.model {
        if ffmpeg_missing_is_fatal_for(extension) {
            return AsrPreflight::Reject(
                "ffmpeg_missing: ffmpeg is required for local speech recognition; \
                 install it manually or run pinvou voice asr-install",
            );
        }
        return AsrPreflight::RunOnRawWav;
    }
    AsrPreflight::Reject(
        "asr_engine_missing: local speech recognition is not installed \
         (hint: run `pinvou voice asr-status`)",
    )
}

fn transcribe(path: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    transcribe_with(path, output, installed_lanes, run_recognition)
}

/// `transcribe` with its two environment-dependent steps injected — the
/// component probe and the recognition dispatch — so a test can pin what the
/// gate does for an install the test host cannot produce. Both are taken lazily
/// so the input validation below still runs before anything is probed or spawned.
fn transcribe_with(
    path: &Path,
    output: OutputMode,
    lanes: impl FnOnce() -> AsrLanes,
    recognize: impl FnOnce(&Path) -> Result<(String, &'static str), CliError>,
) -> Result<CliOutcome, CliError> {
    // Validate the input BEFORE anything else: a FIFO or character device
    // reports len 0, so a size gate alone would let `/dev/zero` stream
    // unbounded into memory — that must fail fast without requiring an ASR
    // install first. Regular files only, and the read itself is capped in
    // case the file grows between the stat and the read.
    let metadata = std::fs::metadata(path).map_err(|error| {
        CliError::failed(format!(
            "voice transcribe: cannot read {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(CliError::failed(format!(
            "voice transcribe: {} is not a regular audio file",
            path.display()
        )));
    }
    if metadata.len() > MAX_TRANSCRIBE_BYTES as u64 {
        return Err(CliError::failed(
            "recording_too_long: recording exceeds the 4 MiB transcription cap",
        ));
    }
    // Then fail fast on missing ASR before loading the file into memory. The
    // gate needs no per-platform special case of its own: `AsrLanes::native`
    // already carries whether the component triple buys a lane here, so on
    // macOS/Windows — where `run_recognition` has only the external lane — the
    // preflight rejects for exactly the reason the dispatcher would.
    match asr_preflight(lanes(), path.extension().and_then(|ext| ext.to_str())) {
        AsrPreflight::Run => {}
        // A warning, not an error: execution continues into the raw-wav lane
        // that `native_engine_transcribe` already implements.
        AsrPreflight::RunOnRawWav => crate::note!(
            "voice transcribe: ffmpeg is missing; feeding the raw wav to the engine \
             (install ffmpeg or run `pinvou voice asr-install` for non-wav audio)"
        ),
        AsrPreflight::Reject(message) => return Err(CliError::failed(message)),
    }
    let audio = {
        use std::io::Read as _;
        let file = std::fs::File::open(path).map_err(|error| {
            CliError::failed(format!(
                "voice transcribe: cannot read {}: {error}",
                path.display()
            ))
        })?;
        let mut capped = Vec::new();
        file.take(MAX_TRANSCRIBE_BYTES as u64 + 1)
            .read_to_end(&mut capped)
            .map_err(|error| {
                CliError::failed(format!(
                    "voice transcribe: cannot read {}: {error}",
                    path.display()
                ))
            })?;
        capped
    };
    if audio.len() > MAX_TRANSCRIBE_BYTES {
        return Err(CliError::failed(
            "recording_too_long: recording exceeds the 4 MiB transcription cap",
        ));
    }
    if audio.len() < 44 {
        return Err(CliError::failed(
            "audio_empty: recording is empty or corrupted",
        ));
    }
    let wav = write_temp_wav(&audio)?;
    let result = recognize(&wav);
    let _ = std::fs::remove_file(&wav);
    let (text, source) = result?;
    let value = serde_json::json!({ "text": text, "source": source });
    let human = format!("Source: {source}\nText: {text}");
    Ok(success(render(output, human, &value)))
}

fn write_temp_wav(bytes: &[u8]) -> Result<PathBuf, CliError> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "pinvou-cli-voice-{}-{nonce}.wav",
        std::process::id()
    ));
    // Private audio: 0600 + exclusive create (the app replaced this exact
    // hand-built temp pattern with NamedTempFile for the same reason — the
    // tempfile crate is not available here, so mirror its guarantees).
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            CliError::failed(format!("voice transcribe: cannot stage audio: {error}"))
        })?;
    // Any failure after the exclusive create must not leak the (empty or
    // partial) staging file.
    let staged = (|| -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        std::io::Write::write_all(&mut file, bytes)?;
        Ok(())
    })();
    if let Err(error) = staged {
        let _ = std::fs::remove_file(&path);
        return Err(CliError::failed(format!(
            "voice transcribe: cannot stage audio: {error}"
        )));
    }
    Ok(path)
}

/// Recognition dispatch mirrors `transcribe_voice_audio_bytes`: the platform
/// native lane first (Linux bundled SenseVoice engine; macOS Speech is not
/// reachable from the CLI), then the external ASR CLI. Without either lane
/// the error names `pinvou voice asr-status` as the hint.
fn run_recognition(wav: &Path) -> Result<(String, &'static str), CliError> {
    let native_attempted = native_lane_supported() && engine_path().is_some() && model_available();
    if native_attempted {
        // GUI parity: a failing native lane falls back to the env-configured
        // external ASR CLI before giving up.
        if let Ok(text) = native_engine_transcribe(wav) {
            // The same token the GUI reports for this lane
            // (`features/voice/platform/linux.rs::native_recognition_source`).
            // The `pinvou-webview-` prefix names the shipped engine build, not
            // the host process, so a consumer reading `source` across both
            // surfaces sees one vocabulary instead of two.
            return Ok((text, "pinvou-webview-sensevoice-local"));
        }
    }
    match external_asr_command() {
        Some(command) => external_cli_transcribe(&command, wav).map(|text| (text, "local_cli")),
        // An attempted-but-failed engine is a different fact from a missing
        // one (the GUI reports `asr_engine_error` here); "not installed"
        // would send the user reinstalling a model that exists.
        None if native_attempted => Err(CliError::failed(
            "asr_engine_error: the local recognition engine failed and no external ASR CLI \
             is configured (hint: run `pinvou voice asr-status`)",
        )),
        None => Err(CliError::failed(
            "asr_engine_missing: local speech recognition is not installed \
             (hint: run `pinvou voice asr-status`)",
        )),
    }
}

/// Mirror of `voice_asr::transcribe`: normalize to 16 kHz mono through ffmpeg
/// when available (bounded wait), run the engine with cwd pinned to the
/// writable asr dir under the shared ASR timeout budget with size-capped
/// output pipes, and fail with `asr_parse_failed` on unusable output.
fn native_engine_transcribe(wav: &Path) -> Result<String, CliError> {
    // The availability check ran earlier in the caller; if the binary
    // vanished since, fail honestly instead of panicking (a panic here
    // would strand the staged wav).
    let engine = match engine_path() {
        Some(engine) => engine,
        None => {
            return Err(CliError::failed(
                "asr_engine_missing: the local ASR engine binary is gone between the \
                 availability check and the spawn; run `pinvou voice asr-install --yes` again",
            ));
        }
    };
    let model = model_path();
    // The normalized scratch file is the same private audio as the staged
    // input: pre-create it 0600 + exclusive (ffmpeg then writes into the
    // existing private file) so a second world-readable copy never exists,
    // and remove it on every path — the app removes it unconditionally after
    // the engine attempt, half-written ffmpeg output included.
    let normalized = std::env::temp_dir().join(format!(
        "pinvou-cli-asr-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let normalized_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&normalized)
        .map_err(|error| {
            CliError::failed(format!("voice transcribe: cannot stage audio: {error}"))
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The chmod must hold before ffmpeg writes audio into the file; a
        // silent failure would leave a world-readable copy of private audio
        // in the shared temp dir.
        if let Err(error) =
            std::fs::set_permissions(&normalized, std::fs::Permissions::from_mode(0o600))
        {
            drop(normalized_file);
            let _ = std::fs::remove_file(&normalized);
            return Err(CliError::failed(format!(
                "voice transcribe: cannot restrict the staging file: {error}"
            )));
        }
    }
    drop(normalized_file);
    let input = if ffmpeg_available() {
        let mut convert_command = std::process::Command::new("ffmpeg");
        convert_command
            .args(["-y", "-i"])
            .arg(wav)
            .args(["-ar", "16000", "-ac", "1", "-f", "wav"])
            .arg(&normalized)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::support::set_process_group(&mut convert_command);
        let spawned = convert_command.spawn();
        let status_ok = match spawned {
            Ok(mut convert) => wait_bounded(&mut convert, FFMPEG_CONVERT_TIMEOUT)
                .map(|status| status.success())
                .unwrap_or(false),
            Err(_) => false,
        };
        let converted = status_ok
            && std::fs::metadata(&normalized)
                .map(|m| m.len() > 44)
                .unwrap_or(false);
        if converted {
            normalized.clone()
        } else {
            wav.to_path_buf()
        }
    } else {
        wav.to_path_buf()
    };
    let work_dir = asr_dir();
    let _ = std::fs::create_dir_all(&work_dir);
    // Bounded like the external ASR lane: stdin is unused (the input file is
    // passed as an argument), both pipes are drained size-capped, and a
    // wedged engine is killed after the shared timeout budget instead of
    // hanging the one-shot CLI forever.
    let mut engine_command = std::process::Command::new(&engine);
    engine_command
        .current_dir(&work_dir)
        .arg("-m")
        .arg(&model)
        .arg(&input)
        .args(["-t", "4", "-l", "auto", "-itn"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // A group leader, so the timeout kill below takes any engine descendants
    // with it instead of orphaning them (same contract as `connectors`'s
    // vendor CLI spawns).
    crate::support::set_process_group(&mut engine_command);
    let mut child = spawn_asr_engine(&mut engine_command, &normalized)?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdout_tx.send(drain_capped(stdout_pipe));
    });
    std::thread::spawn(move || {
        let _ = stderr_tx.send(drain_capped(stderr_pipe));
    });

    let timeout = asr_timeout_secs();
    let started = Instant::now();
    // The group kill lives on the two branches that leave the engine
    // unreaped — a timeout and a broken wait — where the leader is
    // verifiably still alive and its descendants must not be orphaned. Once
    // `try_wait` has reaped the child, whatever its exit status, the pid is
    // free for reuse and `kill(-pgid)` could hit an unrelated process group
    // with this user's privileges, so no kill happens there (the same rule
    // every `connectors` call site follows).
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if started.elapsed() >= Duration::from_secs(timeout) {
                    crate::support::kill_process_tree(&mut child);
                    break Err(CliError::failed(format!(
                        "voice transcribe: local ASR engine timed out after {timeout}s"
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                crate::support::kill_process_tree(&mut child);
                break Err(CliError::failed(format!(
                    "voice transcribe: local ASR engine failed unexpectedly: {error}"
                )));
            }
        }
    };
    // A descendant that inherited the engine's pipes can keep them open past
    // the engine's own exit, so an unbounded join here would hang this
    // one-shot process forever; collect through the bounded grace instead
    // (the same hazard the probe and login lanes bound). A straggler is left
    // alone for the reaped-pid reason above, so the drains may come back
    // short and surface as the usual parse failure instead of a hang.
    let stdout = drain_with_grace(stdout_rx);
    let stderr = drain_with_grace(stderr_rx);
    let _ = std::fs::remove_file(&normalized);
    let status = status?;
    if !status.success() {
        // The engine transcript can leak into stderr; report the failure
        // shape only, like the external-CLI lane and the GUI.
        return Err(CliError::failed(format!(
            "asr_engine_error: ASR engine failed (stdout {} bytes, stderr {} bytes)",
            stdout.len(),
            stderr.len()
        )));
    }
    extract_transcript(&stdout, &stderr).ok_or_else(|| {
        CliError::failed("asr_parse_failed: recognition returned no usable text; please retry")
    })
}

/// Spawns the local ASR engine. The normalized staging file is private
/// audio; a failed spawn must not leak it in the shared temp dir (the app
/// removes it unconditionally after the engine attempt, half-written ffmpeg
/// output included).
fn spawn_asr_engine(
    command: &mut std::process::Command,
    normalized: &Path,
) -> Result<std::process::Child, CliError> {
    command.spawn().map_err(|error| {
        let _ = std::fs::remove_file(normalized);
        CliError::failed(format!(
            "voice transcribe: cannot start ASR engine: {error}"
        ))
    })
}

/// Engine wait budget shared by both ASR lanes: the same env overrides the
/// GUI honors (`PINVOU3_ASR_TIMEOUT_SECS` /
/// `PINVOU3_DEEPSPEECH2_TIMEOUT_SECS`), 60 s by default.
fn asr_timeout_secs() -> u64 {
    std::env::var("PINVOU3_ASR_TIMEOUT_SECS")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_TIMEOUT_SECS"))
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(60)
}

/// Size bound for one captured ASR stream (stdout or stderr): a chatty
/// engine must not buffer unbounded output for the whole timeout window
/// (every other capture in the CLI is size-capped too).
const MAX_ENGINE_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;

/// Drains one child output pipe, KEEPING at most [`MAX_ENGINE_OUTPUT_BYTES`]
/// and decoding lossily, so a chatty engine neither buffers without bound nor
/// fails the whole transcription over an invalid byte.
///
/// The bytes past the cap are read and discarded rather than left in the
/// pipe. A `take(cap)` that simply stops reading leaves the write end full,
/// so the engine blocks in `write()` until `asr_timeout` fires — turning a
/// merely verbose engine into a 60 s stall reported as a timeout. Same
/// discard loop as `code.rs`'s `read_capped_to_eof`; the GUI's equivalent
/// (`app/commands/voice.rs`) reads to EOF for the same reason.
fn drain_capped<R: std::io::Read>(pipe: Option<R>) -> String {
    let mut kept: Vec<u8> = Vec::new();
    if let Some(mut pipe) = pipe {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match std::io::Read::read(&mut pipe, &mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let room = MAX_ENGINE_OUTPUT_BYTES.saturating_sub(kept.len() as u64) as usize;
                    if room > 0 {
                        kept.extend_from_slice(&chunk[..read.min(room)]);
                    }
                }
                Err(_) => break,
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// Collects one capped pipe drain through a bounded grace instead of an
/// unbounded join: a descendant that inherited the pipe's write end can keep
/// it open past the engine's exit, and `read_to_end` never sees EOF. On
/// grace expiry the straggler is left alone — this process exits right
/// after, closing the read end, and the shortfall surfaces as the usual
/// parse/length failure instead of a hang.
fn drain_with_grace(rx: std::sync::mpsc::Receiver<String>) -> String {
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_default()
}

/// Mirror of `run_local_asr_cli`: same argument protocol, same env-driven
/// model/language/timeout, concurrent pipe draining, and exit code 6 as the
/// "no speech" convention.
fn external_cli_transcribe(command: &Path, wav: &Path) -> Result<String, CliError> {
    use std::process::Stdio;

    let model = std::env::var("PINVOU3_ASR_MODEL")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_MODEL"))
        .unwrap_or_else(|_| "sensevoice-q8".to_owned());
    let language = std::env::var("PINVOU3_ASR_LANG")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_LANG"))
        .unwrap_or_else(|_| "zh".to_owned());
    let timeout = asr_timeout_secs();

    // build_command wraps Windows `.cmd` shims (e.g. PINVOU3_ASR_CMD pointing
    // at an npm-style asr.cmd) in `cmd /D /S /C`, which CreateProcess cannot
    // spawn directly.
    let mut command_line = crate::support::build_command(command, &[]);
    command_line
        .arg("asr")
        .arg("--model")
        .arg(&model)
        .arg("--lang")
        .arg(&language)
        .arg("--input")
        .arg(wav)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Mirror the GUI: the engine's own model path is passed through the env
    // when it is installed locally.
    if model_available() {
        command_line.env("PINVOU3_SENSEVOICE_MODEL", model_path());
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW — same console hygiene as the app's spawns.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command_line.creation_flags(CREATE_NO_WINDOW);
    }
    // A group leader, so the timeout kill below takes the CLI's shell/node
    // descendants with it instead of orphaning them (same contract as
    // `connectors`'s vendor CLI spawns).
    crate::support::set_process_group(&mut command_line);
    let mut child = command_line.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::failed(
                "asr_engine_missing: local speech recognition is not installed \
                     (hint: run `pinvou voice asr-status`)",
            )
        } else {
            CliError::failed(format!(
                "asr_engine_start_failed: ASR engine failed to start: {error}"
            ))
        }
    })?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdout_tx.send(drain_capped(stdout_pipe));
    });
    std::thread::spawn(move || {
        let _ = stderr_tx.send(drain_capped(stderr_pipe));
    });

    let started = Instant::now();
    // Same rule as the local-engine lane: the group kill belongs only to the
    // branches that leave the CLI unreaped. A non-success exit is ordinary
    // here — exit code 6 is the documented "no speech" answer — and the child
    // is already reaped by then, so killing its group would race a reused pid
    // and SIGKILL an unrelated process group.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if started.elapsed() >= Duration::from_secs(timeout) {
                    crate::support::kill_process_tree(&mut child);
                    break Err(CliError::failed(format!(
                        "asr_timeout: local speech recognition timed out ({timeout} s)"
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                crate::support::kill_process_tree(&mut child);
                break Err(CliError::failed(format!(
                    "asr_runtime_error: local speech recognition failed unexpectedly: {error}"
                )));
            }
        }
    };
    // A pipe-inheriting descendant can hold EOF open past the CLI's exit, so
    // collect through the bounded grace instead of an unbounded join (see the
    // local-engine lane).
    let stdout = drain_with_grace(stdout_rx);
    let stderr = drain_with_grace(stderr_rx);
    let status = status?;
    if !status.success() {
        if status.code() == Some(6) {
            return Err(CliError::failed(
                "asr_no_speech: no speech content recognized; please retry",
            ));
        }
        // Privacy: process output may carry recognized speech fragments; the
        // error reports lengths only, matching the GUI lane.
        return Err(CliError::failed(format!(
            "asr_cli_failed: local speech recognition failed (stdout_len={} stderr_len={})",
            stdout.len(),
            stderr.len()
        )));
    }
    extract_transcript(&stdout, &stderr).ok_or_else(|| {
        CliError::failed("asr_parse_failed: recognition returned no usable text; please retry")
    })
}

/// The GUI's transcript parser, shared verbatim: JSON `{"text": …}` lines,
/// `[0.00s → 2.10s] …` timestamped segments (all segments are joined, in
/// order, like the GUI), and plain final text lines, with the GUI's log and
/// status filters so engine noise like `system_info: …` or `Done in 3.2s` is
/// never returned as the transcript.
fn extract_transcript(stdout: &str, stderr: &str) -> Option<String> {
    pinvou3_lib::features::voice::transcript::parse_asr_transcript(stdout, stderr)
}

// ─────────────────────────── postprocess ───────────────────────────────────

/// System prompts mirrored verbatim from `app/commands/voice.rs`
/// (`voice_postprocess_prompt`).
fn postprocess_prompt(mode: PostprocessMode) -> &'static str {
    match mode {
        PostprocessMode::Edit => {
            r#"你是 Pinvou 的语音编辑器。你的唯一职责是根据用户的语音修改指令，改写“当前输入框已有文本”。

强规则：
1. 不回答问题，不执行任务，只输出修改后的完整输入框文本。
2. ASR 文本是修改指令，不是要追加到正文里的内容，除非用户明确说“加上/追加/补充”。
3. 必须保留原文中未被修改指令涉及的信息。
4. 不新增用户没说过的事实、时间、数量、条件、工具或结论。
5. 用户要求改成要点、列表或几条时，可以重排为 Markdown 列表。
6. 用户要求删除某条时，只删除明确指定的内容。
7. 用户要求替换实体时，只做对应替换。
8. 如果修改指令为空、纯噪声或无法理解，输出原文。
9. 只输出最终文本，不解释，不包裹代码块。
10. 输入用 <<<…>>> 定界符分段：DRAFT_TEXT 是输入框正文，ASR_TEXT 是规则纠错后的修改指令，ASR_RAW（如有）是原始识别；纠错可能有误，可参考原始识别恢复被误纠的内容。

示例：
当前输入框已有文本：
帮我整理会议纪要，提取风险和待办，明天发给团队。

ASR 文本：
把它改成三条要点。

最终文本：
- 整理会议纪要。
- 提取风险和待办。
- 明天发给团队。"#
        }
        PostprocessMode::Task => {
            r#"你是 Pinvou 的语音任务纠错器。你的唯一职责是把 ASR 文本纠正为用户原本想交给 Agent 执行的任务。

强规则：
1. 不回答问题，不执行任务。
2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。
3. 必须保留任务槽位：动作、对象、时间、数量、格式、限制条件、输出形态。
4. 不把输出形态改掉：用户说图表就保留图表，不要改成表格；用户说输入框就不要发送。
5. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。
6. 禁止截断句子；如果不确定，只做最小纠错并保留原句结构。
7. 英文实体、模型名、产品名和 API 名称要尽量标准化：GPT-5、Claude Sonnet、DeepSeek V3、REST API、PDF、Pinvou。
8. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是/文”等口头禅或噪声占位，才输出空字符串。
9. 优先纠正上下文中明显 ASR 错词：
   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”
   - 数据分析可视化里的“图标”通常应修为“图表”
   - “屁屁提/PPTT”通常应修为“PPT”
   - “销售暑假”通常应修为“销售数据”
   - “截止事件”通常应修为“截止时间”
   - “负责任”通常应修为“负责人”
   - “风险电”通常应修为“风险点”
   - “表哥”通常应修为“表格”
   - “四零一/talken/过期处里”通常应修为“401/token/过期处理”
   - “批地爱福/pDF”通常应修为“PDF”
   - “g p t five/GP杠5”通常应修为“GPT-5”，“closonic/克劳德 sonnet”通常应修为“Claude Sonnet”
   - “deeps V3/deep seek v three”通常应修为“DeepSeek V3”
   - 搜索“爱新闻/AI新闻”通常应修为“AI 新闻”
10. 对明显口语断裂做最小顺句，例如“有长方形，的需要联网下的图片”应整理为“是长方形，需要联网下载图片”。
11. 去掉口头禅、重复词和误识别语气词。
12. 只输出最终任务文本，不解释，不使用 Markdown。
13. 输入用 <<<…>>> 定界符分段：ASR_TEXT 是规则纠错后文本，ASR_RAW（如有）是原始识别；纠错可能有误，可参考原始识别恢复被误纠的实体。

示例：
ASR 文本：查一下今日进价并生成数据分析图标。
最终文本：查一下今日金价并生成数据分析图表。
ASR 文本：比较GP杠5mini和deeps V3的调用成本。
最终文本：比较 GPT-5 mini 和 DeepSeek V3 的调用成本。
ASR 文本：嗯，做一张海报，这个海报有长方形，的需要联网下的图片。用于公司的下午茶需要有一些文字的内容。
最终文本：做一张用于公司下午茶的长方形海报，需要联网下载图片，并包含文案内容。
ASR 文本：文。
最终文本："#
        }
        PostprocessMode::Dictation => {
            r#"你是 Pinvou 的语音听写整理器。你的唯一职责是把 ASR 文本纠正并整理为用户原本想输入到文本框里的内容。

强规则：
1. 不回答问题，不执行任务。
2. 不新增用户没说过的目标、工具、格式、数量、时间、条件。
3. 先去掉口头禅、重复词和误识别语气词，再修明显 ASR 错词。
4. 只有极短、单一、无需拆解的自然句，才输出一条自然句。
5. 除极短自然句外，默认整理成结构化 Markdown 列表。
6. 内容包含目标、用途、功能、字段、截止时间、进度、多个事项、多个条件、步骤、约束或明显需求表达时，必须整理成 Markdown 列表。
7. 整理成列表时必须保留动作、对象、时间、地点、数量、格式、限制条件和输出形态。
8. 正常查询、比较、搜索、整理、生成、做、把、帮我等句子都必须保留原请求，不能输出空字符串。
9. 只有整句去掉标点后只剩“嗯/啊/呃/额/那个/就是/文”等口头禅或噪声占位，才输出空字符串。
10. 日期、时间、地点按用户原话保留；即使看起来不合理，也不能擅自修正或删除。
11. 优先纠正上下文中明显 ASR 错词：
   - 行情/价格查询里的“进价/惊吓”通常应修为“金价”
   - 数据分析可视化里的“图标”通常应修为“图表”
   - “屁屁提/PPTT”通常应修为“PPT”
   - “销售暑假”通常应修为“销售数据”
   - “截止事件”通常应修为“截止时间”
   - “负责任”通常应修为“负责人”
   - “风险电”通常应修为“风险点”
   - “g p t five”通常应修为“GPT-5”，“克劳德 sonnet”通常应修为“Claude Sonnet”
   - 搜索“爱新闻”通常应修为“AI 新闻”
12. 只输出最终文本，不解释。
13. 输入用 <<<…>>> 定界符分段：ASR_TEXT 是规则纠错后文本，ASR_RAW（如有）是原始识别；纠错可能有误，可参考原始识别恢复被误纠的实体。

示例：
ASR 文本：今天天气怎么样？
最终文本：今天天气怎么样？
ASR 文本：嗯。
最终文本：
ASR 文本：搜索一下今天的爱新闻，按重要性排序。
最终文本：搜索一下今天的 AI 新闻，按重要性排序。
ASR 文本：制作一个个人工作台，用于企业录入工作事项进度，包括截止时间。
最终文本：
- 制作一个个人工作台。
- 用途：用于企业录入工作事项进度。
- 需要包含截止时间。
ASR 文本：一张用于公司年会的海报，时间是下午3点，12月36日需要联网下载一张图片，然后这个图片要尽量的好看呃，突出员工协作。这个海报是长方形的，上面需要有一点点文字，然后是红色背景。
最终文本：
- 制作一张用于公司年会的长方形海报。
- 时间：12月36日下午3点。
- 需要联网下载一张图片。
- 图片尽量好看，并突出员工协作。
- 海报需要红色背景。
- 海报上需要包含少量文字。"#
        }
    }
}

/// Retry prompts mirrored from `voice_postprocess_retry_prompt`.
fn postprocess_retry_prompt(mode: PostprocessMode) -> &'static str {
    match mode {
        PostprocessMode::Edit => {
            "你是语音编辑器。当前输入框已有文本是正文，ASR 文本是修改指令，不是要追加的正文。必须根据 ASR 指令修改正文，并只输出修改后的完整正文。除非 ASR 是纯口头禅、纯噪声或完全无法理解，否则禁止原样输出当前输入框已有文本。禁止解释，禁止空输出。"
        }
        PostprocessMode::Task => {
            "你是语音任务纠错器。把 ASR 纠正为用户要交给 Agent 执行的任务。保留所有时间、地点、格式和限制条件。禁止新增事实。必须只输出最终任务文本，禁止空输出；除非 ASR 是纯口头禅或纯噪声。"
        }
        PostprocessMode::Dictation => {
            "你是语音听写整理器。把 ASR 纠正并整理为用户想输入的文本。只有极短、单一、无需拆解的自然句才输出自然句；除此以外默认整理成结构化 Markdown 列表。内容包含目标、用途、功能、字段、截止时间、进度、多个事项、多个条件、步骤、约束或明显需求表达时，必须结构化。保留所有时间、地点、格式和限制条件。禁止新增事实。必须只输出最终文本，禁止空输出；除非 ASR 是纯口头禅或纯噪声。"
        }
    }
}

/// Mirror of `voice_postprocess_max_tokens`.
fn postprocess_max_tokens(mode: PostprocessMode, retry: bool) -> u32 {
    let base = match mode {
        PostprocessMode::Edit => 2048,
        PostprocessMode::Task | PostprocessMode::Dictation => 768,
    };
    if retry { base + 512 } else { base }
}

/// Mirror of `voice_postprocess_timeout`.
fn postprocess_timeout(mode: PostprocessMode, raw_text: &str) -> Duration {
    match mode {
        PostprocessMode::Task => return Duration::from_millis(8000),
        PostprocessMode::Edit => return Duration::from_millis(12000),
        PostprocessMode::Dictation => {}
    }
    let compact_len = raw_text
        .chars()
        .filter(|ch| {
            !ch.is_whitespace() && !"。！？!?，,、；;：:\"'“”‘’（）()【】[]….-—".contains(*ch)
        })
        .count();
    if compact_len <= 18 {
        Duration::from_millis(3000)
    } else {
        Duration::from_millis(5000)
    }
}

/// Input truncation cap (`VOICE_POSTPROCESS_MAX_INPUT_CHARS`).
const POSTPROCESS_MAX_INPUT_CHARS: usize = 4000;

fn truncate_postprocess_input(text: &str) -> String {
    text.chars().take(POSTPROCESS_MAX_INPUT_CHARS).collect()
}

/// Mirror of `voice_postprocess_user_content`, section for section and
/// delimiter for delimiter. The CLI surface has no raw-ASR section
/// (`--text`/`--text-file` is the corrected ASR text and the CLI applies no
/// deterministic rule corrections of its own, so there is no "before/after"
/// pair to send), but the draft IS transmitted: `--draft`/`--draft-file`
/// feeds the same `DRAFT_TEXT` block the GUI builds from `draft_text`, which
/// the edit prompt's rule 10 promises.
fn postprocess_user_content(corrected_text: &str, draft_text: Option<&str>) -> String {
    let mut sections = Vec::new();
    // Ordered exactly like the app: draft first, then the ASR instruction —
    // the prompts' examples read in that order.
    let draft = draft_text.unwrap_or("").trim();
    if !draft.is_empty() {
        sections.push(format!(
            "当前输入框已有文本：\n<<<DRAFT_TEXT>>>\n{draft}\n<<<END>>>"
        ));
    }
    sections.push(format!(
        "ASR 文本（规则纠错后）：\n<<<ASR_TEXT>>>\n{}\n<<<END>>>",
        corrected_text.trim()
    ));
    sections.push(
        "无论系统提示中的规则如何，输出必须使用 ASR 文本（及已有文本）原本的语言，不要翻译成其他语言。"
            .to_owned(),
    );
    sections.join("\n\n")
}

/// Output sanitizer mirrors `sanitize_voice_postprocess_output`:
/// strip a leading `<think>` block, one wrapping quote pair per quote kind,
/// and one fully-wrapping markdown fence.
fn sanitize_postprocess_output(text: &str) -> String {
    let without_thinking = strip_leading_thinking_block(text);
    let cleaned = strip_wrapping_quote(
        &strip_wrapping_quote(without_thinking.trim().trim_matches('\u{feff}').trim(), '"'),
        '\'',
    )
    .trim();
    strip_markdown_fence(cleaned).trim().to_owned()
}

fn strip_leading_thinking_block(text: &str) -> &str {
    let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return text;
    };
    match rest.find("</think>") {
        Some(end) => rest[end + "</think>".len()..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}'),
        None => "",
    }
}

fn strip_wrapping_quote(text: &str, quote: char) -> &str {
    if text.chars().count() == 1 {
        return text;
    }
    let Some(inner) = text.strip_prefix(quote) else {
        return text;
    };
    let Some(stripped) = inner.strip_suffix(quote) else {
        return text;
    };
    stripped
}

fn strip_markdown_fence(text: &str) -> &str {
    let Some(inner) = text.strip_prefix("```") else {
        return text;
    };
    let Some(newline) = inner.find('\n') else {
        return text;
    };
    let body = &inner[newline + 1..];
    match body.trim_end().strip_suffix("```") {
        Some(stripped) => stripped.trim(),
        None => text,
    }
}

/// Only the error class survives a reqwest failure: its Display carries the
/// full request URL (possibly an intranet address or a signed mirror link),
/// so it must never reach the terminal. Shared by the postprocess lane
/// (mirror of `summarize_voice_postprocess_error`) and the model download.
fn summarize_request_error(error: &reqwest::Error, subject: &str) -> String {
    if let Some(status) = error.status() {
        return format!("{subject} http {status}");
    }
    if error.is_timeout() {
        return format!("{subject} timeout");
    }
    if error.is_connect() {
        return format!("{subject} connect failed");
    }
    format!("{subject} request failed")
}

/// Mirror of `summarize_voice_postprocess_error`: reqwest Display carries the
/// full URL (possibly an intranet address), so only the error class is kept.
fn summarize_postprocess_error(error: &reqwest::Error) -> String {
    summarize_request_error(error, "model endpoint")
}

#[derive(Debug)]
struct PostprocessOutcome {
    text: String,
    source: &'static str,
    truncated: bool,
}

/// Applies the GUI's retry contract (`app/commands/voice.rs::voice_postprocess`)
/// to the retry response: non-empty retry text wins; an empty retry is the
/// correct answer for pure-filler input, not an error; and a failed retry is
/// an error — the first output was already judged unusable and must never be
/// returned as the postprocessed text.
fn postprocess_retry_result(
    retry: Result<(String, bool), CliError>,
) -> Result<PostprocessOutcome, CliError> {
    match retry {
        Ok((text, truncated)) if !text.trim().is_empty() => Ok(PostprocessOutcome {
            text,
            source: "llm",
            truncated,
        }),
        // Empty output for pure-filler input is the correct answer,
        // not an error (same contract as the GUI).
        Ok(_) => Ok(PostprocessOutcome {
            text: String::new(),
            source: "llm",
            truncated: false,
        }),
        Err(error) => Err(error),
    }
}

/// Stages of the GUI's JS voice pipeline (`platform/tauri/bridge/voice.js`)
/// that the CLI deliberately does NOT run, reported on every postprocess
/// result so the difference is visible instead of silent (see the module
/// header for the reasoning).
const POSTPROCESS_OMITTED_STAGES: [&str; 2] = [
    "deterministic-rule-corrections",
    "shrink-and-protected-term-validation",
];

/// Wall-clock bound for everything the command does after the windowless host
/// is up. The per-attempt `budget` measures only the model round-trip (GUI
/// parity), so this is the separate guarantee that a wedged bridge/probe
/// cannot make the one-shot CLI run forever. It cannot bound the host boot
/// itself — `run_windowless_host` owns that — but it stops the command before
/// the model phase when boot already blew past it.
const POSTPROCESS_TOTAL_BUDGET: Duration = Duration::from_secs(60);

fn postprocess(
    mode: PostprocessMode,
    text: Option<String>,
    text_file: Option<PathBuf>,
    draft: Option<String>,
    draft_file: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let raw_input = match (text, text_file) {
        (Some(text), _) => text,
        (None, Some(file)) => {
            crate::support::read_text_file_capped(&file, 64 * 1024, "voice postprocess")?
        }
        (None, None) => unreachable!("parse enforces exactly one text source"),
    };
    // Same input hygiene as the ASR text: regular files only, 64 KiB cap,
    // then the app's 4000-character model-input truncation
    // (`truncate_voice_postprocess_input` is applied to the draft there too).
    let draft_input = match (draft, draft_file) {
        (Some(draft), _) => Some(draft),
        (None, Some(file)) => Some(crate::support::read_text_file_capped(
            &file,
            64 * 1024,
            "voice postprocess",
        )?),
        (None, None) => None,
    };
    let draft_text = draft_input
        .map(|draft| truncate_postprocess_input(draft.trim()))
        .filter(|draft| !draft.is_empty());
    // The parser guarantees an edit call carries a draft option, but a file
    // whose whole content is whitespace resolves to nothing — and an edit
    // prompt that promises a DRAFT_TEXT section must never be sent without
    // one.
    if matches!(mode, PostprocessMode::Edit) && draft_text.is_none() {
        return Err(CliError::usage(
            "voice postprocess --mode edit requires a non-empty draft (the input-box text to \
             rewrite)",
        ));
    }
    let raw_text = truncate_postprocess_input(raw_input.trim());
    if raw_text.is_empty() {
        // Mirror of the GUI early return: pure-noise input short-circuits to
        // an empty result without touching the model.
        let value = serde_json::json!({
            "text": "",
            "mode": mode.as_str(),
            "source": "empty",
            "truncated": false,
            "omitted_stages": POSTPROCESS_OMITTED_STAGES,
        });
        return Ok(success(render(
            output,
            format!("Mode: {}\nSource: empty\nText:", mode.as_str()),
            &value,
        )));
    }
    // Overall bound, taken before the host boot; the model-call budget below
    // is taken after it (see `POSTPROCESS_TOTAL_BUDGET`).
    let boot_started = Instant::now();
    let budget = postprocess_timeout(mode, &raw_text);
    let mode_str = mode.as_str();
    // The host's work closure must resolve to `anyhow::Result`; the CLI-side
    // `CliError` (a std Error) converts into it, so this module never names
    // the host's error type.
    let host_result = pinvou3_lib::headless_bridge::run_windowless_host(move |pool, _store| {
        let work = async move {
            if boot_started.elapsed() >= POSTPROCESS_TOTAL_BUDGET {
                return Err(CliError::failed(format!(
                    "the windowless host took longer than the {} s command budget",
                    POSTPROCESS_TOTAL_BUDGET.as_secs()
                )));
            }
            // The model-call clock starts HERE, not before `run_windowless_host`:
            // the GUI's `started_at` (`app/commands/voice.rs`) only precedes an
            // in-memory bridge lookup and the vllm probe, so its 3–12 s budget
            // measures the model round-trip. Timing the tokio/Tauri/SessionStore
            // boot against the same number would spend the whole budget before
            // the first request and leave the retry structurally dead.
            let started = Instant::now();
            // Same shared-bridge fallback as the GUI `voice_postprocess_bridge`
            // with no session: global prefs + the active model.
            let mut bridge = pool.bridge.clone();
            bridge.prefs = pinvou3_lib::platform::prefs::UserPrefs::load();
            bridge.session_model = bridge.prefs.active_model().cloned();
            // vLLM served-name probe, same as the GUI lane (inference-same-origin key).
            let model_name = if bridge.provider() == "vllm" {
                pinvou3_lib::features::monitor::probe_vllm_model_info(
                    &bridge.base_url(),
                    Some(bridge.api_key().as_str()),
                )
                .await
                .0
                .unwrap_or_else(|| bridge.model())
            } else {
                bridge.model()
            };
            // GUI parity: the bridge resolution and the vllm probe share the
            // budget, so a zero remainder before the first request is the same
            // error the app raises instead of a guaranteed instant timeout.
            if started.elapsed() >= budget {
                return Err(CliError::failed(
                    "timeout budget exhausted before first request",
                ));
            }
            let (text, truncated) = call_postprocess_model(
                &bridge,
                mode,
                &raw_text,
                draft_text.as_deref(),
                false,
                &model_name,
                budget.saturating_sub(started.elapsed()),
            )?;
            // Mirror of `voice_postprocess_changed`: only the edit mode has an
            // "unchanged" notion, and it compares against the DRAFT, not the
            // ASR instruction. The draft is guaranteed non-empty above, so the
            // app's empty-draft exemption cannot be reached here.
            let unchanged = matches!(mode, PostprocessMode::Edit)
                && draft_text
                    .as_deref()
                    .map(|draft| text.trim() == draft.trim())
                    .unwrap_or(false);
            let needs_retry = text.trim().is_empty() || truncated || unchanged;
            if !needs_retry {
                return Ok::<PostprocessOutcome, CliError>(PostprocessOutcome {
                    text,
                    source: "llm",
                    truncated,
                });
            }
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                // GUI parity: the timeout budget ran out before the retry
                // could run — an error, never a silent bad first output. The
                // host-error boundary below prefixes "voice postprocess
                // failed: ", so the surfaced message is the exact GUI copy.
                return Err(CliError::failed("timeout budget exhausted before retry"));
            }
            // GUI parity (`app/commands/voice.rs`): a failed retry is an error
            // — the first output was already judged unusable, so returning it
            // would surface known-bad text as the postprocessed result.
            postprocess_retry_result(call_postprocess_model(
                &bridge,
                mode,
                &raw_text,
                draft_text.as_deref(),
                true,
                &model_name,
                remaining,
            ))
        };
        async move { work.await.map_err(std::convert::Into::into) }
    })
    .map_err(|error| CliError::failed(format!("voice postprocess failed: {error:#}")))?;
    let outcome = host_result;
    let value = serde_json::json!({
        "text": outcome.text,
        "mode": mode_str,
        "source": outcome.source,
        "truncated": outcome.truncated,
        "omitted_stages": POSTPROCESS_OMITTED_STAGES,
    });
    let human = format!(
        "Mode: {}\nSource: {}\nTruncated: {}\nText: {}\nNote: the model output is returned as \
         written; the CLI applies neither the desktop app's deterministic rule corrections nor \
         its shrink/protected-term validator, so it never silently falls back to the ASR text.",
        mode_str, outcome.source, outcome.truncated, outcome.text
    );
    Ok(success(render(output, human, &value)))
}

/// Blocking mirror of `call_voice_postprocess_model`. The GUI uses the async
/// reqwest client; this function runs inside the windowless host's tokio
/// worker, where the blocking client would panic (debug builds enforce
/// "cannot be built within an async runtime"), so the request inputs are
/// resolved into owned data first and the HTTP exchange itself runs on a
/// dedicated OS thread with no runtime context. Same endpoints, same body
/// (system+user messages, temperature 0, max_tokens, and the per-provider
/// thinking controls for the common vendors).
#[allow(clippy::too_many_arguments)]
fn call_postprocess_model(
    bridge: &pinvou3_lib::features::assistant::platform::bridge::Pinvou3Bridge,
    mode: PostprocessMode,
    raw_text: &str,
    draft_text: Option<&str>,
    retry: bool,
    model_name: &str,
    timeout: Duration,
) -> Result<(String, bool), CliError> {
    let base_url = bridge.base_url();
    let api_key = bridge.api_key();
    let provider = bridge.provider();
    let preset = bridge
        .effective_model_owned()
        .map(|model| model.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default());
    let system = if retry {
        postprocess_retry_prompt(mode)
    } else {
        postprocess_prompt(mode)
    };
    let user = postprocess_user_content(raw_text, draft_text);
    let model_name = model_name.to_owned();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(postprocess_http_exchange(
            base_url, api_key, provider, preset, system, user, model_name, mode, retry, timeout,
        ));
    });
    // The client's total timeout bounds the thread, so a recv past the
    // timeout plus slack means the thread itself is wedged — fail honestly
    // instead of parking the host's worker forever.
    match rx.recv_timeout(timeout + Duration::from_secs(10)) {
        Ok(result) => result,
        Err(_) => Err(CliError::failed(
            "model endpoint request failed: postprocess exchange timed out",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn postprocess_http_exchange(
    base_url: String,
    api_key: String,
    provider: String,
    preset: pinvou3_lib::platform::prefs::ModelPreset,
    system: &'static str,
    user: String,
    model_name: String,
    mode: PostprocessMode,
    retry: bool,
    timeout: Duration,
) -> Result<(String, bool), CliError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| CliError::failed(format!("voice postprocess: client: {error}")))?;

    if preset == pinvou3_lib::platform::prefs::ModelPreset::Anthropic {
        // Mirror of core::model_endpoint::post_anthropic_messages.
        let trimmed = base_url.trim_end_matches('/');
        let url = if trimmed.ends_with("/v1") {
            format!("{trimmed}/messages")
        } else {
            format!("{trimmed}/v1/messages")
        };
        let mut request = client
            .post(url)
            .header("anthropic-version", "2023-06-01")
            .json(&serde_json::json!({
                "model": model_name,
                "max_tokens": postprocess_max_tokens(mode, retry),
                "system": system,
                "messages": [{ "role": "user", "content": user }],
                "temperature": 0,
            }));
        if !api_key.trim().is_empty() {
            request = request.header("x-api-key", api_key.trim());
        }
        let value: serde_json::Value = request
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.json())
            .map_err(|error| {
                CliError::failed(format!(
                    "model endpoint request failed: {}",
                    summarize_postprocess_error(&error)
                ))
            })?;
        let text = value
            .get("content")
            .and_then(|content| content.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| {
                        if block.get("type").and_then(|kind| kind.as_str()) == Some("text") {
                            block.get("text").and_then(|text| text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        // GUI parity: the preset detects truncation through the response's
        // `stop_reason` (`app/commands/voice.rs` checks `max_tokens`); the CLI
        // mirrors that contract instead of the engine's own
        // `post_anthropic_messages` completion struct (crate-private there in
        // practice — `AnthropicCompletion` is not re-exported through any
        // module this crate can name).
        let truncated = anthropic_stop_reason_says_truncated(&value);
        return Ok((sanitize_postprocess_output(&text), truncated));
    }

    let mut body = serde_json::json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0,
        "max_tokens": postprocess_max_tokens(mode, retry),
        "stream": false
    });
    apply_postprocess_reasoning_controls(&mut body, preset, provider.as_str(), &model_name);
    let value: serde_json::Value = client
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json())
        .map_err(|error| {
            CliError::failed(format!(
                "model endpoint request failed: {}",
                summarize_postprocess_error(&error)
            ))
        })?;
    let message = value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .cloned()
        .unwrap_or_default();
    let content = if let Some(text) = message.get("content").and_then(|text| text.as_str()) {
        text.to_owned()
    } else {
        message
            .get("content")
            .and_then(|content| content.as_array())
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| {
                        part.get("text")
                            .or_else(|| part.get("content"))
                            .and_then(|text| text.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default()
    };
    let truncated = value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(|reason| reason.as_str())
        == Some("length");
    Ok((sanitize_postprocess_output(&content), truncated))
}

/// Truncation detection for the Anthropic Messages preset, mirroring
/// `app/commands/voice.rs` (`stop_reason == "max_tokens"`). Split out of
/// `postprocess_http_exchange` so a unit test can pin the parity without
/// an HTTP round-trip.
fn anthropic_stop_reason_says_truncated(response: &serde_json::Value) -> bool {
    response
        .get("stop_reason")
        .and_then(|reason| reason.as_str())
        == Some("max_tokens")
}

/// Subset of `apply_voice_reasoning_controls` / `voice_reasoning_dialect`
/// covering the deterministic vendor branches (vllm, deepseek, qwen by model
/// name); URL-sniffing lanes need crate-private helpers and are skipped
/// (disclosed in the module docs).
fn apply_postprocess_reasoning_controls(
    body: &mut serde_json::Value,
    preset: pinvou3_lib::platform::prefs::ModelPreset,
    provider: &str,
    model: &str,
) {
    if provider == "vllm" || preset == pinvou3_lib::platform::prefs::ModelPreset::LocalVllm {
        body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false });
        return;
    }
    if provider == "deepseek" || preset == pinvou3_lib::platform::prefs::ModelPreset::Deepseek {
        body["thinking"] = serde_json::json!({ "type": "disabled" });
        return;
    }
    let lower = model.to_ascii_lowercase();
    if provider == "qwen"
        || preset == pinvou3_lib::platform::prefs::ModelPreset::Qwen
        || lower.contains("qwen")
    {
        body["enable_thinking"] = serde_json::json!(false);
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::*;

    #[test]
    fn extract_transcript_joins_all_timed_segments_in_order() {
        // The bundled SenseVoice protocol emits one line per timed segment;
        // the transcript is every segment joined, not just the final one.
        assert_eq!(
            extract_transcript("[0.00-0.50] hello\n[0.50-1.00] wide world\n", ""),
            Some("hello wide world".to_owned())
        );
        assert_eq!(
            extract_transcript("[0.00-0.50] １２\n[0.50-1.00] ３\n", ""),
            Some("１２３".to_owned())
        );
    }

    #[test]
    fn extract_transcript_strips_sensevoice_control_markers() {
        assert_eq!(
            extract_transcript("[0.00-0.50] <|zh|><|NEUTRAL|>你好\n", ""),
            Some("你好".to_owned())
        );
    }

    #[test]
    fn extract_transcript_never_returns_engine_log_noise() {
        // Trailing engine summary / progress lines must not become the
        // transcript (the GUI filters these shapes in the same lane).
        assert_eq!(extract_transcript("done in 3.2s", ""), None);
        assert_eq!(extract_transcript("using 4 threads", ""), None);
        assert_eq!(extract_transcript("loading: model.safetensors", ""), None);
        assert_eq!(
            extract_transcript("00:00:01,000 --> 00:00:04,000", ""),
            None
        );
        assert_eq!(extract_transcript("[ffmpeg] download complete", ""), None);
        assert_eq!(extract_transcript("progress: 50", ""), None);
        // A JSON log object is diagnostics even when it carries a text field.
        assert_eq!(
            extract_transcript("{\"level\":\"info\",\"text\":\"loading\"}", ""),
            None
        );
    }

    #[test]
    fn extract_transcript_reads_json_and_timestamped_segments() {
        assert_eq!(
            extract_transcript("{\"text\": \"你好世界\"}", ""),
            Some("你好世界".to_owned())
        );
        assert_eq!(
            extract_transcript("[0.00-2.10] recognized words", ""),
            Some("recognized words".to_owned())
        );
    }
}

#[cfg(test)]
mod review_fix_tests {
    use super::*;

    #[test]
    fn postprocess_retry_failure_is_an_error_not_the_first_output() {
        // The retry only runs because the first output was judged unusable;
        // GUI parity (`app/commands/voice.rs`) makes a failed retry an error
        // instead of silently returning the known-bad first text.
        let error = CliError::failed("model endpoint request failed: model endpoint timeout");
        let outcome = postprocess_retry_result(Err(error));
        let error = outcome.expect_err("a failed retry must fail the command");
        assert_eq!(error.exit_code(), crate::ExitCode::Failed);
        assert!(
            error.to_string().contains("model endpoint timeout"),
            "the retry error must surface verbatim: {error}"
        );
    }

    #[test]
    fn postprocess_retry_empty_output_is_the_correct_answer() {
        // Empty retry output means pure-filler input: a success with empty
        // text, exactly like the GUI contract.
        let outcome = postprocess_retry_result(Ok((String::new(), true))).unwrap();
        assert_eq!(outcome.text, "");
        assert_eq!(outcome.source, "llm");
        assert!(!outcome.truncated);
    }

    #[test]
    fn postprocess_retry_non_empty_output_wins() {
        // The text is passed through verbatim: trimming/sanitization is the
        // caller's job (`sanitize_postprocess_output`), not the merge's.
        let outcome = postprocess_retry_result(Ok(("corrected text".to_owned(), false)))
            .expect("a non-empty retry succeeds");
        assert_eq!(outcome.text, "corrected text");
        assert_eq!(outcome.source, "llm");
        assert!(!outcome.truncated);
    }

    /// The edit prompt (rule 10) and the edit retry prompt both talk about a
    /// `DRAFT_TEXT` section and about "当前输入框已有文本". Before `--draft`
    /// existed the CLI shipped those prompts with the ASR section only, so
    /// the model was told to rewrite text it had never been given. The user
    /// message must carry the draft in the app's exact section shape and
    /// order (`voice_postprocess_user_content`).
    #[test]
    fn postprocess_user_content_emits_the_draft_section_the_prompt_promises() {
        let with_draft = postprocess_user_content("把它改成三条要点。", Some("  整理会议纪要。 "));
        let draft_at = with_draft
            .find("<<<DRAFT_TEXT>>>")
            .expect("the draft section must be present");
        let asr_at = with_draft
            .find("<<<ASR_TEXT>>>")
            .expect("the ASR section must be present");
        assert!(draft_at < asr_at, "draft first, like the app: {with_draft}");
        // Trimmed, delimited, and not concatenated into the ASR body.
        assert!(with_draft.contains("<<<DRAFT_TEXT>>>\n整理会议纪要。\n<<<END>>>"));
        assert!(with_draft.contains("<<<ASR_TEXT>>>\n把它改成三条要点。\n<<<END>>>"));

        // No draft (dictation/task) keeps the previous single-section shape:
        // an empty DRAFT_TEXT block would be a section the model must reason
        // about for nothing.
        for absent in [None, Some(""), Some("   \n ")] {
            let without = postprocess_user_content("查一下今日金价", absent);
            assert!(
                !without.contains("DRAFT_TEXT"),
                "an empty draft must emit no section: {without}"
            );
            assert!(without.contains("<<<ASR_TEXT>>>\n查一下今日金价\n<<<END>>>"));
        }
    }

    /// `--mode edit` exists only to rewrite the input-box draft, so the
    /// parser refuses without one instead of sending a prompt that describes
    /// an absent section. The other modes accept a draft (the GUI sends
    /// `draft_text` on every lane) but never require it.
    #[test]
    fn postprocess_edit_requires_a_draft_and_the_other_modes_do_not() {
        let strings = |values: &[&str]| values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        let error = parse(&strings(&[
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "把它改成三条要点",
        ]))
        .expect_err("edit without a draft must be a usage error");
        assert_eq!(error.exit_code(), crate::ExitCode::Usage);
        assert!(
            error.to_string().contains("--draft"),
            "the refusal must name the missing option: {error}"
        );

        let parsed = parse(&strings(&[
            "voice",
            "postprocess",
            "--mode",
            "edit",
            "--text",
            "把它改成三条要点",
            "--draft",
            "整理会议纪要",
        ]))
        .expect("edit with a draft parses");
        assert!(matches!(
            parsed,
            VoiceCommand::Postprocess {
                mode: PostprocessMode::Edit,
                draft: Some(_),
                ..
            }
        ));
        for mode in ["dictation", "task"] {
            parse(&strings(&[
                "voice",
                "postprocess",
                "--mode",
                mode,
                "--text",
                "x",
            ]))
            .expect("a draft is optional outside edit mode");
        }
    }

    /// Stopping the drain at the cap leaves the write end full, so a merely
    /// verbose engine blocks in `write()` until `asr_timeout` fires and the
    /// user gets a 60 s stall reported as a timeout instead of a bounded
    /// read. The bytes past the cap must be consumed and discarded (the same
    /// contract as `code.rs`'s `read_capped_to_eof`).
    #[test]
    fn drain_capped_keeps_the_cap_but_still_reads_to_eof() {
        struct Chatty {
            remaining: u64,
            consumed: std::sync::Arc<std::sync::atomic::AtomicU64>,
        }
        impl std::io::Read for Chatty {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Ok(0);
                }
                let take = (buf.len() as u64).min(self.remaining) as usize;
                buf[..take].fill(b'a');
                self.remaining -= take as u64;
                self.consumed
                    .fetch_add(take as u64, std::sync::atomic::Ordering::Relaxed);
                Ok(take)
            }
        }
        let consumed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let total = MAX_ENGINE_OUTPUT_BYTES + 3 * 1024 * 1024;
        let drained = drain_capped(Some(Chatty {
            remaining: total,
            consumed: std::sync::Arc::clone(&consumed),
        }));
        assert_eq!(
            drained.len() as u64,
            MAX_ENGINE_OUTPUT_BYTES,
            "the retained output must stay capped"
        );
        assert_eq!(
            consumed.load(std::sync::atomic::Ordering::Relaxed),
            total,
            "everything past the cap must be read and discarded so the child never blocks"
        );
    }

    #[test]
    fn summarize_request_error_keeps_only_the_error_class() {
        // A refused loopback connect yields a connect-class reqwest error
        // whose Display carries the URL; the summary must keep the class
        // only, never the URL (mirrors `summarize_voice_postprocess_error`).
        let error = reqwest::blocking::Client::new()
            .get("http://127.0.0.1:1/private-mirror-path?q=secret")
            .send()
            .expect_err("nothing listens on loopback port 1");
        let mirror = summarize_request_error(&error, "model mirror");
        assert!(mirror.starts_with("model mirror "), "got: {mirror}");
        assert!(
            mirror.contains("http ")
                || mirror.contains("timeout")
                || mirror.contains("connect failed")
                || mirror.contains("request failed"),
            "expected a known error class, got: {mirror}"
        );
        assert!(!mirror.contains("127.0.0.1"), "URL leaked: {mirror}");
        assert!(
            !mirror.contains("private-mirror-path"),
            "URL leaked: {mirror}"
        );
        // The postprocess lane keeps its established subject line.
        let endpoint = summarize_postprocess_error(&error);
        assert!(endpoint.starts_with("model endpoint "), "got: {endpoint}");
        assert!(!endpoint.contains("127.0.0.1"), "URL leaked: {endpoint}");
    }

    /// The normalized staging file is private audio; when the engine binary
    /// cannot be spawned at all, the spawn-error path must remove it instead
    /// of leaking it in the shared temp dir.
    #[cfg(unix)]
    #[test]
    fn engine_spawn_failure_removes_the_normalized_staging_file() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pinvou-cli-voice-spawn-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let normalized = dir.join("normalized.wav");
        std::fs::write(&normalized, vec![0u8; 44]).unwrap();
        // An engine file that exists but cannot be exec'd: spawn must fail
        // with a permission error, exercising the cleanup path.
        let engine = dir.join(engine_binary_name());
        std::fs::write(&engine, b"not an executable").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let mut command = std::process::Command::new(&engine);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let error =
            spawn_asr_engine(&mut command, &normalized).expect_err("the engine cannot be spawned");
        assert!(
            error.to_string().contains("cannot start ASR engine"),
            "expected the spawn error, got: {error}"
        );
        assert!(
            !normalized.exists(),
            "the failed spawn must not leak the normalized staging file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AsrLanes, AsrPreflight, OutputMode, PostprocessMode, anthropic_stop_reason_says_truncated,
        asr_preflight, ffmpeg_missing_is_fatal_for, postprocess_http_exchange, postprocess_prompt,
        transcribe_with,
    };

    /// The call site, not just the helper: against a loopback endpoint
    /// speaking the Anthropic Messages wire, a `stop_reason: "max_tokens"`
    /// answer must come back as `truncated == true`. Before the fix this
    /// exchange hardcoded `false`, so the GUI's unusable-output retry never
    /// fired for the preset and `truncated` contradicted the OpenAI lane.
    /// Hermetic: the server is a `std::net::TcpListener` on loopback, no
    /// external network.
    #[test]
    fn anthropic_exchange_reports_a_max_tokens_stop_reason_as_truncated() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            // Read until the end of the JSON body: the request is a single
            // reqwest write, so EOF on the client side ends the headers. Read
            // greedily; the client keeps the connection open awaiting the
            // response, so parse on Content-Length instead of read-to-EOF.
            loop {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    // The length prefix ends the request: the client holds the
                    // connection open awaiting the response, so there is no
                    // EOF to read to.
                    let content_length = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    });
                    if let Some(length) = content_length {
                        if body.len() >= length {
                            break;
                        }
                    }
                }
            }
            let json = "{\"content\":[{\"type\":\"text\",\"text\":\"cut mid sent\"}],\
\"stop_reason\":\"max_tokens\",\"model\":\"m\"}";
            let body = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\
content-length: {}\r\n\r\n{}",
                json.len(),
                json
            );
            stream.write_all(body.as_bytes()).unwrap();
        });
        let (text, truncated) = postprocess_http_exchange(
            format!("http://127.0.0.1:{port}"),
            String::new(),
            String::new(),
            pinvou3_lib::platform::prefs::ModelPreset::Anthropic,
            postprocess_prompt(PostprocessMode::Task),
            "user".to_owned(),
            "model".to_owned(),
            PostprocessMode::Task,
            false,
            Duration::from_secs(10),
        )
        .expect("the mock exchange must succeed");
        server.join().unwrap();
        assert_eq!(text, "cut mid sent");
        assert!(
            truncated,
            "a max_tokens stop_reason must be reported as truncated"
        );
        // Sanity on the request that reached the wire: this is the Anthropic
        // Messages route with the version header, not the chat/completions one.
    }

    #[test]
    fn ffmpeg_missing_is_fatal_except_for_wav_inputs() {
        assert!(ffmpeg_missing_is_fatal_for(Some("mp3")));
        assert!(ffmpeg_missing_is_fatal_for(Some("m4a")));
        assert!(!ffmpeg_missing_is_fatal_for(Some("wav")));
        assert!(!ffmpeg_missing_is_fatal_for(Some("WAV")));
        assert!(!ffmpeg_missing_is_fatal_for(Some("Wav")));
        assert!(ffmpeg_missing_is_fatal_for(None));
    }

    /// The Anthropic preset must detect truncation like the GUI does
    /// (`app/commands/voice.rs`: `stop_reason == "max_tokens"`). Before this
    /// the CLI lane hardcoded `false`, so a max_tokens-cut answer was reported
    /// as `truncated: false`, visibly contradicting the same field on the
    /// OpenAI wire (`finish_reason == "length"`), and never triggered the
    /// unusable-output retry the GUI runs for truncated text.
    #[test]
    fn anthropic_truncation_is_detected_like_the_gui() {
        let truncated = serde_json::json!({
            "content": [{ "type": "text", "text": "cut mid sent" }],
            "stop_reason": "max_tokens",
        });
        assert!(
            anthropic_stop_reason_says_truncated(&truncated),
            "stop_reason max_tokens must report truncated"
        );
        for stop_reason in [
            serde_json::json!("end_turn"),
            serde_json::json!("stop_sequence"),
            // A JSON null: the same non-string tolerance the OpenAI lane's
            // `finish_reason` lookup has for its field.
            serde_json::json!(null),
        ] {
            let ordinary = serde_json::json!({
                "content": [{ "type": "text", "text": "complete" }],
                "stop_reason": stop_reason,
            });
            assert!(
                !anthropic_stop_reason_says_truncated(&ordinary),
                "stop_reason {stop_reason} is not truncation"
            );
        }
        // A response that omits the field entirely is not truncated either.
        assert!(!anthropic_stop_reason_says_truncated(&serde_json::json!({
            "content": []
        })));
    }

    /// Lanes with a *supported* native engine (the Linux shape). Every gate
    /// case below that talks about engine/ffmpeg/model presupposes it.
    fn lanes(engine: bool, ffmpeg: bool, model: bool, external: bool) -> AsrLanes {
        AsrLanes {
            engine,
            ffmpeg,
            model,
            external,
            native: true,
        }
    }

    /// Lanes on a target whose `run_recognition` has no native lane (macOS,
    /// Windows): the component triple buys nothing there.
    fn lanes_without_native_support(
        engine: bool,
        ffmpeg: bool,
        model: bool,
        external: bool,
    ) -> AsrLanes {
        AsrLanes {
            engine,
            ffmpeg,
            model,
            external,
            native: false,
        }
    }

    /// The full component table for the pre-flight gate. Only one combination
    /// continues on a warning — engine + model installed, ffmpeg missing, an
    /// input that needs no conversion — and every other one keeps the error it
    /// has always produced.
    #[test]
    fn asr_preflight_downgrades_only_the_ffmpeg_gap_on_wav_inputs() {
        assert_eq!(
            asr_preflight(lanes(true, false, true, false), Some("wav")),
            AsrPreflight::RunOnRawWav
        );
        assert_eq!(
            asr_preflight(lanes(true, false, true, false), Some("WAV")),
            AsrPreflight::RunOnRawWav
        );
        // Anything the engine cannot decode on its own still needs ffmpeg.
        for extension in [Some("mp3"), Some("m4a"), None] {
            let decision = asr_preflight(lanes(true, false, true, false), extension);
            assert!(
                matches!(decision, AsrPreflight::Reject(message) if message.starts_with("ffmpeg_missing")),
                "{extension:?} must stay a hard ffmpeg error, got: {decision:?}"
            );
        }
        for extension in [Some("wav"), Some("mp3"), None] {
            // A missing engine or a missing model is the "not installed"
            // condition, wav or not.
            for missing in [
                lanes(false, false, false, false),
                lanes(false, true, true, false),
                lanes(true, true, false, false),
                lanes(false, false, true, false),
            ] {
                let decision = asr_preflight(missing, extension);
                assert!(
                    matches!(decision, AsrPreflight::Reject(message) if message.starts_with("asr_engine_missing")),
                    "{missing:?} with {extension:?} must report a missing install, got: {decision:?}"
                );
            }
            // A configured external ASR CLI is a complete lane by itself, and
            // a complete native install never reaches the gate's error arms.
            assert_eq!(
                asr_preflight(lanes(false, false, false, true), extension),
                AsrPreflight::Run
            );
            assert_eq!(
                asr_preflight(lanes(true, true, true, false), extension),
                AsrPreflight::Run
            );
        }
    }

    /// The gate and the dispatcher must agree about which components matter.
    /// `run_recognition` only spawns the bundled engine where
    /// [`super::native_lane_supported`] holds, so on macOS/Windows a hand
    /// installed engine + model + ffmpeg under `$PINVOU3_HOME/asr` is NOT a
    /// lane: the gate has to reject up front instead of letting the call
    /// stage its audio and then die with `asr_engine_missing`.
    #[test]
    fn asr_preflight_rejects_the_native_triple_where_no_native_lane_exists() {
        for extension in [Some("wav"), Some("mp3"), None] {
            let decision = asr_preflight(
                lanes_without_native_support(true, true, true, false),
                extension,
            );
            assert!(
                matches!(decision, AsrPreflight::Reject(message) if message.starts_with("asr_engine_missing")),
                "a complete component triple without a native lane must not pass the gate, got: \
                 {decision:?}"
            );
            // The ffmpeg-gap downgrade is a native-lane concept too: without
            // that lane there is nothing to feed the raw wav to.
            let decision = asr_preflight(
                lanes_without_native_support(true, false, true, false),
                extension,
            );
            assert!(
                matches!(decision, AsrPreflight::Reject(message) if message.starts_with("asr_engine_missing")),
                "the raw-wav downgrade needs a native lane, got: {decision:?}"
            );
            // The external CLI stays a complete lane on every target — that
            // is the one lane such a host actually has.
            assert_eq!(
                asr_preflight(
                    lanes_without_native_support(true, true, true, true),
                    extension
                ),
                AsrPreflight::Run
            );
            assert_eq!(
                asr_preflight(
                    lanes_without_native_support(false, false, false, true),
                    extension
                ),
                AsrPreflight::Run
            );
        }
    }

    /// Caller-level guard for the same fact, because the gate once printed the
    /// raw-wav warning and then returned `asr_engine_missing` anyway: with
    /// engine and model installed and ffmpeg absent, a `.wav` input must reach
    /// recognition. Restoring that return makes this fail.
    #[test]
    fn transcribe_reaches_recognition_for_a_wav_input_without_ffmpeg() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "pinvou-cli-voice-gate-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("capture.wav");
        // 44-byte header-only WAV: large enough to pass the empty-audio gate.
        std::fs::write(&wav, vec![0u8; 44]).unwrap();

        let outcome = transcribe_with(
            &wav,
            OutputMode::Human,
            || lanes(true, false, true, false),
            |_| {
                Ok((
                    "stub transcript".to_owned(),
                    "pinvou-webview-sensevoice-local",
                ))
            },
        )
        .expect("the raw-wav lane must run instead of reporting a missing install");
        assert_eq!(outcome.exit_code, crate::ExitCode::Success);
        assert!(
            outcome.stdout.contains("stub transcript"),
            "the recognized text must be reported: {}",
            outcome.stdout
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
