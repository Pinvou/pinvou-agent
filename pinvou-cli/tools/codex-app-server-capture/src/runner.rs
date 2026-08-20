use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
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

pub fn run_s2(config: S2RunConfig) -> Result<S2RunOutcome> {
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
    let execution = execute(&config, &output_dir, &workspace, &mut evidence);
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

fn execute(
    config: &S2RunConfig,
    output_dir: &Path,
    workspace: &Path,
    evidence: &mut S2Evidence,
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
        clock,
        config.global_timeout,
    )?;

    let result = (|| {
        session.request(
            "initialize",
            json!({"clientInfo":{"name":"codex-s2-runner","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
            config.scenario_timeout,
        )?;
        session.notify("initialized", json!({}))?;
        let account = session.request(
            "account/read",
            json!({"refreshToken":false}),
            config.scenario_timeout,
        )?;
        if account.pointer("/result/requiresOpenaiAuth") == Some(&Value::Bool(true))
            && account
                .pointer("/result/account")
                .is_none_or(Value::is_null)
        {
            bail!("authentication required: account/read returned no account");
        }
        let limits = session.request(
            "account/rateLimits/read",
            json!({}),
            config.scenario_timeout,
        )?;
        if quota_exhausted(limits.get("result").unwrap_or(&Value::Null)) {
            bail!("quota exhausted: account rate limit is reached");
        }

        let mut all_content = Vec::new();
        let mut interrupt_control_latency_ms = None;
        let approval_command = create_approval_probe(workspace)?;
        for name in ["A", "B", "C", "D"] {
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
            let started =
                session.request("thread/start", thread_params, config.scenario_timeout)?;
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
                config.scenario_timeout,
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
                config.scenario_timeout,
            )?;
            if name == "A" || name == "B" {
                all_content.extend(observed.content.iter().cloned());
            }
            if name == "D" {
                interrupt_control_latency_ms = observed.interrupt_control_latency_ms;
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
        evidence.performance = Some(performance(&all_content)?);
        evidence.candidate_percentiles = Some(json!({
            "content_event_samples": all_content.len(),
            "merge_rate": evidence.performance.as_ref().map(|p| p.merge_output_events as f64 / p.merge_input_events as f64),
            "interrupt_control_latency_ms": interrupt_control_latency_ms
        }));
        Ok(())
    })();
    session.stop();
    result
}

struct ObservedScenario {
    evidence: ScenarioEvidence,
    content: Vec<ContentEvent>,
    interrupt_control_latency_ms: Option<f64>,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    incoming: mpsc::Receiver<Inbound>,
    recorder: Arc<Mutex<Recorder>>,
    clock: Arc<HostMonotonicClock>,
    next_id: u64,
    pending: VecDeque<(u64, Value)>,
    global_deadline: Instant,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
}

impl Session {
    fn spawn(
        command: CommandSpec,
        recorder: Arc<Mutex<Recorder>>,
        clock: Arc<HostMonotonicClock>,
        global_timeout: Duration,
    ) -> Result<Self> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch app-server executable {:?}",
                    command.program
                )
            })?;
        let stdin = child.stdin.take().context("app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("app-server stderr unavailable")?;
        let (tx, incoming) = mpsc::channel();
        let stdout_recorder = Arc::clone(&recorder);
        let stdout_clock = Arc::clone(&clock);
        let stdout_thread = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        let _ = tx.send(Inbound::Malformed(error.to_string()));
                        break;
                    }
                };
                let timestamp_ns = stdout_clock.now_ns().unwrap_or(0);
                if let Ok(mut guard) = stdout_recorder.lock() {
                    let _ = guard.record(CaptureChannel::ServerToClient, &line);
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) if value.is_object() => {
                        if tx
                            .send(Inbound::Frame {
                                timestamp_ns,
                                value,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(_) => {
                        let _ = tx.send(Inbound::Malformed(
                            "server frame was not an object".to_owned(),
                        ));
                        break;
                    }
                    Err(error) => {
                        let _ = tx.send(Inbound::Malformed(format!(
                            "malformed server JSON: {error}"
                        )));
                        break;
                    }
                }
            }
            let _ = tx.send(Inbound::Closed);
        });
        let stderr_recorder = Arc::clone(&recorder);
        let stderr_thread = thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut guard) = stderr_recorder.lock() {
                    let _ = guard.record(CaptureChannel::Stderr, &line);
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            incoming,
            recorder,
            clock,
            next_id: 1,
            pending: VecDeque::new(),
            global_deadline: Instant::now() + global_timeout,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
        })
    }

    fn send(&mut self, value: &Value) -> Result<()> {
        let line = serde_json::to_string(value)?;
        self.recorder
            .lock()
            .map_err(|_| anyhow!("capture recorder lock poisoned"))?
            .record(CaptureChannel::ClientToServer, &line)?;
        writeln!(self.stdin, "{line}").context("failed writing app-server stdin")?;
        self.stdin
            .flush()
            .context("failed flushing app-server stdin")?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"method":method,"params":params}))
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
                    self.pending.push_back((timestamp_ns, frame));
                    continue;
                }
                self.reject_server_request(&frame)?;
                bail!("unexpected server request while waiting for {method}");
            }
            fail_on_error_notification(&frame)?;
            if method == "turn/start" {
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
        timeout: Duration,
    ) -> Result<ObservedScenario> {
        let deadline = Instant::now() + timeout;
        let mut content = Vec::new();
        let mut approval_seen = false;
        let mut interrupt_response_seen = false;
        let mut interrupt_id = None;
        let mut interrupt_control_latency_ms = None;
        loop {
            let (timestamp_ns, frame) = self.recv(deadline)?;
            fail_on_error_notification(&frame)?;
            if frame.get("id").is_some() && frame.get("method").is_some() {
                if name != "C" || approval_seen {
                    self.reject_server_request(&frame)?;
                    bail!("scenario {name}: unexpected or duplicate approval request");
                }
                self.approve_exact_command(&frame, workspace, approval_command)?;
                approval_seen = true;
                continue;
            }
            if let Some(expected) = interrupt_id {
                if frame.get("id") == Some(&json!(expected)) && frame.get("method").is_none() {
                    if frame.get("error").is_some() || frame.get("result").is_none() {
                        bail!("scenario D: malformed interrupt response");
                    }
                    interrupt_response_seen = true;
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
                    if name == "D" && interrupt_id.is_none() {
                        let id = self.next_id;
                        self.next_id += 1;
                        self.send(&json!({"id":id,"method":"turn/interrupt","params":{"threadId":thread_id,"turnId":turn_id}}))?;
                        let sent_ns = self.clock.now_ns()?;
                        interrupt_control_latency_ms =
                            Some(sent_ns.saturating_sub(timestamp_ns) as f64 / 1_000_000.0);
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
                let bytes: u64 = content.iter().map(|event| event.bytes).sum();
                return Ok(ObservedScenario {
                    evidence: ScenarioEvidence {
                        name: name.to_owned(),
                        turn_completed: terminal_state == TerminalState::Completed,
                        terminal_state,
                        first_delta_seen: !content.is_empty(),
                        r1_sufficient: content.len() >= 2 && bytes >= 64,
                        approval_seen,
                        interrupt_response_seen,
                    },
                    content,
                    interrupt_control_latency_ms,
                });
            }
        }
    }

    fn approve_exact_command(
        &mut self,
        frame: &Value,
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
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

fn quota_exhausted(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            (key == "rateLimitReachedType" && !value.is_null())
                || (key == "usedPercent" && value.as_u64().is_some_and(|used| used >= 100))
                || quota_exhausted(value)
        }),
        Value::Array(values) => values.iter().any(quota_exhausted),
        _ => false,
    }
}

fn scenario_prompt(name: &str, approval_command: &str) -> String {
    match name {
        "A" => "S2-A: Produce a long, continuous plain-text technical explanation of streaming backpressure, at least 1200 words. Do not call tools.".to_owned(),
        "B" => "S2-B: Produce at least 4000 words of dense generated content in one response for a real peak-output measurement. Do not call tools.".to_owned(),
        "C" => format!("S2-C: Execute exactly the following single benign local probe command, requesting approval before execution. Do not run any other command or tool. APPROVAL_COMMAND_JSON:{}", serde_json::to_string(approval_command).unwrap()),
        "D" => "S2-D: Begin producing at least 8000 words of dense generated content immediately and continuously. Do not call tools.".to_owned(),
        _ => unreachable!(),
    }
}

fn create_approval_probe(workspace: &Path) -> Result<String> {
    #[cfg(windows)]
    let (path, contents) = (
        workspace.join("s2-benign-probe.cmd"),
        "@echo off\r\nexit /b 0\r\n",
    );
    #[cfg(not(windows))]
    let (path, contents) = (workspace.join("s2-benign-probe.sh"), "#!/bin/sh\nexit 0\n");
    std::fs::write(&path, contents)
        .with_context(|| format!("failed writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

fn performance(events: &[ContentEvent]) -> Result<PerformanceEvidence> {
    if events.is_empty() {
        bail!("no real content events were observed");
    }
    let mut sizes = events.iter().map(|event| event.bytes).collect::<Vec<_>>();
    sizes.sort_unstable();
    let percentile = |numerator: usize| sizes[((sizes.len() - 1) * numerator) / 100];
    let mut buckets = std::collections::BTreeMap::<u64, (u64, u64)>::new();
    for event in events {
        let bucket = event.timestamp_ns / 1_000_000_000;
        let entry = buckets.entry(bucket).or_default();
        entry.0 += 1;
        entry.1 += event.bytes;
    }
    let peak_events = buckets.values().map(|item| item.0).max().unwrap_or(0);
    let peak_bytes = buckets.values().map(|item| item.1).max().unwrap_or(0);
    let mut merge_windows = std::collections::BTreeSet::new();
    for event in events {
        merge_windows.insert(event.timestamp_ns / 50_000_000);
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
    if message.contains("auth") || message.contains("unauthorized") {
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
    std::fs::write(
        output_dir.join(SUMMARY_FILE),
        format!(
            "S2 {status}\nF1={} F2={} F3={}\n{detail}\n",
            report.f1.passed, report.f2.passed, report.f3.passed
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
