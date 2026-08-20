use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::capture::{JsonlRecorder, create_capture_file};
use crate::clock::{HostMonotonicClock, MonotonicClock};
use crate::protocol::CaptureChannel;
use crate::s2::{
    EventSizeDistribution, PerformanceEvidence, S2Evidence, S2Report, ScenarioEvidence,
    TerminalState, validate,
};

const CAPTURE_FILE: &str = "capture.jsonl";
const EVIDENCE_FILE: &str = "evidence.json";
const REPORT_FILE: &str = "validation-report.json";
const SUMMARY_FILE: &str = "summary.txt";
const APPROVAL_MARKER_NAME: &str = ".codex-s2-approval-marker";
const APPROVAL_MARKER_CONTENT: &str = "S2_APPROVED";
const APPROVAL_MARKER_BYTES: &[u8] = APPROVAL_MARKER_CONTENT.as_bytes();
const APPROVAL_MARKER_MAX_BYTES: u64 = 64;

#[derive(Clone, Debug)]
pub struct S2RunConfig {
    pub output_dir: Option<PathBuf>,
    pub executable: Option<OsString>,
    pub trusted_approval_wrapper: Option<PathBuf>,
    pub model: Option<String>,
    pub scenario_timeout: Duration,
    pub global_timeout: Duration,
}

#[derive(Debug)]
pub struct S2RunOutcome {
    pub output_dir: PathBuf,
    pub report: S2Report,
}

#[derive(Clone, Copy, Debug)]
struct RunnerThresholds {
    a_min_span: Duration,
    a_min_bytes: u64,
    a_min_events: usize,
    b_min_bytes: u64,
    b_min_events: usize,
    d_min_bytes: u64,
    d_min_events: usize,
}

impl RunnerThresholds {
    const PRODUCTION: Self = Self {
        a_min_span: Duration::from_secs(30),
        a_min_bytes: 2 * 1024,
        a_min_events: 8,
        b_min_bytes: 32 * 1024,
        b_min_events: 32,
        d_min_bytes: 2 * 1024,
        d_min_events: 8,
    };

    #[cfg(debug_assertions)]
    const FAST_TEST: Self = Self {
        a_min_span: Duration::ZERO,
        a_min_bytes: 64,
        a_min_events: 2,
        b_min_bytes: 64,
        b_min_events: 2,
        d_min_bytes: 64,
        d_min_events: 2,
    };
}

#[derive(Clone, Debug)]
struct ContentEvent {
    timestamp_ns: u64,
    bytes: u64,
}

#[derive(Debug)]
enum Inbound {
    Frame { timestamp_ns: u64, value: Value },
    Malformed(String),
    Closed,
}

type Recorder = JsonlRecorder<BufWriter<std::fs::File>, Box<dyn FnMut() -> u64 + Send>>;
const INBOUND_CAPACITY: usize = 1024;

pub fn run_s2(config: S2RunConfig) -> Result<S2RunOutcome> {
    run_s2_with_thresholds(config, RunnerThresholds::PRODUCTION)
}

/// Deterministic debug-build seam for the fake app-server integration tests.
/// Production/release builds and the CLI cannot lower the real S2 gates.
#[cfg(debug_assertions)]
pub fn run_s2_for_test(config: S2RunConfig) -> Result<S2RunOutcome> {
    run_s2_with_thresholds(config, RunnerThresholds::FAST_TEST)
}

fn run_s2_with_thresholds(
    config: S2RunConfig,
    thresholds: RunnerThresholds,
) -> Result<S2RunOutcome> {
    let output_dir = match config.output_dir.clone() {
        Some(path) => path,
        None => default_output_dir(),
    };
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;
    let workspace = output_dir.join("workspace");
    if workspace.exists() {
        bail!(
            "isolated workspace already exists; choose a fresh output directory: {}",
            workspace.display()
        );
    }
    std::fs::create_dir_all(&workspace).with_context(|| {
        format!(
            "failed to create isolated workspace {}",
            workspace.display()
        )
    })?;

    let explicit_approval_wrapper =
        validate_explicit_approval_wrapper(config.trusted_approval_wrapper.as_deref())?;
    let global_deadline = Instant::now() + config.global_timeout;
    let mut evidence = empty_evidence();
    let execution = SafeInvocation::resolve(config.executable.as_ref()).and_then(|invocation| {
        verify_executable_version(&invocation, global_deadline).and_then(|_| {
            execute(
                &config,
                &invocation,
                &output_dir,
                &workspace,
                &mut evidence,
                thresholds,
                global_deadline,
                explicit_approval_wrapper.as_ref(),
            )
        })
    });
    if let Err(error) = &execution {
        classify_failure(error, &mut evidence);
    }
    let report = validate(evidence.clone());
    write_artifacts(&output_dir, &evidence, &report, execution.as_ref().err())?;
    if let Err(error) = execution {
        return Err(error.context(format!("S2 artifacts: {}", output_dir.display())));
    }
    if !report.valid {
        bail!("S2 run is INVALID; artifacts: {}", output_dir.display());
    }
    Ok(S2RunOutcome { output_dir, report })
}

#[derive(Clone, Debug)]
struct SafeInvocation {
    program: OsString,
    prefix_args: Vec<OsString>,
    #[cfg(windows)]
    cmd_script: Option<PathBuf>,
}

impl SafeInvocation {
    fn resolve(explicit: Option<&OsString>) -> Result<Self> {
        #[cfg(windows)]
        {
            let candidate = match explicit {
                Some(value) => PathBuf::from(value),
                None => find_windows_path_command("codex.cmd")?,
            };
            if candidate
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("cmd"))
            {
                let script = resolve_safe_cmd_script(&candidate)?;
                return Ok(Self {
                    program: trusted_system_cmd()?.into_os_string(),
                    prefix_args: ["/d", "/s", "/c"].map(OsString::from).to_vec(),
                    cmd_script: Some(script),
                });
            }
            if explicit.is_none()
                || !candidate
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
            {
                bail!("Windows executable override must be a regular .exe or .cmd file");
            }
            let candidate = if candidate.components().count() == 1 {
                find_windows_path_command(
                    candidate
                        .to_str()
                        .context("explicit .exe filename was not valid Unicode")?,
                )?
            } else {
                candidate
            };
            let candidate = candidate.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize .exe executable {}",
                    candidate.display()
                )
            })?;
            if !candidate.is_file() {
                bail!("Windows executable override was not a regular .exe file");
            }
            return Ok(Self {
                program: candidate.into_os_string(),
                prefix_args: Vec::new(),
                cmd_script: None,
            });
        }
        #[cfg(not(windows))]
        Ok(Self {
            program: explicit.cloned().unwrap_or_else(|| OsString::from("codex")),
            prefix_args: Vec::new(),
        })
    }

    fn command(&self, requested_args: &[&str]) -> Result<Command> {
        let mut command = Command::new(&self.program);
        #[cfg(windows)]
        if let Some(script) = self.cmd_script.as_ref() {
            use std::os::windows::process::CommandExt;

            if requested_args.iter().any(|arg| {
                arg.is_empty()
                    || !arg
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            }) {
                bail!("unsafe requested argument for cmd invocation");
            }
            let script = script
                .to_str()
                .context("canonical .cmd path was not valid Unicode")?;
            let mut line = String::new();
            for prefix in &self.prefix_args {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(
                    prefix
                        .to_str()
                        .context("cmd prefix argument was not valid Unicode")?,
                );
            }
            line.push_str(" \"\"");
            line.push_str(script);
            line.push('"');
            for arg in requested_args {
                line.push(' ');
                line.push_str(arg);
            }
            line.push('"');
            // cmd.exe has its own quoting grammar; Rust's ordinary Windows argv
            // escaping would insert backslashes that cmd treats literally.
            // The script path was canonicalized and metacharacter-checked above,
            // and requested arguments are restricted to a fixed safe alphabet.
            command.raw_arg(line);
            return Ok(command);
        }
        command.args(&self.prefix_args);
        command.args(requested_args);
        Ok(command)
    }
}

#[cfg(windows)]
fn find_windows_path_command(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").context("PATH is unavailable for codex.cmd lookup")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("{name} was not found on PATH")
}

#[cfg(windows)]
fn resolve_safe_cmd_script(candidate: &Path) -> Result<PathBuf> {
    let candidate = if candidate.components().count() == 1 {
        find_windows_path_command(
            candidate
                .to_str()
                .context("explicit .cmd filename was not valid Unicode")?,
        )?
    } else {
        candidate.to_owned()
    };
    let canonical = candidate.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize .cmd executable {}",
            candidate.display()
        )
    })?;
    if !canonical.is_file()
        || !canonical
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("cmd"))
    {
        bail!("resolved command is not a regular .cmd file");
    }
    let canonical = normalize_windows_command_path(canonical)?;
    let rendered = canonical
        .to_str()
        .context("canonical .cmd path was not valid Unicode")?;
    if rendered.chars().any(|character| {
        matches!(
            character,
            '\r' | '\n' | '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!' | '(' | ')' | ';'
        )
    }) {
        bail!("unsafe command path metacharacter in .cmd executable");
    }
    Ok(canonical)
}

#[cfg(windows)]
fn normalize_windows_command_path(canonical: PathBuf) -> Result<PathBuf> {
    let rendered = canonical
        .to_str()
        .context("canonical .cmd path was not valid Unicode")?;
    if let Some(path) = rendered.strip_prefix(r"\\?\UNC\") {
        return Ok(PathBuf::from(format!(r"\\{path}")));
    }
    if let Some(path) = rendered.strip_prefix(r"\\?\") {
        return Ok(PathBuf::from(path));
    }
    Ok(canonical)
}

fn verify_executable_version(invocation: &SafeInvocation, global_deadline: Instant) -> Result<()> {
    use std::io::Read;

    remaining_global(global_deadline)?;
    let mut process = invocation.command(&["--version"])?;
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut contained = spawn_contained(process).with_context(|| {
        format!(
            "failed to launch version preflight for {:?}",
            invocation.program
        )
    })?;
    let stdout = contained
        .child
        .stdout
        .take()
        .context("version stdout unavailable")?;
    let stderr = contained
        .child
        .stderr
        .take()
        .context("version stderr unavailable")?;
    let (output_tx, output_rx) = mpsc::sync_channel(2);
    let (done_tx, done_rx) = mpsc::sync_channel(2);
    let mut readers = Vec::new();
    for (label, stream) in [
        ("stdout", Box::new(stdout) as Box<dyn Read + Send>),
        ("stderr", Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = output_tx.clone();
        let done = done_tx.clone();
        readers.push(thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.take(4097).read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send((label, result));
            let _ = done.send(label);
        }));
    }
    drop(output_tx);
    drop(done_tx);
    let deadline = global_deadline.min(Instant::now() + Duration::from_secs(5));
    let status_result = loop {
        if let Some(status) = contained
            .child
            .try_wait()
            .context("failed polling version preflight")?
        {
            break Ok(status);
        }
        if Instant::now() >= deadline {
            break Err(anyhow!("version preflight timeout"));
        }
        thread::sleep(Duration::from_millis(5));
    };
    // Always tear down the contained tree before waiting for EOF: a short-lived
    // version process may have spawned descendants that inherited its pipes.
    contained.terminate_and_wait_bounded();
    let mut stdout_bytes = None;
    let read_deadline = global_deadline.min(Instant::now() + Duration::from_secs(1));
    for _ in 0..2 {
        let remaining = read_deadline
            .checked_duration_since(Instant::now())
            .context("version output reader timeout")?;
        let (label, bytes) = output_rx
            .recv_timeout(remaining)
            .context("version output reader timeout")?;
        let bytes = bytes.context("version output read failed")?;
        if bytes.len() > 4096 {
            bail!("version output exceeded 4096 bytes");
        }
        if label == "stdout" {
            stdout_bytes = Some(bytes);
        }
    }
    let mut completed = std::collections::HashSet::new();
    let join_deadline = Instant::now() + Duration::from_secs(1);
    while completed.len() < readers.len() {
        let Some(remaining) = join_deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match done_rx.recv_timeout(remaining) {
            Ok(label) => {
                completed.insert(label);
            }
            Err(_) => break,
        }
    }
    for (index, handle) in readers.into_iter().enumerate() {
        let label = if index == 0 { "stdout" } else { "stderr" };
        if completed.contains(label) {
            let _ = handle.join();
        }
    }
    let status = status_result?;
    if !status.success() {
        bail!("version preflight exited with status {status}");
    }
    let output = String::from_utf8(stdout_bytes.unwrap_or_default())
        .context("version output was not UTF-8")?;
    if output.trim() != "codex-cli 0.139.0" {
        bail!("version preflight requires exact codex-cli 0.139.0");
    }
    Ok(())
}

fn execute(
    config: &S2RunConfig,
    invocation: &SafeInvocation,
    output_dir: &Path,
    workspace: &Path,
    evidence: &mut S2Evidence,
    thresholds: RunnerThresholds,
    global_deadline: Instant,
    explicit_approval_wrapper: Option<&TrustedApprovalWrapper>,
) -> Result<()> {
    remaining_global(global_deadline)?;
    let clock = Arc::new(HostMonotonicClock::new()?);
    let recorder_clock = Arc::clone(&clock);
    let recorder: Arc<Mutex<Recorder>> = Arc::new(Mutex::new(JsonlRecorder::new(
        BufWriter::new(create_capture_file(&output_dir.join(CAPTURE_FILE))?),
        Box::new(move || recorder_clock.now_ns().expect("monotonic clock failed")),
    )));
    let mut session = Session::spawn(
        invocation.command(&["app-server", "--stdio"])?,
        recorder,
        global_deadline,
    )?;

    let result = (|| {
        let initialized = session.request(
            "initialize",
            json!({"clientInfo":{"name":"codex-s2-runner","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
            config.scenario_timeout,
        )?;
        validate_initialize(&initialized)?;
        session.notify("initialized", json!({}))?;
        let account = session.request(
            "account/read",
            json!({"refreshToken":false}),
            config.scenario_timeout,
        )?;
        validate_account(&account)?;
        let limits = session.request(
            "account/rateLimits/read",
            json!({}),
            config.scenario_timeout,
        )?;
        validate_rate_limits(&limits)?;
        if quota_exhausted(limits.get("result").unwrap_or(&Value::Null)) {
            bail!("quota exhausted: account rate limit is reached");
        }

        let mut scenario_b_content = Vec::new();
        let mut interrupt_response_latency_ms = None;
        let mut interrupt_terminal_latency_ms = None;
        let scenario_shell = trusted_scenario_shell()?;
        let approval_command = approval_command()?;
        #[cfg(windows)]
        let approval_wrapper_shell = match explicit_approval_wrapper {
            Some(wrapper) => Some(wrapper.path.clone()),
            None => auto_trusted_approval_wrapper_shell()?,
        };
        #[cfg(windows)]
        let approval_wrapper = approval_wrapper_shell
            .map(|shell| approval_wrapper_candidate(&shell, &approval_command))
            .transpose()?;
        #[cfg(not(windows))]
        let approval_wrapper: Option<String> = None;
        #[cfg(not(windows))]
        let _ = explicit_approval_wrapper;
        let corpus = restricted_ascii_corpus();
        for name in ["A", "B", "C", "D"] {
            let scenario_deadline = Instant::now() + config.scenario_timeout;
            if name == "C" {
                ensure_approval_marker_absent_bounded(workspace, scenario_deadline)?;
            }
            let approval_policy = if name == "C" { "on-request" } else { "never" };
            let sandbox = if name == "C" {
                "read-only"
            } else {
                "workspace-write"
            };
            let mut thread_params = json!({
                "cwd": workspace,
                "approvalPolicy": approval_policy,
                "sandbox": sandbox,
                "ephemeral": true
            });
            if let Some(model) = config.model.as_ref() {
                thread_params["model"] = Value::String(model.clone());
            }
            let started = session.request(
                "thread/start",
                thread_params,
                remaining_until(scenario_deadline)?,
            )?;
            let thread_id = started
                .pointer("/result/thread/id")
                .and_then(Value::as_str)
                .with_context(|| {
                    format!("scenario {name}: thread/start response missing thread.id")
                })?
                .to_owned();
            let prompt = scenario_prompt(name, &approval_command, &corpus, &scenario_shell)?;
            let mut turn_params = json!({
                "threadId":thread_id,
                "cwd":workspace,
                "input":[{"type":"text","text":prompt}]
            });
            if name == "C" {
                turn_params["approvalPolicy"] = json!("on-request");
                turn_params["sandboxPolicy"] = json!({"type":"readOnly"});
                ensure_approval_marker_absent_bounded(workspace, scenario_deadline)?;
            }
            let turn = session.request(
                "turn/start",
                turn_params,
                remaining_until(scenario_deadline)?,
            )?;
            let turn_id = turn
                .pointer("/result/turn/id")
                .and_then(Value::as_str)
                .with_context(|| format!("scenario {name}: turn/start response missing turn.id"))?
                .to_owned();
            let observed = session.drive_scenario(
                name,
                &thread_id,
                &turn_id,
                workspace,
                &approval_command,
                approval_wrapper.as_deref(),
                explicit_approval_wrapper,
                scenario_deadline,
                thresholds,
            )?;
            if name == "C" && observed.evidence.terminal_state == TerminalState::Completed {
                verify_approval_marker_bounded(workspace, scenario_deadline)?;
            }
            if name == "B" {
                scenario_b_content.extend(observed.content.iter().cloned());
            }
            if name == "D" {
                interrupt_response_latency_ms = observed.interrupt_response_latency_ms;
                interrupt_terminal_latency_ms = observed.interrupt_terminal_latency_ms;
            }
            evidence.scenarios = evidence
                .scenarios
                .iter()
                .cloned()
                .map(|item| {
                    if item.name == name {
                        observed.evidence.clone()
                    } else {
                        item
                    }
                })
                .collect();
        }
        evidence.performance = Some(performance(&scenario_b_content)?);
        evidence.candidate_percentiles = Some(json!({
            "content_event_samples": scenario_b_content.len(),
            "merge_rate": evidence.performance.as_ref().map(|p| p.merge_output_events as f64 / p.merge_input_events as f64),
            "interrupt_response_latency_ms": interrupt_response_latency_ms,
            "interrupt_terminal_latency_ms": interrupt_terminal_latency_ms
        }));
        Ok(())
    })();
    session.stop();
    result
}

struct ObservedScenario {
    evidence: ScenarioEvidence,
    content: Vec<ContentEvent>,
    interrupt_response_latency_ms: Option<f64>,
    interrupt_terminal_latency_ms: Option<f64>,
}

struct ContainedChild {
    child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    process_group: i32,
}

impl ContainedChild {
    fn terminate_tree(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            if !self.job.is_null() {
                let _ = TerminateJobObject(self.job, 1);
                CloseHandle(self.job);
                self.job = std::ptr::null_mut();
            }
        }
        #[cfg(unix)]
        unsafe {
            let _ = kill(-self.process_group, 9);
        }
        let _ = self.child.kill();
    }

    fn terminate_and_wait_bounded(&mut self) {
        self.terminate_tree();
        wait_child_bounded(&mut self.child);
    }
}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        self.terminate_and_wait_bounded();
    }
}

struct Session {
    contained: ContainedChild,
    stdin: ChildStdin,
    incoming: mpsc::Receiver<Inbound>,
    recorder: Arc<Mutex<Recorder>>,
    next_id: u64,
    pending: VecDeque<(u64, Value)>,
    global_deadline: Instant,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    reader_done: mpsc::Receiver<&'static str>,
    inbound_overflow: Arc<AtomicBool>,
}

fn try_emit(sender: &mpsc::SyncSender<Inbound>, overflow: &AtomicBool, event: Inbound) -> bool {
    match sender.try_send(event) {
        Ok(()) => true,
        Err(mpsc::TrySendError::Full(_)) => {
            overflow.store(true, Ordering::Release);
            true
        }
        Err(mpsc::TrySendError::Disconnected(_)) => false,
    }
}

impl Session {
    fn spawn(
        mut process: Command,
        recorder: Arc<Mutex<Recorder>>,
        global_deadline: Instant,
    ) -> Result<Self> {
        process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut contained = spawn_contained(process).context("failed to launch app-server")?;
        let stdin = contained
            .child
            .stdin
            .take()
            .context("app-server stdin unavailable")?;
        let stdout = contained
            .child
            .stdout
            .take()
            .context("app-server stdout unavailable")?;
        let stderr = contained
            .child
            .stderr
            .take()
            .context("app-server stderr unavailable")?;
        let (tx, incoming) = mpsc::sync_channel(INBOUND_CAPACITY);
        let inbound_overflow = Arc::new(AtomicBool::new(false));
        let (reader_done_tx, reader_done) = mpsc::sync_channel(2);
        let stderr_tx = tx.clone();
        let stderr_overflow = Arc::clone(&inbound_overflow);
        let stdout_overflow = Arc::clone(&inbound_overflow);
        let stdout_done = reader_done_tx.clone();
        let stdout_recorder = Arc::clone(&recorder);
        let stdout_thread = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        try_emit(&tx, &stdout_overflow, Inbound::Malformed(error.to_string()));
                        break;
                    }
                };
                let timestamp_ns = match stdout_recorder.lock() {
                    Ok(mut guard) => {
                        match guard.record_timestamped(CaptureChannel::ServerToClient, &line) {
                            Ok(timestamp) => timestamp,
                            Err(error) => {
                                try_emit(
                                    &tx,
                                    &stdout_overflow,
                                    Inbound::Malformed(format!(
                                        "raw capture write failed: {error}"
                                    )),
                                );
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Malformed("raw capture recorder lock poisoned".to_owned()),
                        );
                        break;
                    }
                };
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) if value.is_object() => {
                        if !try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Frame {
                                timestamp_ns,
                                value,
                            },
                        ) {
                            break;
                        }
                    }
                    Ok(_) => {
                        try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Malformed("server frame was not an object".to_owned()),
                        );
                        break;
                    }
                    Err(error) => {
                        try_emit(
                            &tx,
                            &stdout_overflow,
                            Inbound::Malformed(format!("malformed server JSON: {error}")),
                        );
                        break;
                    }
                }
            }
            try_emit(&tx, &stdout_overflow, Inbound::Closed);
            let _ = stdout_done.send("stdout");
        });
        let stderr_recorder = Arc::clone(&recorder);
        let stderr_done = reader_done_tx;
        let stderr_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let line = match read_line_checked(&mut reader) {
                    Ok(Some(line)) => line,
                    Ok(None) => break,
                    Err(error) => {
                        try_emit(
                            &stderr_tx,
                            &stderr_overflow,
                            Inbound::Malformed(format!("stderr read failed: {error}")),
                        );
                        break;
                    }
                };
                let result = stderr_recorder
                    .lock()
                    .map_err(|_| anyhow!("raw capture recorder lock poisoned"))
                    .and_then(|mut guard| guard.record(CaptureChannel::Stderr, &line));
                if let Err(error) = result {
                    try_emit(
                        &stderr_tx,
                        &stderr_overflow,
                        Inbound::Malformed(format!("raw stderr capture write failed: {error}")),
                    );
                    break;
                }
            }
            let _ = stderr_done.send("stderr");
        });
        Ok(Self {
            contained,
            stdin,
            incoming,
            recorder,
            next_id: 1,
            pending: VecDeque::new(),
            global_deadline,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            reader_done,
            inbound_overflow,
        })
    }

    fn send(&mut self, value: &Value) -> Result<u64> {
        let line = serde_json::to_string(value)?;
        let timestamp_ns = self
            .recorder
            .lock()
            .map_err(|_| anyhow!("capture recorder lock poisoned"))?
            .record_timestamped(CaptureChannel::ClientToServer, &line)?;
        writeln!(self.stdin, "{line}").context("failed writing app-server stdin")?;
        self.stdin
            .flush()
            .context("failed flushing app-server stdin")?;
        Ok(timestamp_ns)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"method":method,"params":params}))
            .map(|_| ())
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let (timestamp_ns, frame) = self.recv_wire(deadline)?;
            if frame.get("id") == Some(&json!(id)) && frame.get("method").is_none() {
                if let Some(error) = frame.get("error") {
                    bail!("{method} error: {}", sanitized_error(error));
                }
                if frame.get("result").is_none() {
                    bail!("{method} response missing result");
                }
                return Ok(frame);
            }
            if frame.get("id").is_some() && frame.get("method").is_some() {
                if method == "turn/start" {
                    if self.pending.len() >= INBOUND_CAPACITY {
                        bail!("protocol error: pending frame queue overflow");
                    }
                    self.pending.push_back((timestamp_ns, frame));
                    continue;
                }
                self.reject_server_request(&frame)?;
                bail!("unexpected server request while waiting for {method}");
            }
            fail_on_error_notification(&frame)?;
            if method == "turn/start" {
                if self.pending.len() >= INBOUND_CAPACITY {
                    bail!("protocol error: pending frame queue overflow");
                }
                self.pending.push_back((timestamp_ns, frame));
            }
        }
    }

    fn drive_scenario(
        &mut self,
        name: &str,
        thread_id: &str,
        turn_id: &str,
        workspace: &Path,
        approval_command: &str,
        approval_wrapper: Option<&str>,
        explicit_approval_wrapper: Option<&TrustedApprovalWrapper>,
        deadline: Instant,
        thresholds: RunnerThresholds,
    ) -> Result<ObservedScenario> {
        let mut content = Vec::new();
        let mut approval_seen = false;
        let mut interrupt_response_seen = false;
        let mut interrupt_id = None;
        let mut interrupt_request_ns = None;
        let mut interrupt_response_latency_ms = None;
        let mut interrupt_terminal_latency_ms = None;
        let mut pending_terminal = None;
        loop {
            let (timestamp_ns, frame) = self.recv(deadline)?;
            fail_on_error_notification(&frame)?;
            if frame.get("id").is_some() && frame.get("method").is_some() {
                if name != "C" || approval_seen {
                    self.reject_server_request(&frame)?;
                    bail!("scenario {name}: unexpected or duplicate approval request");
                }
                self.approve_exact_command(
                    &frame,
                    thread_id,
                    turn_id,
                    workspace,
                    approval_command,
                    approval_wrapper,
                    explicit_approval_wrapper,
                    deadline,
                )?;
                approval_seen = true;
                continue;
            }
            if let Some(expected) = interrupt_id {
                if frame.get("id") == Some(&json!(expected)) && frame.get("method").is_none() {
                    if frame.get("error").is_some() || frame.get("result").is_none() {
                        bail!("scenario D: malformed interrupt response");
                    }
                    interrupt_response_seen = true;
                    interrupt_response_latency_ms = interrupt_request_ns.map(|request_ns: u64| {
                        timestamp_ns.saturating_sub(request_ns) as f64 / 1_000_000.0
                    });
                    if pending_terminal.is_some() {
                        break;
                    }
                    continue;
                }
            }
            if let Some(bytes) = delta_bytes(&frame, thread_id, turn_id) {
                content.push(ContentEvent {
                    timestamp_ns,
                    bytes,
                });
                let content_bytes = content.iter().map(|event| event.bytes).sum::<u64>();
                if name == "D"
                    && interrupt_id.is_none()
                    && content.len() >= thresholds.d_min_events
                    && content_bytes >= thresholds.d_min_bytes
                {
                    let id = self.next_id;
                    self.next_id += 1;
                    let request_ns = self.send(&json!({"id":id,"method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn_id}}))?;
                    interrupt_request_ns = Some(request_ns);
                    interrupt_id = Some(id);
                }
            }
            if let Some(status) = terminal_status(&frame, thread_id, turn_id) {
                let terminal_state = match status {
                    "completed" => TerminalState::Completed,
                    "interrupted" => TerminalState::Interrupted,
                    _ => TerminalState::Failed,
                };
                if name == "D" && interrupt_request_ns.is_some() {
                    interrupt_terminal_latency_ms = interrupt_request_ns.map(|request_ns| {
                        timestamp_ns.saturating_sub(request_ns) as f64 / 1_000_000.0
                    });
                    pending_terminal = Some(terminal_state);
                    if !interrupt_response_seen {
                        continue;
                    }
                    break;
                }
                pending_terminal = Some(terminal_state);
                break;
            }
        }
        let terminal_state = pending_terminal.unwrap_or(TerminalState::Missing);
        let bytes: u64 = content.iter().map(|event| event.bytes).sum();
        let span = content
            .first()
            .zip(content.last())
            .map(|(first, last)| {
                Duration::from_nanos(last.timestamp_ns.saturating_sub(first.timestamp_ns))
            })
            .unwrap_or_default();
        let r1_sufficient = match name {
            "A" => {
                content.len() >= thresholds.a_min_events
                    && bytes >= thresholds.a_min_bytes
                    && span >= thresholds.a_min_span
            }
            "B" => content.len() >= thresholds.b_min_events && bytes >= thresholds.b_min_bytes,
            "D" => content.len() >= thresholds.d_min_events && bytes >= thresholds.d_min_bytes,
            _ => false,
        };
        Ok(ObservedScenario {
            evidence: ScenarioEvidence {
                name: name.to_owned(),
                turn_completed: terminal_state == TerminalState::Completed,
                terminal_state,
                first_delta_seen: !content.is_empty(),
                r1_sufficient,
                approval_seen,
                interrupt_response_seen,
            },
            content,
            interrupt_response_latency_ms,
            interrupt_terminal_latency_ms,
        })
    }

    fn approve_exact_command(
        &mut self,
        frame: &Value,
        thread_id: &str,
        turn_id: &str,
        workspace: &Path,
        command: &str,
        wrapper_candidate: Option<&str>,
        explicit_wrapper: Option<&TrustedApprovalWrapper>,
        deadline: Instant,
    ) -> Result<()> {
        let id = frame.get("id").cloned().context("approval missing id")?;
        let method = frame.get("method").and_then(Value::as_str);
        let params = frame.get("params").context("approval missing params")?;
        let cwd = params.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        let workspace_root = workspace.canonicalize().ok();
        let canonical_cwd = cwd.as_deref().and_then(|path| path.canonicalize().ok());
        let raw_command = params.get("command").and_then(Value::as_str);
        let action_consistency = params.get("commandActions").map(|value| {
            value.as_array().is_some_and(|actions| {
                actions.len() == 1
                    && actions[0].get("command").and_then(Value::as_str) == Some(command)
            })
        });
        let direct_command =
            raw_command == Some(command) && action_consistency.is_none_or(|consistent| consistent);
        let exact_wrapped_command = wrapper_candidate.is_some_and(|wrapper| {
            raw_command == Some(wrapper)
                && action_consistency == Some(true)
                && explicit_wrapper.map_or(true, TrustedApprovalWrapper::path_identity_matches)
        });
        let safe = method == Some("item/commandExecution/requestApproval")
            && params.get("threadId").and_then(Value::as_str) == Some(thread_id)
            && params.get("turnId").and_then(Value::as_str) == Some(turn_id)
            && (direct_command || exact_wrapped_command)
            && workspace_root
                .as_ref()
                .is_some_and(|root| canonical_cwd.as_ref().is_some_and(|path| path == root));
        if !safe {
            self.send(&json!({"id":id,"result":{"decision":"cancel"}}))?;
            bail!(
                "unexpected approval rejected: method/command/cwd was outside the exact allowlist"
            );
        }
        if let Err(error) = ensure_approval_marker_absent_bounded(workspace, deadline) {
            self.send(&json!({"id":id,"result":{"decision":"cancel"}}))?;
            return Err(error);
        }
        self.send(&json!({"id":id,"result":{"decision":"accept"}}))
            .map(|_| ())
    }

    fn reject_server_request(&mut self, frame: &Value) -> Result<()> {
        if let Some(id) = frame.get("id") {
            self.send(&json!({"id":id,"error":{"code":-32601,"message":"S2 runner rejected unexpected server request"}}))?;
        }
        Ok(())
    }

    fn recv(&mut self, scenario_deadline: Instant) -> Result<(u64, Value)> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(frame);
        }
        self.recv_wire(scenario_deadline)
    }

    fn recv_wire(&mut self, scenario_deadline: Instant) -> Result<(u64, Value)> {
        if self.inbound_overflow.load(Ordering::Acquire) {
            bail!("protocol error: bounded inbound queue overflow");
        }
        let deadline = scenario_deadline.min(self.global_deadline);
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("S2 timeout expired")?;
        match self.incoming.recv_timeout(remaining) {
            Ok(Inbound::Frame {
                timestamp_ns,
                value,
            }) => Ok((timestamp_ns, value)),
            Ok(Inbound::Malformed(error)) => bail!("protocol error: {error}"),
            Ok(Inbound::Closed) => bail!("protocol error: app-server stdout closed"),
            Err(mpsc::RecvTimeoutError::Timeout) => bail!("S2 timeout expired"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("protocol error: app-server reader stopped")
            }
        }
    }

    fn stop(&mut self) {
        self.contained.terminate_and_wait_bounded();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut done = std::collections::HashSet::new();
        while done.len() < 2 {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match self.reader_done.recv_timeout(remaining) {
                Ok(label) => {
                    done.insert(label);
                }
                Err(_) => break,
            }
        }
        if done.contains("stdout") {
            if let Some(handle) = self.stdout_thread.take() {
                let _ = handle.join();
            }
        } else {
            self.stdout_thread.take();
        }
        if done.contains("stderr") {
            if let Some(handle) = self.stderr_thread.take() {
                let _ = handle.join();
            }
        } else {
            self.stderr_thread.take();
        }
    }
}

fn read_line_checked(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    while matches!(line.as_bytes().last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    Ok(Some(line))
}

fn remaining_until(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .context("S2 scenario timeout expired")
}

fn remaining_global(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .context("S2 global timeout expired")
}

fn spawn_contained(mut process: Command) -> Result<ContainedChild> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpgid is async-signal-safe and called before exec in the child.
        unsafe {
            process.pre_exec(|| {
                if setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        process.creation_flags(CREATE_SUSPENDED);
    }
    let mut child = process.spawn()?;
    #[cfg(windows)]
    let job = match contain_and_resume_windows_child(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            wait_child_bounded(&mut child);
            return Err(error);
        }
    };
    #[cfg(unix)]
    let process_group = child.id() as i32;
    Ok(ContainedChild {
        child,
        #[cfg(windows)]
        job,
        #[cfg(unix)]
        process_group,
    })
}

fn wait_child_bounded(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                break;
            }
        }
    }
}

#[cfg(windows)]
fn contain_and_resume_windows_child(
    child: &Child,
) -> Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: null security/name create an unnamed job; child handle is live.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        bail!(
            "CreateJobObjectW failed: {}",
            std::io::Error::last_os_error()
        );
    }
    if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(job) };
        bail!("AssignProcessToJobObject failed: {error}");
    }

    // `std::process::Child` does not retain the primary thread handle. Because
    // CREATE_SUSPENDED prevents any child code from running, the snapshot has
    // exactly the suspended primary thread for this PID at this point.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        terminate_and_close_job(job);
        bail!("CreateToolhelp32Snapshot failed: {error}");
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == child.id() {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(snapshot);
                }
                terminate_and_close_job(job);
                bail!("OpenThread failed: {error}");
            }
            let resumed = unsafe { ResumeThread(thread) };
            unsafe { CloseHandle(thread) };
            if resumed == u32::MAX {
                let error = std::io::Error::last_os_error();
                unsafe {
                    CloseHandle(snapshot);
                }
                terminate_and_close_job(job);
                bail!("ResumeThread failed: {error}");
            }
            found = true;
            break;
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if !found {
        terminate_and_close_job(job);
        bail!("suspended child primary thread was not found");
    }
    Ok(job)
}

#[cfg(windows)]
fn terminate_and_close_job(job: windows_sys::Win32::Foundation::HANDLE) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    unsafe {
        let _ = TerminateJobObject(job, 1);
        CloseHandle(job);
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

fn delta_bytes(frame: &Value, thread_id: &str, turn_id: &str) -> Option<u64> {
    let method = frame.get("method").and_then(Value::as_str)?;
    if !matches!(
        method,
        "item/agentMessage/delta" | "item/commandExecution/outputDelta"
    ) || frame.pointer("/params/threadId").and_then(Value::as_str) != Some(thread_id)
        || frame.pointer("/params/turnId").and_then(Value::as_str) != Some(turn_id)
    {
        return None;
    }
    frame
        .pointer("/params/delta")
        .and_then(Value::as_str)
        .filter(|delta| !delta.is_empty())
        .map(|delta| delta.len() as u64)
}

fn terminal_status<'a>(frame: &'a Value, thread_id: &str, turn_id: &str) -> Option<&'a str> {
    (frame.get("method").and_then(Value::as_str) == Some("turn/completed")
        && frame.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
        && frame.pointer("/params/turn/id").and_then(Value::as_str) == Some(turn_id))
    .then(|| frame.pointer("/params/turn/status").and_then(Value::as_str))
    .flatten()
}

fn fail_on_error_notification(frame: &Value) -> Result<()> {
    if frame.get("method").and_then(Value::as_str) == Some("error") {
        bail!(
            "server error notification: {}",
            sanitized_error(frame.get("params").unwrap_or(&Value::Null))
        );
    }
    Ok(())
}

fn sanitized_error(value: &Value) -> String {
    value
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/error/message").and_then(Value::as_str))
        .unwrap_or("unspecified server error")
        .chars()
        .take(240)
        .collect()
}

fn validate_initialize(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .context("initialize malformed success: result must be an object")?;
    let nonempty_string = |key: &str| {
        result
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    if !nonempty_string("userAgent")
        || !nonempty_string("platformFamily")
        || !nonempty_string("platformOs")
        || !result
            .get("codexHome")
            .and_then(Value::as_str)
            .is_some_and(|path| Path::new(path).is_absolute())
    {
        bail!("initialize malformed success: missing pinned 0.139 structural fields");
    }
    Ok(())
}

fn validate_account(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .context("account/read malformed success: result must be an object")?;
    let requires_auth = result
        .get("requiresOpenaiAuth")
        .and_then(Value::as_bool)
        .context("account/read malformed success: requiresOpenaiAuth must be boolean")?;
    match result.get("account") {
        None | Some(Value::Null) if requires_auth => {
            bail!("authentication required: account/read returned no account")
        }
        None => bail!("account/read malformed success: account field is missing"),
        Some(Value::Null) => Ok(()),
        Some(Value::Object(account)) => match account.get("type").and_then(Value::as_str) {
            Some("apiKey") | Some("amazonBedrock") => Ok(()),
            Some("chatgpt")
                if account.get("email").is_some_and(Value::is_string)
                    && account.get("planType").is_some_and(Value::is_string) =>
            {
                Ok(())
            }
            _ => bail!("account/read malformed success: invalid account structure"),
        },
        Some(_) => bail!("account/read malformed success: account must be object or null"),
    }
}

fn validate_rate_limits(frame: &Value) -> Result<()> {
    let result = frame
        .get("result")
        .and_then(Value::as_object)
        .context("account/rateLimits/read malformed success: result must be an object")?;
    let snapshot = result
        .get("rateLimits")
        .and_then(Value::as_object)
        .context("account/rateLimits/read malformed success: rateLimits must be an object")?;
    for window_name in ["primary", "secondary"] {
        if let Some(value) = snapshot.get(window_name).filter(|value| !value.is_null()) {
            let window = value.as_object().with_context(|| {
                format!(
                    "account/rateLimits/read malformed success: {window_name} must be an object"
                )
            })?;
            if !window.get("usedPercent").is_some_and(Value::is_number) {
                bail!(
                    "account/rateLimits/read malformed success: {window_name} usedPercent must be numeric"
                );
            }
        }
    }
    if snapshot
        .get("rateLimitReachedType")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        bail!(
            "account/rateLimits/read malformed success: rateLimitReachedType must be string or null"
        );
    }
    Ok(())
}

fn quota_exhausted(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            (key == "rateLimitReachedType" && !value.is_null())
                || (key == "usedPercent" && value.as_f64().is_some_and(|used| used >= 100.0))
                || quota_exhausted(value)
        }),
        Value::Array(values) => values.iter().any(quota_exhausted),
        _ => false,
    }
}

const CORPUS_BYTES: usize = 1024;
const A_CHUNKS: usize = 36;
const A_CADENCE_MS: usize = 1_000;
const B_CHUNKS: usize = 56;
const B_CADENCE_MS: usize = 50;
const D_CHUNK_BYTES: usize = 256;
const D_CHUNKS: usize = 64;
const D_CADENCE_MS: usize = 250;

#[derive(Debug)]
struct ScenarioStimulus {
    prompt: String,
    command: String,
    chunk_bytes: usize,
    chunks: usize,
    cadence_ms: usize,
    target_bytes: usize,
}

fn restricted_ascii_corpus() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 .,:-_";
    let mut state = 0x5a17_2d39_u32;
    let mut bytes = Vec::with_capacity(CORPUS_BYTES);
    for index in 0..CORPUS_BYTES {
        if index % 64 == 63 {
            bytes.push(b'\n');
        } else {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            bytes.push(ALPHABET[(state as usize) % ALPHABET.len()]);
        }
    }
    String::from_utf8(bytes).expect("restricted ASCII corpus must be UTF-8")
}

fn streaming_stimulus(name: &str, corpus: &str, shell: &Path) -> Result<ScenarioStimulus> {
    let (chunk, chunks, cadence_ms) = match name {
        "A" => (corpus, A_CHUNKS, A_CADENCE_MS),
        "B" => (corpus, B_CHUNKS, B_CADENCE_MS),
        "D" => (&corpus[..D_CHUNK_BYTES], D_CHUNKS, D_CADENCE_MS),
        _ => bail!("unknown streaming scenario {name}"),
    };
    let command = streaming_command(shell, chunk, chunks, cadence_ms)?;
    let prompt = format!(
        "S2-{name}: Execute exactly this command once now. Use no other tool or command. Emit no prose before completion.\nCOMMAND_BEGIN\n{command}\nCOMMAND_END"
    );
    Ok(ScenarioStimulus {
        prompt,
        command,
        chunk_bytes: chunk.len(),
        chunks,
        cadence_ms,
        target_bytes: chunk.len() * chunks,
    })
}

fn scenario_prompt(
    name: &str,
    approval_command: &str,
    corpus: &str,
    shell: &Path,
) -> Result<String> {
    match name {
        "A" | "B" | "D" => {
            let stimulus = streaming_stimulus(name, corpus, shell)?;
            debug_assert!(stimulus.target_bytes >= if name == "D" { 16 * 1024 } else { 36 * 1024 });
            debug_assert_eq!(
                stimulus.target_bytes,
                stimulus.chunk_bytes * stimulus.chunks
            );
            debug_assert!(stimulus.cadence_ms > 0);
            debug_assert!(stimulus.prompt.contains(&stimulus.command));
            debug_assert!(name != "D" || stimulus.chunk_bytes * 8 >= 2 * 1024);
            Ok(stimulus.prompt)
        }
        "C" => Ok(format!(
            "S2-C: Execute exactly this command once now. Emit no prose. Use no other command or tool. APPROVAL_COMMAND_JSON:{}",
            serde_json::to_string(approval_command).unwrap()
        )),
        _ => unreachable!(),
    }
}

fn streaming_command(
    shell: &Path,
    chunk: &str,
    chunks: usize,
    cadence_ms: usize,
) -> Result<String> {
    let shell = shell
        .to_str()
        .context("trusted scenario shell path was not valid Unicode")?;
    if shell.contains(['\r', '\n', '"']) {
        bail!("trusted scenario shell path contained unsafe characters");
    }
    #[cfg(windows)]
    {
        let script = powershell_streaming_script(chunk, chunks, cadence_ms);
        let encoded = encode_powershell_command(&script);
        return Ok(format!(
            "& \"{shell}\" -NoProfile -NonInteractive -EncodedCommand {encoded}"
        ));
    }
    #[cfg(not(windows))]
    {
        let encoded = chunk
            .as_bytes()
            .iter()
            .map(|byte| format!(r"\{byte:03o}"))
            .collect::<String>();
        let cadence_seconds = cadence_ms as f64 / 1_000.0;
        Ok(format!(
            "/bin/sh -c 'i=0; while [ \"$i\" -lt {chunks} ]; do printf \"%b\" \"{encoded}\"; sleep {cadence_seconds:.3}; i=$((i + 1)); done'"
        ))
    }
}

#[cfg(windows)]
fn powershell_streaming_script(chunk: &str, chunks: usize, cadence_ms: usize) -> String {
    let encoded = chunk
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    format!(
        "$h='{encoded}';$c=New-Object byte[] ($h.Length/2);for($j=0;$j -lt $c.Length;$j++){{$c[$j]=[Convert]::ToByte($h.Substring($j*2,2),16)}};$o=[Console]::OpenStandardOutput();for($i=0;$i -lt {chunks};$i++){{$o.Write($c,0,$c.Length);$o.Flush();Start-Sleep -Milliseconds {cadence_ms}}}"
    )
}

#[cfg(windows)]
fn encode_powershell_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

#[cfg(windows)]
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        encoded.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(value & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

fn approval_command() -> Result<String> {
    #[cfg(windows)]
    {
        let path = trusted_system_cmd()?;
        let path = path
            .to_str()
            .context("system cmd.exe path was not valid Unicode")?;
        if path.contains('"') {
            bail!("system cmd.exe path contained an unsafe quote");
        }
        return Ok(format!(
            "& \"{path}\" /d /s /c \"<nul set /p ={APPROVAL_MARKER_CONTENT}>{APPROVAL_MARKER_NAME}&exit /b 0\""
        ));
    }
    #[cfg(not(windows))]
    Ok(format!(
        "/bin/sh -c 'printf {APPROVAL_MARKER_CONTENT} > {APPROVAL_MARKER_NAME}'"
    ))
}

fn approval_marker_path(workspace: &Path) -> PathBuf {
    workspace.join(APPROVAL_MARKER_NAME)
}

fn run_marker_io_worker<T>(
    deadline: Instant,
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T>
where
    T: Send + 'static,
{
    let remaining = remaining_until(deadline)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(operation());
    });
    match receiver.recv_timeout(remaining) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            bail!("scenario C approval marker I/O timeout")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            bail!("scenario C approval marker I/O worker failed")
        }
    }
}

fn ensure_approval_marker_absent_bounded(workspace: &Path, deadline: Instant) -> Result<()> {
    let path = approval_marker_path(workspace);
    run_marker_io_worker(deadline, move || {
        ensure_approval_marker_absent_atomically(&path)
    })
}

fn verify_approval_marker_bounded(workspace: &Path, deadline: Instant) -> Result<()> {
    let path = approval_marker_path(workspace);
    let bytes = run_marker_io_worker(deadline, move || read_approval_marker_atomically(&path))?;
    if bytes != APPROVAL_MARKER_BYTES {
        bail!("scenario C approval marker verification failed");
    }
    Ok(())
}

#[cfg(windows)]
struct OwnedMarkerHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedMarkerHandle {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(windows)]
fn open_approval_marker_nofollow(path: &Path) -> std::io::Result<OwnedMarkerHandle> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    Ok(OwnedMarkerHandle(handle))
}

#[cfg(windows)]
fn ensure_approval_marker_absent_atomically(path: &Path) -> Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};

    match open_approval_marker_nofollow(path) {
        Ok(_) => bail!("scenario C approval marker precondition failed"),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code) if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            Ok(())
        }
        Err(_) => bail!("scenario C approval marker precondition failed"),
    }
}

#[cfg(unix)]
fn open_approval_marker_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(unix)]
fn ensure_approval_marker_absent_atomically(path: &Path) -> Result<()> {
    match open_approval_marker_nofollow(path) {
        Ok(_) => bail!("scenario C approval marker precondition failed"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => bail!("scenario C approval marker precondition failed"),
    }
}

#[cfg(windows)]
fn read_approval_marker_atomically(path: &Path) -> Result<Vec<u8>> {
    read_approval_marker_with_post_open(path, || {})
}

#[cfg(windows)]
fn read_approval_marker_with_post_open(path: &Path, post_open: impl FnOnce()) -> Result<Vec<u8>> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_TYPE_DISK, GetFileInformationByHandle, GetFileType, ReadFile,
    };

    let handle = open_approval_marker_nofollow(path)
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    post_open();
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0
        || unsafe { GetFileType(handle.0) } != FILE_TYPE_DISK
        || info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
    {
        bail!("scenario C approval marker verification failed");
    }
    let size = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
    if size > APPROVAL_MARKER_MAX_BYTES || size != APPROVAL_MARKER_BYTES.len() as u64 {
        bail!("scenario C approval marker verification failed");
    }
    let mut bytes = vec![0_u8; (APPROVAL_MARKER_MAX_BYTES + 1) as usize];
    let mut total = 0_usize;
    while total < bytes.len() {
        let mut read = 0_u32;
        if unsafe {
            ReadFile(
                handle.0,
                bytes[total..].as_mut_ptr(),
                (bytes.len() - total) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            bail!("scenario C approval marker verification failed");
        }
        if read == 0 {
            break;
        }
        total += read as usize;
    }
    bytes.truncate(total);
    Ok(bytes)
}

#[cfg(unix)]
fn read_approval_marker_atomically(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;

    let file = open_approval_marker_nofollow(path)
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    if !metadata.file_type().is_file()
        || metadata.len() > APPROVAL_MARKER_MAX_BYTES
        || metadata.len() != APPROVAL_MARKER_BYTES.len() as u64
    {
        bail!("scenario C approval marker verification failed");
    }
    let mut bytes = Vec::with_capacity(APPROVAL_MARKER_BYTES.len());
    file.take(APPROVAL_MARKER_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("scenario C approval marker verification failed"))?;
    Ok(bytes)
}

fn trusted_scenario_shell() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let system_root = canonical_safe_windows_root(system_directory()?, "system directory")?;
        let candidate = system_root.join(r"WindowsPowerShell\v1.0\powershell.exe");
        let canonical = canonical_safe_windows_executable(
            &candidate,
            "powershell.exe",
            "system Windows PowerShell",
        )?;
        if !canonical.starts_with(&system_root)
            || !canonical
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("powershell.exe"))
        {
            bail!("system Windows PowerShell escaped the protected system directory");
        }
        return Ok(canonical);
    }
    #[cfg(not(windows))]
    {
        let shell = PathBuf::from("/bin/sh");
        if !shell.is_file() {
            bail!("trusted /bin/sh was not a regular file");
        }
        Ok(shell)
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileIdentity {
    volume_serial: u32,
    file_index: u64,
}

#[cfg(windows)]
struct TrustedApprovalWrapper {
    path: PathBuf,
    handle: windows_sys::Win32::Foundation::HANDLE,
    identity: WindowsFileIdentity,
}

#[cfg(not(windows))]
struct TrustedApprovalWrapper;

#[cfg(windows)]
struct OwnedWrapperHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedWrapperHandle {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(windows)]
impl OwnedWrapperHandle {
    fn into_raw(self) -> windows_sys::Win32::Foundation::HANDLE {
        let handle = self.0;
        std::mem::forget(self);
        handle
    }
}

#[cfg(windows)]
impl Drop for TrustedApprovalWrapper {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe { CloseHandle(self.handle) };
    }
}

impl TrustedApprovalWrapper {
    #[cfg(windows)]
    fn path_identity_matches(&self) -> bool {
        let Ok(handle) = open_locked_wrapper_handle(&self.path) else {
            return false;
        };
        let Ok(final_path) = final_path_from_wrapper_handle(handle.0) else {
            return false;
        };
        if final_path != self.path {
            return false;
        }
        wrapper_file_identity(handle.0).is_ok_and(|identity| identity == self.identity)
    }

    #[cfg(not(windows))]
    fn path_identity_matches(&self) -> bool {
        false
    }
}

fn validate_explicit_approval_wrapper(
    path: Option<&Path>,
) -> Result<Option<TrustedApprovalWrapper>> {
    let Some(path) = path else {
        return Ok(None);
    };
    #[cfg(windows)]
    {
        if !path.is_absolute() {
            bail!("trusted approval wrapper validation failed");
        }
        return acquire_explicit_approval_wrapper_with_post_open(path, || {})
            .map(Some)
            .map_err(|_| anyhow!("trusted approval wrapper validation failed"));
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        bail!("trusted approval wrapper validation failed");
    }
}

#[cfg(windows)]
fn acquire_explicit_approval_wrapper_with_post_open(
    path: &Path,
    post_open: impl FnOnce(),
) -> Result<TrustedApprovalWrapper> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_TYPE_DISK, GetFileType,
    };

    if !path.is_absolute() {
        bail!("trusted approval wrapper validation failed");
    }
    let handle = open_locked_wrapper_handle(path)?;
    post_open();
    let final_path = final_path_from_wrapper_handle(handle.0)?;
    let info = wrapper_file_information(handle.0)?;
    let safe = final_path.is_absolute()
        && unsafe { GetFileType(handle.0) } == FILE_TYPE_DISK
        && info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        && final_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("pwsh.exe"))
        && final_path.to_str().is_some_and(|value| {
            !value.chars().any(|character| {
                matches!(
                    character,
                    '\r' | '\n' | '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!' | '(' | ')' | ';'
                )
            })
        });
    if !safe {
        bail!("trusted approval wrapper validation failed");
    }
    let identity = WindowsFileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    };
    Ok(TrustedApprovalWrapper {
        path: final_path,
        handle: handle.into_raw(),
        identity,
    })
}

#[cfg(windows)]
fn open_locked_wrapper_handle(path: &Path) -> Result<OwnedWrapperHandle> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        bail!("trusted approval wrapper validation failed");
    }
    Ok(OwnedWrapperHandle(handle))
}

#[cfg(windows)]
fn final_path_from_wrapper_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_DOS,
        )
    };
    if length == 0 || length as usize >= buffer.len() {
        bail!("trusted approval wrapper validation failed");
    }
    normalize_windows_command_path(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..length as usize],
    )))
}

#[cfg(windows)]
fn wrapper_file_information(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        bail!("trusted approval wrapper validation failed");
    }
    Ok(info)
}

#[cfg(windows)]
fn wrapper_file_identity(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<WindowsFileIdentity> {
    let info = wrapper_file_information(handle)?;
    Ok(WindowsFileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}

#[cfg(windows)]
fn auto_trusted_approval_wrapper_shell() -> Result<Option<PathBuf>> {
    let candidate = match find_windows_path_command("pwsh.exe") {
        Ok(candidate) => candidate,
        Err(_) => return Ok(None),
    };
    let canonical =
        canonical_safe_windows_executable(&candidate, "pwsh.exe", "app-server PowerShell")?;
    let program_files = canonical_safe_windows_root(known_program_files()?, "Program Files")?;
    let powershell_root = match program_files.join("PowerShell").canonicalize() {
        Ok(root) => normalize_windows_command_path(root)?,
        Err(_) => return Ok(None),
    };
    if !powershell_root.starts_with(&program_files) || !canonical.starts_with(&powershell_root) {
        return Ok(None);
    }
    Ok(Some(canonical))
}

#[cfg(windows)]
fn canonical_safe_windows_root(path: PathBuf, label: &str) -> Result<PathBuf> {
    let canonical = normalize_windows_command_path(
        path.canonicalize()
            .with_context(|| format!("failed to canonicalize {label} {}", path.display()))?,
    )?;
    if !canonical.is_absolute() || !canonical.is_dir() {
        bail!("{label} was not a canonical absolute directory");
    }
    validate_safe_windows_path(&canonical, label)?;
    Ok(canonical)
}

#[cfg(windows)]
fn canonical_safe_windows_executable(
    path: &Path,
    expected_name: &str,
    label: &str,
) -> Result<PathBuf> {
    let canonical = normalize_windows_command_path(
        path.canonicalize()
            .with_context(|| format!("failed to canonicalize {label} {}", path.display()))?,
    )?;
    if !canonical.is_absolute()
        || !canonical.is_file()
        || !canonical
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(expected_name))
    {
        bail!("{label} was not the expected canonical absolute regular file");
    }
    validate_safe_windows_path(&canonical, label)?;
    Ok(canonical)
}

#[cfg(windows)]
fn validate_safe_windows_path(path: &Path, label: &str) -> Result<()> {
    let rendered = path
        .to_str()
        .with_context(|| format!("{label} path was not valid Unicode"))?;
    if rendered
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '"'))
    {
        bail!("{label} path contained unsafe characters");
    }
    Ok(())
}

#[cfg(windows)]
fn known_program_files() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};

    let mut path = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, std::ptr::null_mut(), &mut path) };
    if result < 0 || path.is_null() {
        unsafe { CoTaskMemFree(path.cast()) };
        bail!("SHGetKnownFolderPath(FOLDERID_ProgramFiles) failed: HRESULT {result:#x}");
    }
    let length = unsafe {
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        length
    };
    let value = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(path, length)
    }));
    unsafe { CoTaskMemFree(path.cast()) };
    Ok(value)
}

#[cfg(windows)]
fn system_directory() -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0_u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        bail!("GetSystemDirectoryW failed or returned an oversized path");
    }
    Ok(PathBuf::from(OsString::from_wide(
        &buffer[..length as usize],
    )))
}

fn approval_wrapper_candidate(pwsh: &Path, expected: &str) -> Result<String> {
    let pwsh = pwsh
        .to_str()
        .context("trusted pwsh.exe path was not valid Unicode")?;
    if pwsh
        .chars()
        .any(|character| matches!(character, '\r' | '\n' | '"'))
    {
        bail!("trusted pwsh.exe path contained unsafe characters");
    }
    let escaped = expected.replace('\\', r"\\").replace('"', r#"\""#);
    Ok(format!(r#""{pwsh}" -Command "{escaped}""#))
}

#[cfg(windows)]
fn trusted_system_cmd() -> Result<PathBuf> {
    let system_root = canonical_safe_windows_root(system_directory()?, "system directory")?;
    let path = canonical_safe_windows_executable(
        &system_root.join("cmd.exe"),
        "cmd.exe",
        "system cmd.exe",
    )?;
    if !path.starts_with(&system_root) {
        bail!("system cmd.exe escaped the protected system directory");
    }
    Ok(path)
}

fn performance(events: &[ContentEvent]) -> Result<PerformanceEvidence> {
    if events.is_empty() {
        bail!("no real content events were observed");
    }
    let mut sizes = events.iter().map(|event| event.bytes).collect::<Vec<_>>();
    sizes.sort_unstable();
    let percentile = |numerator: usize| sizes[((sizes.len() - 1) * numerator) / 100];
    let mut ordered = events.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|event| event.timestamp_ns);
    let mut left = 0;
    let mut window_bytes = 0_u64;
    let mut peak_events = 0_u64;
    let mut peak_bytes = 0_u64;
    for (right, event) in ordered.iter().enumerate() {
        window_bytes += event.bytes;
        while event
            .timestamp_ns
            .saturating_sub(ordered[left].timestamp_ns)
            >= 1_000_000_000
        {
            window_bytes -= ordered[left].bytes;
            left += 1;
        }
        peak_events = peak_events.max((right - left + 1) as u64);
        peak_bytes = peak_bytes.max(window_bytes);
    }
    let mut merge_windows = std::collections::BTreeSet::new();
    let first_timestamp = ordered[0].timestamp_ns;
    for event in ordered {
        merge_windows.insert(event.timestamp_ns.saturating_sub(first_timestamp) / 50_000_000);
    }
    Ok(PerformanceEvidence {
        real_content: true,
        peak_events_per_second: peak_events as f64,
        peak_megabytes_per_second: peak_bytes as f64 / 1_000_000.0,
        event_sizes: EventSizeDistribution {
            samples: sizes.len() as u64,
            min_bytes: sizes[0],
            p50_bytes: percentile(50),
            p95_bytes: percentile(95),
            max_bytes: *sizes.last().unwrap(),
        },
        merge_window_ms: 50,
        merge_input_events: events.len() as u64,
        merge_output_events: merge_windows.len() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::{ContentEvent, performance};
    use std::io::{self, BufReader, Read};
    use std::path::Path;

    use serde_json::json;

    #[cfg(windows)]
    fn decode_base64(input: &str) -> Vec<u8> {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0_u32;
        let mut bit_count = 0_u8;
        let mut output = Vec::new();
        for byte in input.bytes().take_while(|byte| *byte != b'=') {
            let value = alphabet.iter().position(|item| *item == byte).unwrap() as u32;
            bits = (bits << 6) | value;
            bit_count += 6;
            if bit_count >= 8 {
                bit_count -= 8;
                output.push((bits >> bit_count) as u8);
                bits &= (1 << bit_count) - 1;
            }
        }
        output
    }

    #[test]
    fn performance_uses_sliding_one_second_peak() {
        let metrics = performance(&[
            ContentEvent {
                timestamp_ns: 990_000_000,
                bytes: 7,
            },
            ContentEvent {
                timestamp_ns: 1_010_000_000,
                bytes: 11,
            },
        ])
        .unwrap();

        assert_eq!(metrics.peak_events_per_second, 2.0);
        assert_eq!(metrics.peak_megabytes_per_second, 18.0 / 1_000_000.0);
    }

    #[test]
    fn merge_windows_are_relative_to_first_event() {
        let metrics = performance(&[
            ContentEvent {
                timestamp_ns: 49_000_000,
                bytes: 7,
            },
            ContentEvent {
                timestamp_ns: 51_000_000,
                bytes: 11,
            },
        ])
        .unwrap();

        assert_eq!(metrics.merge_input_events, 2);
        assert_eq!(metrics.merge_output_events, 1);
    }

    #[test]
    fn stderr_line_reader_propagates_io_errors() {
        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::Other, "injected failure"))
            }
        }

        let error = super::read_line_checked(&mut BufReader::new(BrokenReader)).unwrap_err();
        assert_eq!(error.to_string(), "injected failure");
    }

    #[test]
    fn scenario_corpus_is_deterministic_restricted_ascii_near_one_kibibyte() {
        let first = super::restricted_ascii_corpus();
        let second = super::restricted_ascii_corpus();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1024);
        assert!(
            first
                .bytes()
                .all(|byte| byte == b'\n' || (0x20..=0x7e).contains(&byte))
        );
    }

    #[test]
    fn command_output_delta_is_r1_only_for_the_exact_thread_and_turn() {
        let command_output = json!({
            "method":"item/commandExecution/outputDelta",
            "params":{"threadId":"thread-a","turnId":"turn-a","delta":"real-output"}
        });
        assert_eq!(
            super::delta_bytes(&command_output, "thread-a", "turn-a"),
            Some(11)
        );
        assert_eq!(
            super::delta_bytes(&command_output, "other-thread", "turn-a"),
            None
        );
        assert_eq!(
            super::delta_bytes(&command_output, "thread-a", "other-turn"),
            None
        );
        let unrelated = json!({
            "method":"item/commandExecution/outputDelta",
            "params":{"threadId":"thread-a","turnId":"turn-a","output":"not-a-delta"}
        });
        assert_eq!(super::delta_bytes(&unrelated, "thread-a", "turn-a"), None);
    }

    #[test]
    fn deterministic_streaming_commands_have_safe_output_and_cadence_headroom() {
        let corpus = super::restricted_ascii_corpus();
        #[cfg(windows)]
        let shell = Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe");
        #[cfg(not(windows))]
        let shell = Path::new("/bin/sh");
        let a = super::streaming_stimulus("A", &corpus, shell).unwrap();
        let b = super::streaming_stimulus("B", &corpus, shell).unwrap();
        let d = super::streaming_stimulus("D", &corpus, shell).unwrap();

        assert_eq!(a.chunk_bytes, 1024);
        assert_eq!(a.chunks, 36);
        assert_eq!(a.cadence_ms, 1_000);
        assert!((a.chunks - 1) * a.cadence_ms >= 35_000);
        assert_eq!(b.chunk_bytes, 1024);
        assert!(b.chunks >= 56);
        assert_eq!(b.cadence_ms, 50);
        assert_eq!(d.chunk_bytes, 256);
        assert!(d.chunks >= 64);
        assert!(d.cadence_ms >= 200);
        assert!(d.chunk_bytes * 8 >= 2 * 1024);
        #[cfg(windows)]
        for stimulus in [&a, &b, &d] {
            assert!(stimulus.command.contains(" -EncodedCommand "));
            assert!(!stimulus.command.contains(r#"\""#));
            assert!(!stimulus.command.contains(" -Command "));
        }
        #[cfg(windows)]
        {
            let payload = a.command.split(" -EncodedCommand ").nth(1).unwrap();
            assert!(
                payload
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
            );
            let decoded = decode_base64(payload);
            assert_eq!(decoded.len() % 2, 0);
            let utf16 = decoded
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            let script = String::from_utf16(&utf16).unwrap();
            assert_eq!(
                script,
                super::powershell_streaming_script(&corpus, 36, 1_000)
            );
        }
        for stimulus in [a, b, d] {
            assert_eq!(
                stimulus.target_bytes,
                stimulus.chunk_bytes * stimulus.chunks
            );
            assert!(stimulus.prompt.contains(&stimulus.command));
            assert!(stimulus.prompt.contains("exactly this command once"));
            assert!(stimulus.prompt.contains("Use no other tool or command"));
            assert!(stimulus.prompt.contains("Emit no prose before completion"));
            assert!(!stimulus.command.contains("caller-controlled"));
        }
    }

    #[test]
    #[cfg(windows)]
    fn generated_windows_streaming_command_executes_through_outer_powershell() {
        let shell = super::trusted_scenario_shell().unwrap();
        let chunk = "S2xy";
        let chunks = 3;
        let command = super::streaming_command(&shell, chunk, chunks, 1).unwrap();
        let output = std::process::Command::new(&shell)
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(&command)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, chunk.repeat(chunks).as_bytes());
    }

    #[test]
    #[cfg(windows)]
    fn approval_inner_command_executes_only_in_the_exact_outer_powershell_cwd() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-approval-command-{}-{nonce}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let command = super::approval_command().unwrap();
        let shell = super::trusted_scenario_shell().unwrap();
        let output = std::process::Command::new(shell)
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(&command)
            .current_dir(&workspace)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let marker = workspace.join(super::APPROVAL_MARKER_NAME);
        assert!(marker.is_file());
        assert_eq!(std::fs::read(marker).unwrap(), super::APPROVAL_MARKER_BYTES);
        assert!(!root.join(".codex-s2-approval-marker").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn approval_wrapper_is_byte_exact_and_escapes_backslashes_before_quotes() {
        let expected = r#"& \"C:\Windows\System32\cmd.exe\" /d /s /c \"<nul set /p =S2_APPROVED>.codex-s2-approval-marker&exit /b 0\""#;
        let pwsh = Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe");
        let wrapped = super::approval_wrapper_candidate(pwsh, expected).unwrap();
        let escaped = expected.replace('\\', r"\\").replace('"', r#"\""#);
        assert_eq!(
            wrapped,
            format!(r#""{}" -Command "{}""#, pwsh.display(), escaped)
        );
        assert!(!wrapped.starts_with(r#"\""#));
        assert!(!wrapped.ends_with(r#"\""#));
    }

    #[test]
    #[cfg(windows)]
    fn explicit_wrapper_is_locked_before_final_path_resolution() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-atomic-wrapper-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wrapper_path = root.join("pwsh.exe");
        let displaced_path = root.join("displaced.exe");
        std::fs::write(&wrapper_path, b"trusted").unwrap();

        let wrapper =
            super::acquire_explicit_approval_wrapper_with_post_open(&wrapper_path, || {
                assert!(
                    std::fs::rename(&wrapper_path, &displaced_path).is_err(),
                    "the raw input path must already be locked against replacement"
                );
            })
            .unwrap();

        assert_eq!(
            wrapper.path,
            super::normalize_windows_command_path(wrapper_path.canonicalize().unwrap()).unwrap()
        );
        drop(wrapper);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn marker_io_worker_obeys_the_absolute_deadline() {
        let start = std::time::Instant::now();
        let result =
            super::run_marker_io_worker(start + std::time::Duration::from_millis(25), || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                Ok(())
            });
        assert!(result.is_err());
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }

    #[test]
    #[cfg(windows)]
    fn marker_reader_locks_the_opened_identity_before_observing_content() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-marker-identity-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join(super::APPROVAL_MARKER_NAME);
        let displaced = root.join("displaced-marker");
        std::fs::write(&marker, super::APPROVAL_MARKER_BYTES).unwrap();

        let bytes = super::read_approval_marker_with_post_open(&marker, || {
            assert!(
                std::fs::rename(&marker, &displaced).is_err(),
                "the opened marker must already exclude delete sharing"
            );
        })
        .unwrap();

        assert_eq!(bytes, super::APPROVAL_MARKER_BYTES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn marker_reader_rejects_a_static_symlink_when_supported() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-marker-symlink-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        let marker = root.join(super::APPROVAL_MARKER_NAME);
        std::fs::write(&target, super::APPROVAL_MARKER_BYTES).unwrap();
        if std::os::windows::fs::symlink_file(&target, &marker).is_ok() {
            assert!(super::read_approval_marker_atomically(&marker).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn marker_reader_rejects_symlinks_and_fifos_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codex-s2-marker-special-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        std::fs::write(&target, super::APPROVAL_MARKER_BYTES).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        assert!(super::read_approval_marker_atomically(&link).is_err());

        let fifo = root.join("fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(super::read_approval_marker_atomically(&fifo).is_err());
        assert!(started.elapsed() < std::time::Duration::from_millis(250));
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn empty_evidence() -> S2Evidence {
    S2Evidence {
        scenarios: ["A", "B", "C", "D"]
            .map(|name| ScenarioEvidence {
                name: name.to_owned(),
                terminal_state: TerminalState::Missing,
                turn_completed: false,
                first_delta_seen: false,
                r1_sufficient: false,
                approval_seen: false,
                interrupt_response_seen: false,
            })
            .to_vec(),
        auth_errors: 0,
        quota_errors: 0,
        protocol_errors: 0,
        performance: None,
        candidate_percentiles: None,
    }
}

fn classify_failure(error: &anyhow::Error, evidence: &mut S2Evidence) {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("malformed success") {
        evidence.protocol_errors = 1;
    } else if message.contains("auth") || message.contains("unauthorized") {
        evidence.auth_errors = 1;
    } else if message.contains("quota")
        || message.contains("usage limit")
        || message.contains("usagelimit")
        || message.contains("rate limit")
        || message.contains("ratelimit")
    {
        evidence.quota_errors = 1;
    } else {
        evidence.protocol_errors = 1;
    }
}

fn write_artifacts(
    output_dir: &Path,
    evidence: &S2Evidence,
    report: &S2Report,
    error: Option<&anyhow::Error>,
) -> Result<()> {
    std::fs::write(
        output_dir.join(EVIDENCE_FILE),
        format!("{}\n", serde_json::to_string_pretty(evidence)?),
    )?;
    std::fs::write(
        output_dir.join(REPORT_FILE),
        format!("{}\n", serde_json::to_string_pretty(report)?),
    )?;
    let status = if report.valid { "PASS" } else { "INVALID" };
    let detail = match error {
        Some(err) => sanitized_error_text(&format!("{err:#}")),
        None if report.valid => "all F1-F3 gates passed".to_owned(),
        None => report.reasons.join("; "),
    };
    let timing = evidence
        .candidate_percentiles
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metrics| {
            Some(format!(
                "interrupt_response_latency_ms={:.3} interrupt_terminal_latency_ms={:.3}",
                metrics.get("interrupt_response_latency_ms")?.as_f64()?,
                metrics.get("interrupt_terminal_latency_ms")?.as_f64()?
            ))
        })
        .unwrap_or_else(|| "interrupt_timings=unavailable".to_owned());
    std::fs::write(
        output_dir.join(SUMMARY_FILE),
        format!(
            "S2 {status}\nF1={} F2={} F3={}\n{timing}\n{detail}\n",
            report.f1.passed, report.f2.passed, report.f3.passed,
        ),
    )?;
    Ok(())
}

fn sanitized_error_text(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("usage limit") || lower.contains("quota") || lower.contains("rate limit") {
        "quota/rate-limit precondition failed".to_owned()
    } else if lower.contains("auth") || lower.contains("unauthorized") {
        "authentication precondition failed".to_owned()
    } else if lower.contains("timeout") {
        "bounded scenario/global timeout expired".to_owned()
    } else if lower.contains("approval") {
        "approval safety/protocol precondition failed".to_owned()
    } else {
        "protocol/scenario precondition failed".to_owned()
    }
}

fn default_output_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("codex-s2-{}-{nonce}", std::process::id()))
}
