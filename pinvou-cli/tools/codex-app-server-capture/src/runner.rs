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

use crate::capture::{CommandSpec, JsonlRecorder, create_capture_file};
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

#[derive(Clone, Debug)]
pub struct S2RunConfig {
    pub output_dir: Option<PathBuf>,
    pub executable: Option<OsString>,
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

    let mut evidence = empty_evidence();
    let execution = verify_executable_version(&config, config.global_timeout)
        .and_then(|_| execute(&config, &output_dir, &workspace, &mut evidence, thresholds));
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

fn verify_executable_version(config: &S2RunConfig, timeout: Duration) -> Result<()> {
    use std::io::Read;

    let program = config
        .executable
        .clone()
        .unwrap_or_else(|| OsString::from("codex"));
    let mut child = Command::new(&program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to launch version preflight for {program:?}"))?;
    let stdout = child.stdout.take().context("version stdout unavailable")?;
    let stderr = child.stderr.take().context("version stderr unavailable")?;
    let (output_tx, output_rx) = mpsc::sync_channel(2);
    for (label, stream) in [
        ("stdout", Box::new(stdout) as Box<dyn Read + Send>),
        ("stderr", Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = output_tx.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.take(4097).read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send((label, result));
        });
    }
    drop(output_tx);
    let deadline = Instant::now() + timeout.min(Duration::from_secs(5));
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context("failed polling version preflight")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("version preflight timeout");
        }
        thread::sleep(Duration::from_millis(5));
    };
    let mut stdout_bytes = None;
    let read_deadline = Instant::now() + Duration::from_secs(1);
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
    output_dir: &Path,
    workspace: &Path,
    evidence: &mut S2Evidence,
    thresholds: RunnerThresholds,
) -> Result<()> {
    let clock = Arc::new(HostMonotonicClock::new()?);
    let recorder_clock = Arc::clone(&clock);
    let recorder: Arc<Mutex<Recorder>> = Arc::new(Mutex::new(JsonlRecorder::new(
        BufWriter::new(create_capture_file(&output_dir.join(CAPTURE_FILE))?),
        Box::new(move || recorder_clock.now_ns().expect("monotonic clock failed")),
    )));
    let mut session = Session::spawn(
        CommandSpec::codex(config.executable.clone()),
        recorder,
        config.global_timeout,
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
        let approval_command = approval_command();
        for name in ["A", "B", "C", "D"] {
            let scenario_deadline = Instant::now() + config.scenario_timeout;
            let approval_policy = if name == "C" { "untrusted" } else { "never" };
            let mut thread_params = json!({
                "cwd": workspace,
                "approvalPolicy": approval_policy,
                "sandbox": "workspace-write",
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
            let prompt = scenario_prompt(name, &approval_command);
            let turn = session.request(
                "turn/start",
                json!({"threadId":thread_id,"cwd":workspace,"input":[{"type":"text","text":prompt}]}),
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
                approval_command,
                scenario_deadline,
                thresholds,
            )?;
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

struct Session {
    child: Child,
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
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    process_group: i32,
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
        command: CommandSpec,
        recorder: Arc<Mutex<Recorder>>,
        global_timeout: Duration,
    ) -> Result<Self> {
        let mut process = Command::new(&command.program);
        process
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        let mut child = process.spawn().with_context(|| {
            format!(
                "failed to launch app-server executable {:?}",
                command.program
            )
        })?;
        #[cfg(windows)]
        let job = assign_child_job(&mut child)?;
        #[cfg(unix)]
        let process_group = child.id() as i32;
        let stdin = child.stdin.take().context("app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server stdout unavailable")?;
        let stderr = child
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
            child,
            stdin,
            incoming,
            recorder,
            next_id: 1,
            pending: VecDeque::new(),
            global_deadline: Instant::now() + global_timeout,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            reader_done,
            inbound_overflow,
            #[cfg(windows)]
            job,
            #[cfg(unix)]
            process_group,
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
            if is_delta(&frame, thread_id, turn_id) {
                let bytes = frame
                    .pointer("/params/delta")
                    .and_then(Value::as_str)
                    .map(|delta| delta.len() as u64)
                    .unwrap_or(0);
                if bytes > 0 {
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
    ) -> Result<()> {
        let id = frame.get("id").cloned().context("approval missing id")?;
        let method = frame.get("method").and_then(Value::as_str);
        let params = frame.get("params").context("approval missing params")?;
        let cwd = params.get("cwd").and_then(Value::as_str).map(PathBuf::from);
        let workspace_root = workspace.canonicalize().ok();
        let canonical_cwd = cwd.as_deref().and_then(|path| path.canonicalize().ok());
        let safe = method == Some("item/commandExecution/requestApproval")
            && params.get("threadId").and_then(Value::as_str) == Some(thread_id)
            && params.get("turnId").and_then(Value::as_str) == Some(turn_id)
            && params.get("command").and_then(Value::as_str) == Some(command)
            && workspace_root.as_ref().is_some_and(|root| {
                canonical_cwd
                    .as_ref()
                    .is_some_and(|path| path == root || path.starts_with(root))
            });
        if !safe {
            self.send(&json!({"id":id,"result":{"decision":"cancel"}}))?;
            bail!(
                "unexpected approval rejected: method/command/cwd was outside the exact allowlist"
            );
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
        self.terminate_tree();
        let _ = self.child.wait();
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

    fn terminate_tree(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            let _ = TerminateJobObject(self.job, 1);
            CloseHandle(self.job);
            self.job = std::ptr::null_mut();
        }
        #[cfg(unix)]
        unsafe {
            let _ = kill(-self.process_group, 9);
        }
        let _ = self.child.kill();
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

#[cfg(windows)]
fn assign_child_job(child: &mut Child) -> Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    // SAFETY: null security/name create an unnamed job; child handle is live.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        let _ = child.kill();
        bail!(
            "CreateJobObjectW failed: {}",
            std::io::Error::last_os_error()
        );
    }
    if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(job) };
        let _ = child.kill();
        bail!("AssignProcessToJobObject failed: {error}");
    }
    Ok(job)
}

#[cfg(unix)]
unsafe extern "C" {
    fn setpgid(pid: i32, pgid: i32) -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

fn is_delta(frame: &Value, thread_id: &str, turn_id: &str) -> bool {
    frame.get("method").and_then(Value::as_str) == Some("item/agentMessage/delta")
        && frame.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
        && frame.pointer("/params/turnId").and_then(Value::as_str) == Some(turn_id)
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

fn scenario_prompt(name: &str, approval_command: &str) -> String {
    match name {
        "A" => "S2-A: Sustain a continuous plain-text technical explanation of streaming backpressure for at least 35 seconds and at least 2 KiB. Do not call tools.".to_owned(),
        "B" => "S2-B: Produce at least 40 KiB of dense generated content in one response for a real peak-output measurement. Do not call tools.".to_owned(),
        "C" => format!("S2-C: Execute exactly the following single benign local probe command, requesting approval before execution. Do not run any other command or tool. APPROVAL_COMMAND_JSON:{}", serde_json::to_string(approval_command).unwrap()),
        "D" => "S2-D: Begin producing at least 8000 words of dense generated content immediately and continuously. Do not call tools.".to_owned(),
        _ => unreachable!(),
    }
}

fn approval_command() -> &'static str {
    #[cfg(windows)]
    return "cmd.exe /d /c exit 0";
    #[cfg(not(windows))]
    return "/bin/true";
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
