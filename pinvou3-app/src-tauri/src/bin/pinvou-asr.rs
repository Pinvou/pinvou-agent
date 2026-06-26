//! Stable CLI entrypoint for pinvou's optional offline ASR runtime.
//!
//! The executable is intentionally a small wrapper around a bundled ASR
//! runtime. This keeps the main app independent from backend/runtime layout
//! details while preserving a fixed command contract:
//!
//!   pinvou-asr asr --model sensevoice-q8 --lang zh --input input.wav

use std::env;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_MODEL: &str = "sensevoice-q8";
const DEFAULT_LANG: &str = "zh";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

fn main() {
    match run() {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(err.exit_code());
        }
    }
}

fn run() -> Result<String, AsrCliError> {
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(String::new());
    }
    if args[0] == "--version" || args[0] == "-V" {
        return Ok(format!(
            "pinvou-asr {}",
            option_env!("CARGO_PKG_VERSION").unwrap_or("dev")
        ));
    }
    let command = args.remove(0);
    match command.to_string_lossy().as_ref() {
        "asr" => {
            let request = AsrRequest::parse(args)?;
            transcribe(request)
        }
        "check" => {
            let backend = resolve_backend().map_err(|err| AsrCliError::MissingRuntime(err.0))?;
            Ok(backend.display_name())
        }
        other => Err(AsrCliError::InvalidArgs(format!(
            "unknown command `{other}`; expected `asr` or `check`"
        ))),
    }
}

fn print_help() {
    println!(
        "pinvou-asr\n\
\n\
Usage:\n\
  pinvou-asr asr --input <wav> [--model <name>] [--lang <lang>] [--timeout-secs <seconds>]\n\
  pinvou-asr check\n\
\n\
Environment:\n\
  PINVOU3_ASR_BACKEND       Exact backend executable to call.\n\
  PINVOU3_ASR_BACKEND_KIND  Backend kind: sensevoice or paddlespeech.\n\
  PINVOU3_SENSEVOICE_MODEL  Exact SenseVoice GGUF model path.\n\
  PINVOU3_SENSEVOICE_VAD    Exact FSMN-VAD GGUF model path.\n\
  PINVOU3_ASR_TIMEOUT_SECS  Default timeout for backend execution.\n\
\n\
Backend discovery order:\n\
  1. PINVOU3_ASR_BACKEND\n\
  2. <pinvou-asr-dir>/llama-funasr-sensevoice(.exe)\n\
  3. <pinvou-asr-dir>/runtime/llama-funasr-sensevoice(.exe)\n\
  4. llama-funasr-sensevoice from PATH\n\
  5. PaddleSpeech-compatible fallback"
    );
}

#[derive(Debug, Clone)]
struct AsrRequest {
    input: PathBuf,
    model: String,
    lang: String,
    timeout: Duration,
}

impl AsrRequest {
    fn parse(args: Vec<std::ffi::OsString>) -> Result<Self, AsrCliError> {
        let mut input = None;
        let mut model = DEFAULT_MODEL.to_string();
        let mut lang = DEFAULT_LANG.to_string();
        let mut timeout = env::var("PINVOU3_ASR_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let mut idx = 0;
        while idx < args.len() {
            let key = args[idx].to_string_lossy();
            let value = |idx: usize, name: &str| -> Result<String, AsrCliError> {
                args.get(idx + 1)
                    .map(|v| v.to_string_lossy().into_owned())
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| AsrCliError::InvalidArgs(format!("missing value for {name}")))
            };
            match key.as_ref() {
                "--input" | "-i" => {
                    input = Some(PathBuf::from(value(idx, "--input")?));
                    idx += 2;
                }
                "--model" | "-m" => {
                    model = value(idx, "--model")?;
                    idx += 2;
                }
                "--lang" | "-l" => {
                    lang = value(idx, "--lang")?;
                    idx += 2;
                }
                "--timeout-secs" => {
                    timeout = value(idx, "--timeout-secs")?
                        .parse::<u64>()
                        .map_err(|_| {
                            AsrCliError::InvalidArgs(
                                "--timeout-secs must be a positive integer".to_string(),
                            )
                        })?;
                    if timeout == 0 {
                        return Err(AsrCliError::InvalidArgs(
                            "--timeout-secs must be greater than 0".to_string(),
                        ));
                    }
                    idx += 2;
                }
                unknown => {
                    return Err(AsrCliError::InvalidArgs(format!(
                        "unknown option `{unknown}`"
                    )));
                }
            }
        }

        let input = input.ok_or_else(|| {
            AsrCliError::InvalidArgs("missing required option --input <wav>".to_string())
        })?;
        if !input.is_file() {
            return Err(AsrCliError::InvalidArgs(format!(
                "input file does not exist: {}",
                input.display()
            )));
        }

        Ok(Self {
            input,
            model,
            lang,
            timeout: Duration::from_secs(timeout),
        })
    }
}

#[derive(Debug, Clone)]
enum Backend {
    SenseVoice { executable: PathBuf },
    PaddleSpeech { executable: PathBuf },
}

impl Backend {
    fn display_name(&self) -> String {
        match self {
            Backend::SenseVoice { executable } => {
                format!("sensevoice: {}", executable.display())
            }
            Backend::PaddleSpeech { executable } => {
                format!("paddlespeech: {}", executable.display())
            }
        }
    }

    fn command(&self, request: &AsrRequest) -> Command {
        match self {
            Backend::SenseVoice { executable } => {
                let runtime_dir = executable.parent().map(Path::to_path_buf);
                let model = resolve_sensevoice_model(&request.model, runtime_dir.as_deref())
                    .unwrap_or_else(|| PathBuf::from(&request.model));
                let vad = resolve_sensevoice_vad(runtime_dir.as_deref());
                let mut command = Command::new(executable);
                command.arg("-m").arg(model).arg("-a").arg(&request.input);
                if let Some(vad) = vad {
                    command.arg("--vad").arg(vad);
                }
                command
            }
            Backend::PaddleSpeech { executable } => {
                let mut command = Command::new(executable);
                command
                    .arg("asr")
                    .arg("--model")
                    .arg(&request.model)
                    .arg("--lang")
                    .arg(&request.lang)
                    .arg("--input")
                    .arg(&request.input);
                command
            }
        }
    }
}

fn transcribe(request: AsrRequest) -> Result<String, AsrCliError> {
    let backend = resolve_backend().map_err(|err| AsrCliError::MissingRuntime(err.0))?;
    if matches!(backend, Backend::SenseVoice { .. })
        && resolve_sensevoice_model(&request.model, backend_dir(&backend).as_deref()).is_none()
    {
        return Err(AsrCliError::MissingRuntime(format!(
            "SenseVoice q8 model not found for `{}`; place sensevoice-small-q8.gguf under models/ or gguf/, or set PINVOU3_SENSEVOICE_MODEL",
            request.model
        )));
    }
    let output = run_backend(&backend, &request)?;
    parse_asr_text(&output.stdout, &output.stderr).ok_or_else(|| {
        AsrCliError::EmptyResult(format!(
            "backend returned no usable text: {}",
            compact_process_output(&output.stdout, &output.stderr)
        ))
    })
}

struct BackendOutput {
    stdout: String,
    stderr: String,
}

fn run_backend(backend: &Backend, request: &AsrRequest) -> Result<BackendOutput, AsrCliError> {
    let mut command = backend.command(request);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_child_console(&mut command);

    let mut child = command.spawn().map_err(|err| {
        AsrCliError::BackendFailed(format!(
            "failed to start backend `{}`: {err}",
            backend.display_name()
        ))
    })?;

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= request.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AsrCliError::Timeout(format!(
                        "backend timed out after {} seconds",
                        request.timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AsrCliError::BackendFailed(format!(
                    "failed while waiting for backend: {err}"
                )));
            }
        }
    };

    let mut stdout = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }

    if !status.success() {
        return Err(AsrCliError::BackendFailed(format!(
            "backend exited with {}: {}",
            status
                .code()
                .map_or_else(|| "unknown".to_string(), |code| code.to_string()),
            compact_process_output(&stdout, &stderr)
        )));
    }

    Ok(BackendOutput { stdout, stderr })
}

#[cfg(windows)]
fn hide_child_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console(_command: &mut Command) {}

#[derive(Debug, Clone)]
struct BackendResolveError(String);

fn resolve_backend() -> Result<Backend, BackendResolveError> {
    if let Ok(raw) = env::var("PINVOU3_ASR_BACKEND") {
        let path = PathBuf::from(raw.trim());
        if path.is_file() || command_name_exists(&path) {
            return Ok(backend_from_path(path));
        }
        return Err(BackendResolveError(format!(
            "PINVOU3_ASR_BACKEND does not exist: {}",
            path.display()
        )));
    }

    let exe_dir = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir {
        for candidate in sibling_sensevoice_candidates(&dir) {
            if candidate.is_file() {
                return Ok(Backend::SenseVoice {
                    executable: candidate,
                });
            }
        }
        for candidate in sibling_paddlespeech_candidates(&dir) {
            if candidate.is_file() {
                return Ok(Backend::PaddleSpeech {
                    executable: candidate,
                });
            }
        }
    }

    if command_name_exists(Path::new("llama-funasr-sensevoice")) {
        return Ok(Backend::SenseVoice {
            executable: PathBuf::from("llama-funasr-sensevoice"),
        });
    }

    if command_name_exists(Path::new("paddlespeech")) {
        return Ok(Backend::PaddleSpeech {
            executable: PathBuf::from("paddlespeech"),
        });
    }

    Err(BackendResolveError(
        "no SenseVoice/FunASR backend found; place llama-funasr-sensevoice next to pinvou-asr.exe, bundle it under runtime/, or set PINVOU3_ASR_BACKEND".to_string(),
    ))
}

fn backend_from_path(path: PathBuf) -> Backend {
    let kind = env::var("PINVOU3_ASR_BACKEND_KIND")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if kind == "paddlespeech" {
        return Backend::PaddleSpeech { executable: path };
    }
    if kind == "sensevoice" {
        return Backend::SenseVoice { executable: path };
    }
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if filename.contains("paddlespeech") {
        Backend::PaddleSpeech { executable: path }
    } else {
        Backend::SenseVoice { executable: path }
    }
}

fn sibling_sensevoice_candidates(dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        candidates.push(dir.join("llama-funasr-sensevoice.exe"));
        candidates.push(dir.join("runtime").join("llama-funasr-sensevoice.exe"));
        candidates.push(
            dir.join("runtime")
                .join("bin")
                .join("llama-funasr-sensevoice.exe"),
        );
        candidates.push(dir.join("bin").join("llama-funasr-sensevoice.exe"));
    }
    #[cfg(not(windows))]
    {
        candidates.push(dir.join("llama-funasr-sensevoice"));
        candidates.push(dir.join("runtime").join("llama-funasr-sensevoice"));
        candidates.push(dir.join("runtime").join("bin").join("llama-funasr-sensevoice"));
        candidates.push(dir.join("bin").join("llama-funasr-sensevoice"));
    }
    candidates
}

fn sibling_paddlespeech_candidates(dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        candidates.push(dir.join("paddlespeech.exe"));
        candidates.push(dir.join("runtime").join("Scripts").join("paddlespeech.exe"));
        candidates.push(dir.join("runtime").join("paddlespeech.exe"));
    }
    #[cfg(not(windows))]
    {
        candidates.push(dir.join("paddlespeech"));
        candidates.push(dir.join("runtime").join("bin").join("paddlespeech"));
    }
    candidates
}

fn backend_dir(backend: &Backend) -> Option<PathBuf> {
    match backend {
        Backend::SenseVoice { executable } | Backend::PaddleSpeech { executable } => {
            executable.parent().map(Path::to_path_buf)
        }
    }
}

fn resolve_sensevoice_model(model: &str, runtime_dir: Option<&Path>) -> Option<PathBuf> {
    if let Ok(path) = env::var("PINVOU3_SENSEVOICE_MODEL") {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let requested = PathBuf::from(model);
    if requested.is_file() {
        return Some(requested);
    }
    let filename = match model {
        "sensevoice" | "sensevoice-q8" | "SenseVoiceSmall-q8" => "sensevoice-small-q8.gguf",
        "sensevoice-f16" | "SenseVoiceSmall-f16" => "sensevoice-small-f16.gguf",
        other if other.ends_with(".gguf") => other,
        _ => "sensevoice-small-q8.gguf",
    };
    let mut candidates = Vec::new();
    if let Some(dir) = runtime_dir {
        candidates.push(dir.join(filename));
        candidates.push(dir.join("models").join(filename));
        candidates.push(dir.join("gguf").join(filename));
        candidates.push(dir.join("runtime").join("models").join(filename));
        candidates.push(dir.join("runtime").join("gguf").join(filename));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn resolve_sensevoice_vad(runtime_dir: Option<&Path>) -> Option<PathBuf> {
    if let Ok(path) = env::var("PINVOU3_SENSEVOICE_VAD") {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let mut candidates = Vec::new();
    if let Some(dir) = runtime_dir {
        for filename in ["fsmn-vad.gguf", "fsmn-vad-q8.gguf"] {
            candidates.push(dir.join(filename));
            candidates.push(dir.join("models").join(filename));
            candidates.push(dir.join("gguf").join(filename));
            candidates.push(dir.join("runtime").join("models").join(filename));
            candidates.push(dir.join("runtime").join("gguf").join(filename));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn command_name_exists(command: &Path) -> bool {
    if command.components().count() > 1 || command.extension().is_some() {
        return command.is_file();
    }
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    let names = executable_names(command.as_os_str());
    env::split_paths(&path).any(|dir| names.iter().any(|name| dir.join(name).is_file()))
}

fn executable_names(command: &OsStr) -> Vec<PathBuf> {
    let raw = command.to_string_lossy();
    #[cfg(windows)]
    {
        if Path::new(raw.as_ref()).extension().is_some() {
            return vec![PathBuf::from(raw.as_ref())];
        }
        let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let mut names = vec![PathBuf::from(raw.as_ref())];
        for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
            let ext = if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            };
            names.push(PathBuf::from(format!("{raw}{ext}")));
        }
        names
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from(raw.as_ref())]
    }
}

fn parse_asr_text(stdout: &str, stderr: &str) -> Option<String> {
    let combined = format!("{stdout}\n{stderr}");
    let result_prefixes = [
        "result:",
        "asr result:",
        "recognition result:",
        "transcription:",
        "transcript:",
        "text:",
        "output:",
    ];

    for line in combined.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        for prefix in result_prefixes {
            if lower.starts_with(prefix) {
                let text = clean_asr_text(line[prefix.len()..].trim());
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        if let Some(text) = parse_json_text_line(line) {
            return Some(text);
        }
        let list_text = clean_asr_text(strip_wrapping_list_quotes(line));
        if list_text != line && !list_text.is_empty() {
            return Some(list_text);
        }
        if looks_like_log_line(&lower) {
            continue;
        }
        if line
            .chars()
            .any(|ch| ch.is_alphabetic() || ('\u{4e00}'..='\u{9fff}').contains(&ch))
        {
            let text = clean_asr_text(line);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn parse_json_text_line(line: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    for key in ["text", "result", "sentence", "transcript"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            let text = clean_asr_text(text);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn clean_asr_text(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text.trim();
    loop {
        let Some(start) = rest.find("<|") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("|>") else {
            out.push_str(&rest[start..]);
            break;
        };
        rest = &after_start[end + 2..];
    }
    out.trim().to_string()
}

fn strip_wrapping_list_quotes(line: &str) -> &str {
    if (line.starts_with("['") && line.ends_with("']"))
        || (line.starts_with("[\"") && line.ends_with("\"]"))
    {
        return line
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_matches('\'')
            .trim_matches('"')
            .trim();
    }
    line
}

fn looks_like_log_line(lower: &str) -> bool {
    lower.contains("error")
        || lower.contains("warning")
        || lower.contains("paddlespeech")
        || lower.contains("sensevoice")
        || lower.contains("funasr")
        || lower.contains("gguf")
        || lower.contains("python")
        || lower.contains("download")
        || lower.starts_with('[')
}

fn compact_process_output(stdout: &str, stderr: &str) -> String {
    let joined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if joined.chars().count() <= 2000 {
        return joined;
    }
    let tail = joined
        .chars()
        .rev()
        .take(2000)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

#[derive(Debug)]
enum AsrCliError {
    InvalidArgs(String),
    MissingRuntime(String),
    BackendFailed(String),
    Timeout(String),
    EmptyResult(String),
}

impl AsrCliError {
    fn exit_code(&self) -> i32 {
        match self {
            AsrCliError::InvalidArgs(_) => 2,
            AsrCliError::MissingRuntime(_) => 3,
            AsrCliError::BackendFailed(_) => 4,
            AsrCliError::Timeout(_) => 5,
            AsrCliError::EmptyResult(_) => 6,
        }
    }
}

impl std::fmt::Display for AsrCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsrCliError::InvalidArgs(message) => write!(f, "invalid arguments: {message}"),
            AsrCliError::MissingRuntime(message) => write!(f, "missing ASR runtime: {message}"),
            AsrCliError::BackendFailed(message) => write!(f, "ASR backend failed: {message}"),
            AsrCliError::Timeout(message) => write!(f, "ASR timeout: {message}"),
            AsrCliError::EmptyResult(message) => write!(f, "ASR empty result: {message}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_backend_output() {
        assert_eq!(
            parse_asr_text("hello from backend\n", ""),
            Some("hello from backend".to_string())
        );
    }

    #[test]
    fn parses_list_backend_output() {
        assert_eq!(
            parse_asr_text("[INFO] load\n['你好 pinvou']\n", ""),
            Some("你好 pinvou".to_string())
        );
    }

    #[test]
    fn parses_prefixed_backend_output() {
        assert_eq!(
            parse_asr_text("ASR result: hello\n", ""),
            Some("hello".to_string())
        );
    }

    #[test]
    fn parses_sensevoice_tagged_output() {
        assert_eq!(
            parse_asr_text("<|zh|><|NEUTRAL|><|Speech|><|woitn|>你好 pinvou\n", ""),
            Some("你好 pinvou".to_string())
        );
    }

    #[test]
    fn parses_json_output() {
        assert_eq!(
            parse_asr_text(r#"{"text":"<|zh|><|Speech|>你好"}"#, ""),
            Some("你好".to_string())
        );
    }

    #[test]
    fn rejects_missing_input_arg() {
        let err = AsrRequest::parse(vec![]).unwrap_err();
        assert!(matches!(err, AsrCliError::InvalidArgs(_)));
    }
}
