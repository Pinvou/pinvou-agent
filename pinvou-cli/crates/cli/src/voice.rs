//! `voice` family: local ASR status/transcription/install and voice
//! post-processing, mirroring `pinvou3-app/src-tauri/src/app/commands/voice.rs`.
//!
//! Visibility disclosure: the entire `pinvou3_lib::features::voice` surface
//! (status, engine transcription, transcript parsing, platform adapters) is
//! `pub(crate)` to the app crate, so the CLI *mirrors* its semantics over the
//! exact same paths and environment variables instead of calling it:
//! - data root `$PINVOU3_HOME/asr` (`features::voice::voice_asr::asr_dir`),
//!   engine binary per platform adapter (`sense-voice-main` on Linux,
//!   `pinvou-asr` elsewhere), model `sense-voice-small-q4_k.gguf` (Linux/
//!   macOS) or `sensevoice-small-q8.gguf` (Windows) with the same expected
//!   byte size, and the same `PINVOU3_ASR_CMD` / `PINVOU3_DEEPSPEECH2_CMD` /
//!   `PADDLESPEECH_BIN` external-CLI fallback chain with the same `asr
//!   --model --lang --input` protocol, 60 s default timeout and exit-code-6
//!   "no speech" convention.
//! - the app verifies downloaded models by size **and** sha256
//!   (`platform::hashing::sha256_file`, also crate-private); the CLI has no
//!   hash dependency, so `asr-install` verifies the expected byte size only
//!   and says so in its output.
//! - the app's transcript parser (`features::voice::transcript`) handles
//!   several engine log protocols; the CLI mirrors the common shapes (JSON
//!   `{"text": …}` lines, `[0.00s → 2.10s] …` segments, plain final lines)
//!   and discloses the simplification.
//! - macOS transcription uses the system Speech framework through a
//!   crate-private adapter; the CLI cannot reach it, so on macOS it reports
//!   the Speech runtime as ready (same as the GUI status) but performs
//!   transcription through the external-CLI lane only.
//!
//! `voice postprocess` needs the GUI's `EnginePool` state to resolve the
//! active model credentials, so it runs through the windowless product host
//! (`run_windowless_host`, requires a display / xvfb like `agent run`) and
//! then issues the same OpenAI-compatible (or Anthropic Messages) HTTP
//! request with the mirrored prompt constants. The prompts are private to
//! `app/commands/voice.rs` and are duplicated here verbatim; if they drift
//! upstream this module must be updated in the same pull request.

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
    },
    AsrStatus,
    AsrInstall,
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
     (--text S|--text-file F)|asr-status|asr-install>";

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
            Ok(VoiceCommand::Postprocess {
                mode,
                text,
                text_file,
            })
        }
        "asr-status" => {
            if !rest.is_empty() {
                return Err(CliError::usage("voice asr-status accepts no options"));
            }
            Ok(VoiceCommand::AsrStatus)
        }
        "asr-install" => {
            if !rest.is_empty() {
                return Err(CliError::usage("voice asr-install accepts no options"));
            }
            Ok(VoiceCommand::AsrInstall)
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

pub fn execute(command: VoiceCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        VoiceCommand::Transcribe { path } => transcribe(&path, output),
        VoiceCommand::Postprocess {
            mode,
            text,
            text_file,
        } => postprocess(mode, text, text_file, output),
        VoiceCommand::AsrStatus => asr_status(output),
        VoiceCommand::AsrInstall => asr_install(output),
    }
}

// ─────────────────────────── ASR platform mirror ───────────────────────────

/// Same fields as `features::voice::voice_asr::AsrModelSpec` (the struct is
/// pub but its module is `pub(crate)`).
struct AsrModelSpec {
    filename: &'static str,
    expected_size: u64,
    #[allow(dead_code)] // mirrored for documentation; CLI verifies size only
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

/// Fast availability probe: the expected byte size (the sha256 digest is
/// verified by the download itself, mirroring `model_file_verified`).
fn model_available() -> bool {
    std::fs::metadata(model_path())
        .map(|meta| meta.len() == model_spec().expected_size)
        .unwrap_or(false)
}

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Same composition as `voice_asr::compose_status` / `status()`: macOS
/// reports the system Speech runtime as present (`asr_bundled_runtime_status`
/// → `Some(true)`), other platforms check engine + ffmpeg + model files.
fn asr_components() -> (bool, bool, bool, bool) {
    // The app's Windows status reports installable from the same external
    // tool probe; macOS needs no installation; Linux uses the installer.
    let installable = cfg!(target_os = "linux") || cfg!(target_os = "windows");
    if cfg!(target_os = "macos") {
        // System Speech needs no engine binary, model file, or ffmpeg.
        return (true, true, true, installable);
    }
    let engine = engine_path().is_some();
    let model = model_available();
    let ffmpeg = ffmpeg_available();
    (engine, ffmpeg, model, installable)
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
    // transcribe right now (Linux bundled engine, or the external ASR CLI).
    let cli_transcribe_ready =
        (cfg!(target_os = "linux") && engine && model) || external_asr_command().is_some();
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
    let value = serde_json::json!({
        "engine": engine,
        "ffmpeg": ffmpeg,
        "model": model,
        "ready": ready,
        "cli_transcribe_ready": cli_transcribe_ready,
        "installable": installable,
        "missing": missing,
        "asr_dir": asr_dir().display().to_string(),
    });
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
    Ok(success(render(output, human, &value)))
}

/// Linux install lane: missing ffmpeg goes through the same public
/// `features::dependencies::install_dependencies` call the GUI platform
/// adapter makes (pkexec/apt), then the SenseVoice model is downloaded from
/// the primary URL with the mirror as fallback (a `PINVOU3_ASR_MODEL_URL`
/// override wins, like the app's `model_download_urls`). The download is
/// staged, size-capped, and sha256-verified against the pinned digest.
fn asr_install(output: OutputMode) -> Result<CliOutcome, CliError> {
    if !cfg!(target_os = "linux") {
        return Err(CliError::failed(
            "voice asr-install is only supported on Linux; on Windows repair/ reinstall pinvou, \
             on macOS system speech needs no installation",
        ));
    }
    let mut steps: Vec<String> = Vec::new();
    if !ffmpeg_available() {
        pinvou3_lib::features::dependencies::install_dependencies(vec!["ffmpeg".to_owned()], None)
            .map_err(|error| CliError::failed(format!("voice asr-install: ffmpeg: {error}")))?;
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
    let mut urls: Vec<&str> = Vec::new();
    if let Some(custom) = custom.as_deref() {
        urls.push(custom);
    }
    urls.push(spec.primary_url);
    urls.push(spec.mirror_url);
    for url in urls {
        if download_to(url, &dest).is_ok() && file_is_sha256(&dest, spec.sha256) {
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

fn download_to(url: &str, dest: &Path) -> Result<(), CliError> {
    // Staged through a .part sibling and size-capped: a crashed or hostile
    // download must never leave a truncated/garbage file at the real path
    // (the checksum gate below still has final say).
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
        .map_err(|error| CliError::failed(format!("voice asr-install: download: {error}")))?;
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
    let size = std::fs::metadata(&part)
        .map(|meta| meta.len())
        .map_err(|error| CliError::failed(format!("voice asr-install: stat: {error}")))?;
    if size > MAX_DOWNLOAD_BYTES {
        let _ = std::fs::remove_file(&part);
        return Err(CliError::failed(
            "voice asr-install: model exceeds the size cap",
        ));
    }
    std::fs::rename(&part, dest)
        .map_err(|error| CliError::failed(format!("voice asr-install: finish: {error}")))?;
    Ok(())
}

/// Hard upper bound for one model download (largest expected model is ~254
/// MiB; the cap exists so a hostile mirror cannot balloon the disk).
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

// ─────────────────────────── transcribe ────────────────────────────────────

/// Mirror of the GUI's decoded-audio cap (`recording_too_long`).
const MAX_TRANSCRIBE_BYTES: usize = 4 * 1024 * 1024;

fn transcribe(path: &Path, output: OutputMode) -> Result<CliOutcome, CliError> {
    // Fail fast on missing ASR before loading the file into memory.
    let (engine, ffmpeg, model, _) = asr_components();
    if !cfg!(target_os = "macos")
        && !(engine && ffmpeg && model)
        && external_asr_command().is_none()
    {
        return Err(CliError::failed(
            "asr_engine_missing: local speech recognition is not installed \
             (hint: run `pinvou voice asr-status`)",
        ));
    }
    let metadata = std::fs::metadata(path).map_err(|error| {
        CliError::failed(format!(
            "voice transcribe: cannot read {}: {error}",
            path.display()
        ))
    })?;
    if metadata.len() > MAX_TRANSCRIBE_BYTES as u64 {
        return Err(CliError::failed(
            "recording_too_long: recording exceeds the 4 MiB transcription cap",
        ));
    }
    let audio = std::fs::read(path).map_err(|error| {
        CliError::failed(format!(
            "voice transcribe: cannot read {}: {error}",
            path.display()
        ))
    })?;
    if audio.len() < 44 {
        return Err(CliError::failed(
            "audio_empty: recording is empty or corrupted",
        ));
    }
    let wav = write_temp_wav(&audio)?;
    let result = run_recognition(&wav);
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| CliError::failed(format!("voice transcribe: cannot stage audio: {error}")),
        )?;
    }
    std::io::Write::write_all(&mut file, bytes).map_err(|error| {
        CliError::failed(format!("voice transcribe: cannot stage audio: {error}"))
    })?;
    Ok(path)
}

/// Recognition dispatch mirrors `transcribe_voice_audio_bytes`: the platform
/// native lane first (Linux bundled SenseVoice engine; macOS Speech is not
/// reachable from the CLI), then the external ASR CLI. Without either lane
/// the error names `pinvou voice asr-status` as the hint.
fn run_recognition(wav: &Path) -> Result<(String, &'static str), CliError> {
    if cfg!(target_os = "linux") && engine_path().is_some() && model_available() {
        // GUI parity: a failing native lane falls back to the env-configured
        // external ASR CLI before giving up.
        if let Ok(text) = native_engine_transcribe(wav) {
            return Ok((text, "sensevoice-local"));
        }
    }
    match external_asr_command() {
        Some(command) => external_cli_transcribe(&command, wav).map(|text| (text, "local_cli")),
        None => Err(CliError::failed(
            "asr_engine_missing: local speech recognition is not installed \
             (hint: run `pinvou voice asr-status`)",
        )),
    }
}

/// Mirror of `voice_asr::transcribe`: normalize to 16 kHz mono through ffmpeg
/// when available, run the engine with cwd pinned to the writable asr dir,
/// and fail on empty output.
fn native_engine_transcribe(wav: &Path) -> Result<String, CliError> {
    let engine = engine_path().expect("engine checked by caller");
    let model = model_path();
    // The nonce keeps two concurrent transcribes (or a reused pid) from
    // colliding on the normalized scratch file, same as `write_temp_wav`.
    let normalized = std::env::temp_dir().join(format!(
        "pinvou-cli-asr-{}-{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let input = if ffmpeg_available() {
        let converted = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(wav)
            .args(["-ar", "16000", "-ac", "1", "-f", "wav"])
            .arg(&normalized)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
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
    let engine_result = std::process::Command::new(&engine)
        .current_dir(&work_dir)
        .arg("-m")
        .arg(&model)
        .arg(&input)
        .args(["-t", "4", "-l", "auto", "-itn"])
        .output();
    if input != wav {
        let _ = std::fs::remove_file(&input);
    }
    let engine_result = engine_result.map_err(|error| {
        CliError::failed(format!(
            "voice transcribe: cannot start ASR engine: {error}"
        ))
    })?;
    if !engine_result.status.success() {
        let tail = String::from_utf8_lossy(&engine_result.stderr);
        return Err(CliError::failed(format!(
            "asr_engine_error: ASR engine failed: {}",
            tail.lines().last().unwrap_or("").trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&engine_result.stdout).into_owned();
    extract_transcript(&stdout, "").ok_or_else(|| {
        CliError::failed("asr_no_speech: no speech content recognized; please retry")
    })
}

/// Mirror of `run_local_asr_cli`: same argument protocol, same env-driven
/// model/language/timeout, concurrent pipe draining, and exit code 6 as the
/// "no speech" convention.
fn external_cli_transcribe(command: &Path, wav: &Path) -> Result<String, CliError> {
    use std::io::Read;
    use std::process::Stdio;

    let model = std::env::var("PINVOU3_ASR_MODEL")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_MODEL"))
        .unwrap_or_else(|_| "sensevoice-q8".to_owned());
    let language = std::env::var("PINVOU3_ASR_LANG")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_LANG"))
        .unwrap_or_else(|_| "zh".to_owned());
    let timeout = std::env::var("PINVOU3_ASR_TIMEOUT_SECS")
        .or_else(|_| std::env::var("PINVOU3_DEEPSPEECH2_TIMEOUT_SECS"))
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(60);

    let mut command_line = std::process::Command::new(command);
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
    let stdout_drain = std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut pipe) = stdout_pipe {
            let _ = pipe.read_to_string(&mut buffer);
        }
        buffer
    });
    let stderr_drain = std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut pipe) = stderr_pipe {
            let _ = pipe.read_to_string(&mut buffer);
        }
        buffer
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if started.elapsed() >= Duration::from_secs(timeout) {
                    break Err(CliError::failed(format!(
                        "asr_timeout: local speech recognition timed out ({timeout} s)"
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                break Err(CliError::failed(format!(
                    "asr_runtime_error: local speech recognition failed unexpectedly: {error}"
                )));
            }
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    let stdout = stdout_drain.join().unwrap_or_default();
    let stderr = stderr_drain.join().unwrap_or_default();
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
        CliError::failed("asr_no_speech: recognition returned no usable text; please retry")
    })
}

/// Mirror of `features::voice::transcript::parse_asr_transcript` (JSON
/// `{"text": …}` lines, `[0.00s → 2.10s] …` timestamped segments, and plain
/// final text lines; the last usable line wins, stdout preferred over
/// stderr) including the GUI's log/status line filters, so engine noise like
/// `system_info: …` or `Done in 3.2s` is never returned as the transcript.
fn extract_transcript(stdout: &str, stderr: &str) -> Option<String> {
    let usable = |text: &str| text.chars().any(|ch| ch.is_alphanumeric());
    let clean = |line: &str| -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(text) = value.get("text").and_then(|text| text.as_str()) {
                return usable(text).then(|| text.trim().to_owned());
            }
            return None;
        }
        // Timestamped segment shapes ("[0.00s -> 2.10s] text" /
        // "[00:00.000 --> 00:02.100] text") are recognized before the log
        // filters — the GUI parses segments first for the same reason.
        let bracket = line.starts_with('[');
        let stamp_end = line.find(']');
        let is_segment = bracket
            && stamp_end
                .map(|end| line[..end].contains("->"))
                .unwrap_or(false);
        if !is_segment && (looks_like_log_line(line) || looks_like_plain_status(line)) {
            return None;
        }
        let without_time = if bracket {
            stamp_end.map(|end| line[end + 1..].trim()).unwrap_or(line)
        } else {
            line
        };
        let candidate = without_time.trim();
        if candidate.is_empty() || candidate.contains("-->") {
            return None;
        }
        usable(candidate).then(|| candidate.to_owned())
    };
    for stream in [stdout, stderr] {
        for line in stream.lines().rev() {
            if let Some(text) = clean(line) {
                return Some(text);
            }
        }
    }
    None
}

/// Simplified port of `transcript::looks_like_log_line`: engine/runtime noise
/// that must never surface as recognized speech.
fn looks_like_log_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("warning")
        || lower.contains("paddlespeech")
        || lower.contains("sensevoice")
        || lower.contains("funasr")
        || lower.contains("gguf")
        || lower.contains("python")
        || lower.contains("download")
        || lower == "done"
        || lower.starts_with("done in ")
        || (lower.starts_with("using ") && lower.contains("thread"))
        || lower.starts_with("system_info")
        || lower.starts_with('[')
}

/// Simplified port of `transcript::looks_like_plain_status`: progress /
/// loading / subtitle-timing shapes.
fn looks_like_plain_status(line: &str) -> bool {
    let trimmed = line.trim().trim_matches(['[', ']', '(', ')']);
    let lower = trimmed.to_ascii_lowercase();
    ["progress:", "progress ", "loading:", "loading ", "processed:", "processed "]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        // Subtitle timing: "00:00:01,000 --> 00:00:04,000" (already excluded
        // by the --> check for candidates, but the raw line itself is status).
        || lower.contains("-->")
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

/// Mirror of `voice_postprocess_user_content` (no draft / raw text from the
/// CLI surface: `--text`/`--text-file` is the corrected ASR text).
fn postprocess_user_content(corrected_text: &str) -> String {
    let mut sections = Vec::new();
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

/// Mirror of `summarize_voice_postprocess_error`: reqwest Display carries the
/// full URL (possibly an intranet address), so only the error class is kept.
fn summarize_postprocess_error(error: &reqwest::Error) -> String {
    if let Some(status) = error.status() {
        return format!("model endpoint http {status}");
    }
    if error.is_timeout() {
        return "model endpoint timeout".to_owned();
    }
    if error.is_connect() {
        return "model endpoint connect failed".to_owned();
    }
    "model endpoint request failed".to_owned()
}

struct PostprocessOutcome {
    text: String,
    source: &'static str,
    truncated: bool,
}

fn postprocess(
    mode: PostprocessMode,
    text: Option<String>,
    text_file: Option<PathBuf>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    sandbox_home()?;
    let raw_input = match (text, text_file) {
        (Some(text), _) => text,
        (None, Some(file)) => std::fs::read_to_string(&file).map_err(|error| {
            CliError::failed(format!(
                "voice postprocess: cannot read {}: {error}",
                file.display()
            ))
        })?,
        (None, None) => unreachable!("parse enforces exactly one text source"),
    };
    let raw_text = truncate_postprocess_input(raw_input.trim());
    if raw_text.is_empty() {
        // Mirror of the GUI early return: pure-noise input short-circuits to
        // an empty result without touching the model.
        let value = serde_json::json!({
            "text": "",
            "mode": mode.as_str(),
            "source": "empty",
            "truncated": false,
        });
        return Ok(success(render(
            output,
            format!("Mode: {}\nSource: empty\nText:", mode.as_str()),
            &value,
        )));
    }
    let started = Instant::now();
    let budget = postprocess_timeout(mode, &raw_text);
    let mode_str = mode.as_str();
    // The host's work closure must resolve to `anyhow::Result`; the CLI-side
    // `CliError` (a std Error) converts into it, so this module never names
    // the host's error type.
    let host_result = pinvou3_lib::headless_bridge::run_windowless_host(move |pool, _store| {
        let work = async move {
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
            // A floor keeps the first call meaningful when host boot already
            // consumed most of the budget: a zero timeout would fail
            // instantly ("model endpoint timeout") instead of trying.
            const MIN_FIRST_CALL_TIMEOUT: Duration = Duration::from_secs(2);
            let (text, truncated) = call_postprocess_model(
                &bridge,
                mode,
                &raw_text,
                false,
                &model_name,
                budget
                    .saturating_sub(started.elapsed())
                    .max(MIN_FIRST_CALL_TIMEOUT),
            )?;
            let needs_retry = text.trim().is_empty()
                || truncated
                || (matches!(mode, PostprocessMode::Edit) && text.trim() == raw_text.trim());
            if !needs_retry {
                return Ok::<PostprocessOutcome, CliError>(PostprocessOutcome {
                    text,
                    source: "llm",
                    truncated,
                });
            }
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Ok(PostprocessOutcome {
                    text,
                    source: "llm",
                    truncated,
                });
            }
            match call_postprocess_model(&bridge, mode, &raw_text, true, &model_name, remaining) {
                Ok((retry_text, retry_truncated)) if !retry_text.trim().is_empty() => {
                    Ok(PostprocessOutcome {
                        text: retry_text,
                        source: "llm",
                        truncated: retry_truncated,
                    })
                }
                // Empty output for pure-filler input is the correct answer,
                // not an error (same contract as the GUI).
                Ok(_) => Ok(PostprocessOutcome {
                    text: String::new(),
                    source: "llm",
                    truncated: false,
                }),
                Err(_) => Ok(PostprocessOutcome {
                    text,
                    source: "llm",
                    truncated,
                }),
            }
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
    });
    let human = format!(
        "Mode: {}\nSource: {}\nTruncated: {}\nText: {}",
        mode_str, outcome.source, outcome.truncated, outcome.text
    );
    Ok(success(render(output, human, &value)))
}

/// Blocking mirror of `call_voice_postprocess_model`. The GUI uses the async
/// reqwest client; the CLI has no async runtime of its own inside this
/// closure, so the blocking client is used against the same endpoints with
/// the same body (system+user messages, temperature 0, max_tokens, and the
/// per-provider thinking controls for the common vendors).
#[allow(clippy::too_many_arguments)]
fn call_postprocess_model(
    bridge: &pinvou3_lib::features::assistant::platform::bridge::Pinvou3Bridge,
    mode: PostprocessMode,
    raw_text: &str,
    retry: bool,
    model_name: &str,
    timeout: Duration,
) -> Result<(String, bool), CliError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| CliError::failed(format!("voice postprocess: client: {error}")))?;
    let base_url = bridge.base_url();
    let system = if retry {
        postprocess_retry_prompt(mode)
    } else {
        postprocess_prompt(mode)
    };
    let user = postprocess_user_content(raw_text);
    let preset = bridge
        .effective_model_owned()
        .map(|model| model.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default());

    if preset == pinvou3_lib::platform::prefs::ModelPreset::Anthropic {
        // Mirror of core::model_endpoint::post_anthropic_messages.
        let trimmed = base_url.trim_end_matches('/');
        let url = if trimmed.ends_with("/v1") {
            format!("{trimmed}/messages")
        } else {
            format!("{trimmed}/v1/messages")
        };
        let api_key = bridge.api_key();
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
        return Ok((sanitize_postprocess_output(&text), false));
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
    apply_postprocess_reasoning_controls(&mut body, preset, bridge.provider().as_str(), model_name);
    let value: serde_json::Value = client
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(bridge.api_key())
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
    fn extract_transcript_prefers_the_last_usable_line() {
        let stdout = "loading model\nsystem_info: n_threads = 4\nhello world\n";
        assert_eq!(
            extract_transcript(stdout, ""),
            Some("hello world".to_owned())
        );
    }

    #[test]
    fn extract_transcript_never_returns_engine_log_noise() {
        // Trailing engine summary / progress lines must not become the
        // transcript (the GUI filters these shapes in the same lane).
        assert_eq!(extract_transcript("done in 3.2s", ""), None);
        assert_eq!(extract_transcript("system_info: n_threads = 4", ""), None);
        assert_eq!(extract_transcript("loading: model.safetensors", ""), None);
        assert_eq!(
            extract_transcript("00:00:01,000 --> 00:00:04,000", ""),
            None
        );
        assert_eq!(extract_transcript("[ffmpeg] download complete", ""), None);
    }

    #[test]
    fn extract_transcript_reads_json_and_timestamped_segments() {
        assert_eq!(
            extract_transcript("{\"text\": \"你好世界\"}", ""),
            Some("你好世界".to_owned())
        );
        assert_eq!(
            extract_transcript("[0.00s -> 2.10s] recognized words", ""),
            Some("recognized words".to_owned())
        );
    }
}
