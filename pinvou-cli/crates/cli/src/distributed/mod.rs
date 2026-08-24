//! Feature-gated terminal client for the stage-one Controller IPC contract.

pub mod tui_backend;

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use pinvou_controller::{ControllerPaths, DetachedLaunch, HostPlatform, LocalEndpoint};
use pinvou_protocol::{
    ExitCause, HelloClient, HelloServer, IpcMessage, IpcMessageKind, RuntimeEventEnvelope,
    RuntimeEventKind, StableExitCode, encode_frame, read_frame,
};
use serde_json::{Value, json};

use crate::{CliError, OutputMode};

pub const PROTOCOL_CRATE_NAME: &str = pinvou_protocol::CRATE_NAME;
pub const CHAT_TEXT_FRAME: Duration = Duration::from_millis(50);
const READY_TIMEOUT: Duration = Duration::from_secs(3);
const READY_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DistributedCommand {
    Chat { runtime: Option<String> },
    RuntimeDetect { runtime: Option<String> },
    RuntimeList,
    RuntimeSwitch(String),
}

pub(crate) fn parse_command(values: &[String]) -> Result<Option<DistributedCommand>, CliError> {
    match values
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["chat"] => Ok(Some(DistributedCommand::Chat { runtime: None })),
        ["chat", "--runtime", runtime] if !runtime.is_empty() => {
            Ok(Some(DistributedCommand::Chat {
                runtime: Some((*runtime).into()),
            }))
        }
        ["runtime", "detect"] => Ok(Some(DistributedCommand::RuntimeDetect { runtime: None })),
        ["runtime", "detect", runtime] if !runtime.is_empty() => {
            Ok(Some(DistributedCommand::RuntimeDetect {
                runtime: Some((*runtime).into()),
            }))
        }
        ["runtime", "list"] => Ok(Some(DistributedCommand::RuntimeList)),
        ["runtime", "switch", runtime] if !runtime.is_empty() => {
            Ok(Some(DistributedCommand::RuntimeSwitch((*runtime).into())))
        }
        ["chat", ..] => Err(CliError::usage("usage: pinvou chat [--runtime <runtime>]")),
        ["runtime", ..] => Err(CliError::usage(
            "usage: pinvou runtime detect [runtime]|list|switch <runtime>",
        )),
        _ => Ok(None),
    }
}

#[derive(Debug)]
pub struct DistributedError {
    message: String,
    exit_code: StableExitCode,
}

impl DistributedError {
    fn new(exit_code: StableExitCode, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            exit_code,
        }
    }
    fn controller(message: impl Into<String>) -> Self {
        Self::new(StableExitCode::ControllerUnavailable, message)
    }
    fn runtime(message: impl Into<String>) -> Self {
        Self::new(StableExitCode::RuntimeFailed, message)
    }
    pub const fn exit_code(&self) -> StableExitCode {
        self.exit_code
    }
}

impl fmt::Display for DistributedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for DistributedError {}

pub fn require_interactive_terminal(
    stdin_tty: bool,
    stdout_tty: bool,
) -> Result<(), DistributedError> {
    if stdin_tty && stdout_tty {
        Ok(())
    } else {
        Err(DistributedError::new(
            StableExitCode::Usage,
            "chat requires an interactive terminal on stdin and stdout",
        ))
    }
}

pub fn map_error_causes(causes: impl IntoIterator<Item = ExitCause>) -> StableExitCode {
    StableExitCode::from_causal_chain(causes)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionAction {
    None,
    WriteText(String),
    AskApproval {
        approval_id: String,
        prompt: String,
    },
    AskInput {
        input_id: String,
        prompt: String,
    },
    RuntimeError {
        code: StableExitCode,
        runtime_code: String,
        message: String,
    },
    TurnEnded(StableExitCode),
}

#[derive(Debug)]
pub struct TerminalProjection {
    pending_text: String,
    next_text_frame: Instant,
}

impl TerminalProjection {
    pub fn new(now: Instant) -> Self {
        Self {
            pending_text: String::new(),
            // The Node has already paid its durable 50ms merge window. Render
            // the first frame immediately, then cap later terminal flushes.
            next_text_frame: now,
        }
    }

    pub fn push(
        &mut self,
        event: &RuntimeEventEnvelope,
        now: Instant,
    ) -> Result<ProjectionAction, DistributedError> {
        let payload: Value = serde_json::from_str(event.payload().get())
            .map_err(|_| DistributedError::runtime("runtime event payload is invalid"))?;
        match event.event_kind() {
            RuntimeEventKind::TextDelta => {
                let content = payload
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| DistributedError::runtime("text.delta has no content"))?;
                self.pending_text.push_str(content);
                Ok(self
                    .flush_due(now)
                    .map(ProjectionAction::WriteText)
                    .unwrap_or(ProjectionAction::None))
            }
            RuntimeEventKind::ApprovalRequested => Ok(ProjectionAction::AskApproval {
                approval_id: required_string(&payload, "approval_id")?,
                prompt: format!("{} [y/N] ", required_string(&payload, "summary")?),
            }),
            RuntimeEventKind::InputRequested => Ok(ProjectionAction::AskInput {
                input_id: required_string(&payload, "input_id")?,
                prompt: required_string(&payload, "prompt")?,
            }),
            RuntimeEventKind::TurnEnded => {
                let code = match required_string(&payload, "end_reason")?.as_str() {
                    "completed" => StableExitCode::Success,
                    "interrupted" | "cancelled" => StableExitCode::Cancelled,
                    _ => StableExitCode::RuntimeFailed,
                };
                Ok(ProjectionAction::TurnEnded(code))
            }
            RuntimeEventKind::ErrorRaised => {
                let runtime_code = required_string(&payload, "code")?;
                let message = required_string(&payload, "message")?;
                Ok(ProjectionAction::RuntimeError {
                    code: stable_code_for_runtime_error(&runtime_code),
                    runtime_code,
                    message,
                })
            }
            _ => Ok(ProjectionAction::None),
        }
    }

    pub fn flush_due(&mut self, now: Instant) -> Option<String> {
        if now < self.next_text_frame || self.pending_text.is_empty() {
            return None;
        }
        self.next_text_frame = now + CHAT_TEXT_FRAME;
        Some(std::mem::take(&mut self.pending_text))
    }
    pub fn flush_pending(&mut self) -> Option<String> {
        (!self.pending_text.is_empty()).then(|| std::mem::take(&mut self.pending_text))
    }
    pub fn parse_approval(&self, answer: &str) -> Option<bool> {
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => Some(true),
            "n" | "no" => Some(false),
            _ => None,
        }
    }
}

fn required_string(payload: &Value, field: &str) -> Result<String, DistributedError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| DistributedError::runtime(format!("runtime event has no {field}")))
}

fn stable_code_for_runtime_error(code: &str) -> StableExitCode {
    match code {
        "blocked_auth" | "auth_expired" => StableExitCode::BlockedAuth,
        "resource_exhausted" | "spool_exhausted" => StableExitCode::ResourceExhausted,
        "data_corruption" | "wal_corruption" | "spool_corruption" => StableExitCode::DataCorruption,
        "cancelled" | "timeout" => StableExitCode::Cancelled,
        _ => StableExitCode::RuntimeFailed,
    }
}

fn format_runtime_error_for_chat(runtime_code: &str, message: &str) -> String {
    format!(
        "\nruntime error: {runtime_code}\nmessage: {message}\nhint: run /detect to inspect the active runtime, or /runtime <id> to switch.\n"
    )
}

pub struct ControllerWire<S> {
    stream: S,
    instance_id: String,
    next_id: u64,
}

impl<S: Read + Write> ControllerWire<S> {
    pub fn from_authenticated(stream: S, instance_id: impl Into<String>) -> Self {
        Self {
            stream,
            instance_id: instance_id.into(),
            next_id: 1,
        }
    }
    pub fn chat_start(&mut self, prompt: &str) -> Result<IpcMessage, DistributedError> {
        self.request("chat.start", json!({"prompt": prompt}))
    }
    pub fn resolve_approval(
        &mut self,
        approval_id: &str,
        accepted: bool,
    ) -> Result<IpcMessage, DistributedError> {
        self.request(
            "approval.resolve",
            json!({"approval_id": approval_id, "accepted": accepted}),
        )
    }
    pub fn resolve_input(
        &mut self,
        input_id: &str,
        value: &str,
    ) -> Result<IpcMessage, DistributedError> {
        self.request(
            "input.resolve",
            json!({"input_id": input_id, "value": value}),
        )
    }
    pub fn interrupt_turn(&mut self, turn_id: &str) -> Result<IpcMessage, DistributedError> {
        self.request("turn.interrupt", json!({"turn_id": turn_id}))
    }
    pub fn runtime_detect(
        &mut self,
        runtime: Option<&str>,
    ) -> Result<IpcMessage, DistributedError> {
        let mut payload = json!({});
        if let Some(runtime) = runtime {
            payload["runtime"] = json!(runtime);
        }
        self.request("runtime.detect", payload)
    }
    pub fn runtime_list(&mut self) -> Result<IpcMessage, DistributedError> {
        self.request("runtime.list", json!({}))
    }
    pub fn runtime_switch(&mut self, runtime: &str) -> Result<IpcMessage, DistributedError> {
        self.request("runtime.switch", json!({"runtime": runtime}))
    }
    pub fn runtime_switch_prepare(
        &mut self,
        runtime: &str,
    ) -> Result<IpcMessage, DistributedError> {
        self.request("runtime.switch.prepare", json!({"runtime": runtime}))
    }
    pub fn runtime_switch_commit(
        &mut self,
        runtime: &str,
        switch_token: &str,
    ) -> Result<IpcMessage, DistributedError> {
        self.request(
            "runtime.switch.commit",
            json!({"runtime": runtime, "switch_token": switch_token}),
        )
    }
    pub fn health(&mut self) -> Result<IpcMessage, DistributedError> {
        self.request("health", json!({}))
    }
    pub fn read_next(&mut self) -> Result<IpcMessage, DistributedError> {
        read_frame(&mut self.stream)
            .map_err(|_| DistributedError::controller("controller IPC stream closed"))
    }
    pub fn into_inner(self) -> S {
        self.stream
    }

    fn request(
        &mut self,
        method: &str,
        mut payload: Value,
    ) -> Result<IpcMessage, DistributedError> {
        payload
            .as_object_mut()
            .ok_or_else(|| DistributedError::runtime("controller request payload is invalid"))?
            .insert(
                "instance_id".into(),
                Value::String(self.instance_id.clone()),
            );
        let id = self.next_id;
        self.next_id += 1;
        let request = IpcMessage::request(json!(id), method, payload)
            .map_err(|_| DistributedError::runtime("controller request is invalid"))?;
        self.stream
            .write_all(
                &encode_frame(&request)
                    .map_err(|_| DistributedError::runtime("controller request is too large"))?,
            )
            .map_err(|_| DistributedError::controller("controller IPC write failed"))?;
        self.stream
            .flush()
            .map_err(|_| DistributedError::controller("controller IPC flush failed"))?;
        let response = self.read_next()?;
        if response.kind() == IpcMessageKind::Err {
            return Err(error_from_response(response.payload()));
        }
        if response.kind() == IpcMessageKind::Rsp && response.id() != Some(&json!(id)) {
            return Err(DistributedError::controller(
                "controller response id does not match request",
            ));
        }
        Ok(response)
    }
}

fn error_from_response(payload: &Value) -> DistributedError {
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(1);
    let exit = match code {
        2 => StableExitCode::Usage,
        3 => StableExitCode::ControllerUnavailable,
        4 => StableExitCode::BlockedAuth,
        5 => StableExitCode::RuntimeFailed,
        6 => StableExitCode::Cancelled,
        7 => StableExitCode::ResourceExhausted,
        8 => StableExitCode::DataCorruption,
        _ => StableExitCode::Internal,
    };
    DistributedError::new(
        exit,
        payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("controller request failed"),
    )
}

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}
type LiveController = ControllerWire<Box<dyn ReadWrite>>;

fn connect_authenticated(endpoint: &LocalEndpoint) -> Result<LiveController, DistributedError> {
    let mut stream = connect_endpoint(endpoint)?;
    let hello = HelloClient::new(json!({"name":"pinvou-cli","pid":std::process::id()}))
        .map_err(|_| DistributedError::controller("controller hello is invalid"))?;
    stream
        .write_all(
            &encode_frame(&hello)
                .map_err(|_| DistributedError::controller("controller hello is too large"))?,
        )
        .map_err(|_| DistributedError::controller("controller hello write failed"))?;
    stream
        .flush()
        .map_err(|_| DistributedError::controller("controller hello flush failed"))?;
    let answer: HelloServer = read_frame(&mut stream)
        .map_err(|_| DistributedError::controller("controller IPC version mismatch"))?;
    Ok(ControllerWire::from_authenticated(
        stream,
        answer.instance_id(),
    ))
}

#[cfg(windows)]
fn connect_endpoint(endpoint: &LocalEndpoint) -> Result<Box<dyn ReadWrite>, DistributedError> {
    let LocalEndpoint::WindowsPipe(name) = endpoint else {
        return Err(DistributedError::controller(
            "controller endpoint is invalid",
        ));
    };
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name)
        .map(|stream| Box::new(stream) as Box<dyn ReadWrite>)
        .map_err(|_| DistributedError::controller("controller is not reachable"))
}

#[cfg(target_os = "linux")]
fn connect_endpoint(endpoint: &LocalEndpoint) -> Result<Box<dyn ReadWrite>, DistributedError> {
    let LocalEndpoint::UnixSocket(path) = endpoint else {
        return Err(DistributedError::controller(
            "controller endpoint is invalid",
        ));
    };
    std::os::unix::net::UnixStream::connect(path)
        .map(|stream| Box::new(stream) as Box<dyn ReadWrite>)
        .map_err(|_| DistributedError::controller("controller is not reachable"))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn connect_endpoint(_: &LocalEndpoint) -> Result<Box<dyn ReadWrite>, DistributedError> {
    Err(DistributedError::controller(
        "controller platform is unsupported",
    ))
}

fn ensure_controller() -> Result<LiveController, DistributedError> {
    let paths = ControllerPaths::discover()
        .map_err(|_| DistributedError::controller("controller paths are unavailable"))?;
    if let Ok(mut client) = connect_authenticated(paths.endpoint()) {
        client.health()?;
        return Ok(client);
    }
    let executable = controller_executable()?;
    let controller_args = controller_args_for_test();
    DetachedLaunch::for_platform(
        HostPlatform::current()
            .map_err(|_| DistributedError::controller("controller platform is unsupported"))?,
    )
    .spawn(&executable, &controller_args)
    .map_err(|_| DistributedError::controller("controller could not be started"))?;
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(mut client) = connect_authenticated(paths.endpoint())
            && client.health().is_ok()
        {
            return Ok(client);
        }
        std::thread::sleep(READY_POLL);
    }
    Err(DistributedError::controller(
        "controller did not become healthy before timeout",
    ))
}

fn controller_args_for_test() -> Vec<&'static str> {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("PINVOU_CONTROLLER_ONCE_FOR_TEST").is_some() {
            return vec!["--once-for-test"];
        }
    }
    Vec::new()
}

fn controller_executable() -> Result<PathBuf, DistributedError> {
    if let Some(path) = std::env::var_os("PINVOU_CONTROLLER_EXE").map(PathBuf::from) {
        return validate_executable(path);
    }
    let directory = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .ok_or_else(|| DistributedError::controller("controller executable is unavailable"))?;
    #[cfg(windows)]
    let candidate = directory.join("pinvou-controller.exe");
    #[cfg(not(windows))]
    let candidate = directory.join("pinvou-controller");
    validate_executable(candidate)
}

fn validate_executable(path: PathBuf) -> Result<PathBuf, DistributedError> {
    let path = path
        .canonicalize()
        .map_err(|_| DistributedError::controller("controller executable is unavailable"))?;
    if path.is_absolute() && path.metadata().is_ok_and(|metadata| metadata.is_file()) {
        Ok(path)
    } else {
        Err(DistributedError::controller(
            "controller executable is unavailable",
        ))
    }
}

pub(crate) fn execute(
    command: DistributedCommand,
    output: OutputMode,
) -> Result<String, DistributedError> {
    match command {
        DistributedCommand::Chat { runtime } => execute_chat(runtime.as_deref()),
        DistributedCommand::RuntimeDetect { runtime } => {
            let response = ensure_controller()?.runtime_detect(runtime.as_deref())?;
            Ok(match output {
                OutputMode::Json => response.payload().to_string(),
                OutputMode::Human => format_runtime_detect(response.payload()),
            })
        }
        DistributedCommand::RuntimeList => {
            let response = ensure_controller()?.runtime_list()?;
            Ok(match output {
                OutputMode::Json => response.payload().to_string(),
                OutputMode::Human => format_runtime_list(response.payload()),
            })
        }
        DistributedCommand::RuntimeSwitch(runtime) => {
            let mut controller = ensure_controller()?;
            execute_runtime_switch_with_controller(&mut controller, &runtime, output)
        }
    }
}

fn execute_runtime_switch_with_controller<S: Read + Write>(
    controller: &mut ControllerWire<S>,
    runtime: &str,
    output: OutputMode,
) -> Result<String, DistributedError> {
    let detect = controller.runtime_detect(Some(runtime))?;
    if let Some(code) = blocking_code_for_runtime_detect(detect.payload()) {
        return Err(DistributedError::new(
            code,
            format!("runtime {runtime} is not ready"),
        ));
    }
    let response = prepare_and_commit_runtime_switch(controller, runtime)?;
    Ok(match output {
        OutputMode::Json => response.payload().to_string(),
        OutputMode::Human => {
            let selected = response
                .payload()
                .get("runtime")
                .and_then(Value::as_str)
                .unwrap_or(runtime);
            format_runtime_switch_human(selected, detect.payload())
        }
    })
}

fn prepare_and_commit_runtime_switch<S: Read + Write>(
    client: &mut ControllerWire<S>,
    runtime: &str,
) -> Result<IpcMessage, DistributedError> {
    let prepared = client.runtime_switch_prepare(runtime)?;
    let switch_token = prepared
        .payload()
        .get("switch_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            DistributedError::new(
                StableExitCode::RuntimeFailed,
                "runtime switch prepare response is invalid",
            )
        })?;
    client.runtime_switch_commit(runtime, switch_token)
}

fn execute_chat(initial_runtime: Option<&str>) -> Result<String, DistributedError> {
    if !assume_interactive_for_test() {
        require_interactive_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
    }
    let interrupted = Arc::new(AtomicBool::new(false));
    let active_turn = Arc::new(Mutex::new(None::<String>));
    install_interrupt_handler(Arc::clone(&interrupted), Arc::clone(&active_turn))?;
    let mut client = ensure_controller()?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if let Some(runtime) = initial_runtime {
        write_initial_runtime_switch(&mut output, &mut client, runtime)?;
    } else {
        write_chat_startup_banner(&mut output, &mut client)?;
    }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let control_endpoint = ControllerPaths::discover()
        .map_err(|_| DistributedError::controller("controller paths are unavailable"))?
        .endpoint()
        .clone();
    let mut resolve_approval = |approval_id: &str, accepted: bool| {
        connect_authenticated(&control_endpoint)?
            .resolve_approval(approval_id, accepted)
            .map(|_| ())
    };
    let mut resolve_input = |input_id: &str, value: &str| {
        connect_authenticated(&control_endpoint)?
            .resolve_input(input_id, value)
            .map(|_| ())
    };
    execute_chat_with_io_and_control(
        &mut input,
        &mut output,
        &mut client,
        &mut resolve_approval,
        &mut resolve_input,
        interrupted,
        active_turn,
        assume_interactive_for_test(),
    )?;
    Ok(String::new())
}

fn assume_interactive_for_test() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("PINVOU_ASSUME_INTERACTIVE_TTY_FOR_TEST").is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

#[cfg(test)]
fn execute_chat_with_io<R: BufRead, W: Write, S: Read + Write>(
    input: &mut R,
    output: &mut W,
    client: &mut ControllerWire<S>,
    interrupted: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<String>>>,
    peek_eof_before_prompt: bool,
) -> Result<(), DistributedError> {
    let mut reject_approval = |_: &str, _: bool| {
        Err(DistributedError::controller(
            "chat approval requires a dedicated controller connection",
        ))
    };
    let mut reject_input = |_: &str, _: &str| {
        Err(DistributedError::controller(
            "chat input requires a dedicated controller connection",
        ))
    };
    execute_chat_with_io_and_control(
        input,
        output,
        client,
        &mut reject_approval,
        &mut reject_input,
        interrupted,
        active_turn,
        peek_eof_before_prompt,
    )
}

fn execute_chat_with_io_and_control<
    R: BufRead,
    W: Write,
    S: Read + Write,
    A: FnMut(&str, bool) -> Result<(), DistributedError>,
    I: FnMut(&str, &str) -> Result<(), DistributedError>,
>(
    input: &mut R,
    output: &mut W,
    client: &mut ControllerWire<S>,
    resolve_approval: &mut A,
    resolve_input: &mut I,
    interrupted: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<String>>>,
    peek_eof_before_prompt: bool,
) -> Result<(), DistributedError> {
    // After chat.start yields its first event, this connection is an event stream until
    // turn.ended. Approval and input callbacks must use independent controller connections.
    let mut prompt = String::new();
    let mut projection = TerminalProjection::new(Instant::now());
    loop {
        if peek_eof_before_prompt
            && input
                .fill_buf()
                .map_err(|_| {
                    DistributedError::new(StableExitCode::Internal, "terminal read failed")
                })?
                .is_empty()
        {
            return Ok(());
        }
        write_terminal_to(output, "You: ")?;
        prompt.clear();
        let bytes = input
            .read_line(&mut prompt)
            .map_err(|_| DistributedError::new(StableExitCode::Internal, "terminal read failed"))?;
        if bytes == 0 {
            return Ok(());
        }
        let prompt = prompt.trim();
        if matches!(prompt, "/exit" | "/quit") {
            return Ok(());
        }
        if prompt.is_empty() {
            continue;
        }
        if handle_slash_command(prompt, output, client)? {
            continue;
        }

        let first = client.chat_start(prompt)?;
        let mut next = Some(first);
        loop {
            if interrupted.load(Ordering::SeqCst) {
                return Err(DistributedError::new(
                    StableExitCode::Cancelled,
                    "chat interrupted by user",
                ));
            }
            let message = match next.take() {
                Some(value) => value,
                None => client.read_next()?,
            };
            if message.kind() == IpcMessageKind::Err {
                return Err(error_from_response(message.payload()));
            }
            if message.topic() != Some("runtime.event") {
                continue;
            }
            let event =
                RuntimeEventEnvelope::from_value(message.payload().clone()).map_err(|_| {
                    DistributedError::runtime("controller returned an invalid runtime event")
                })?;
            if let Some(turn_id) = event.turn_id()
                && let Ok(mut active) = active_turn.lock()
            {
                *active = Some(turn_id.to_owned());
            }
            match projection.push(&event, Instant::now())? {
                ProjectionAction::None => {}
                ProjectionAction::WriteText(text) => write_terminal_to(output, &text)?,
                ProjectionAction::AskApproval {
                    approval_id,
                    prompt,
                } => {
                    write_terminal_to(output, &prompt)?;
                    let mut answer = String::new();
                    input.read_line(&mut answer).map_err(|_| {
                        DistributedError::new(StableExitCode::Internal, "terminal read failed")
                    })?;
                    let accepted = projection.parse_approval(&answer).ok_or_else(|| {
                        DistributedError::new(StableExitCode::Usage, "approval requires y or n")
                    })?;
                    resolve_approval(&approval_id, accepted)?;
                }
                ProjectionAction::AskInput { input_id, prompt } => {
                    write_terminal_to(output, &format!("{prompt} "))?;
                    let mut answer = String::new();
                    input.read_line(&mut answer).map_err(|_| {
                        DistributedError::new(StableExitCode::Internal, "terminal read failed")
                    })?;
                    resolve_input(&input_id, answer.trim_end())?;
                }
                ProjectionAction::RuntimeError {
                    code,
                    runtime_code,
                    message,
                } => {
                    if let Some(text) = projection.flush_pending() {
                        write_terminal_to(output, &text)?;
                    }
                    write_terminal_to(
                        output,
                        &format_runtime_error_for_chat(&runtime_code, &message),
                    )?;
                    if let Ok(mut active) = active_turn.lock() {
                        *active = None;
                    }
                    return Err(DistributedError::new(
                        code,
                        format!("runtime {runtime_code}"),
                    ));
                }
                ProjectionAction::TurnEnded(code) => {
                    if let Some(text) = projection.flush_pending() {
                        write_terminal_to(output, &text)?;
                    }
                    if let Ok(mut active) = active_turn.lock() {
                        *active = None;
                    }
                    if code == StableExitCode::Success {
                        if peek_eof_before_prompt
                            && input
                                .fill_buf()
                                .map_err(|_| {
                                    DistributedError::new(
                                        StableExitCode::Internal,
                                        "terminal read failed",
                                    )
                                })?
                                .is_empty()
                        {
                            return Ok(());
                        }
                        write_terminal_to(output, "\n")?;
                        break;
                    }
                    return Err(DistributedError::new(code, "chat turn did not complete"));
                }
            }
        }
    }
}

fn handle_slash_command<S: Read + Write>(
    prompt: &str,
    output: &mut impl Write,
    client: &mut ControllerWire<S>,
) -> Result<bool, DistributedError> {
    let Some(command) = prompt.strip_prefix('/') else {
        return Ok(false);
    };
    let mut parts = command.split_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some("help"), None, None) => {
            write_terminal_to(output, chat_help_text())?;
            Ok(true)
        }
        (Some("runtime") | Some("runtimes"), None, None) => {
            let response = client.runtime_list()?;
            write_terminal_to(output, &format_runtime_list(response.payload()))?;
            Ok(true)
        }
        (Some("runtime"), Some(runtime), None) => {
            let detect = client.runtime_detect(Some(runtime))?;
            if blocking_code_for_runtime_detect(detect.payload()).is_some() {
                write_terminal_to(output, &format_runtime_detect(detect.payload()))?;
                write_terminal_to(output, "runtime not switched\n")?;
                return Ok(true);
            }
            let response = prepare_and_commit_runtime_switch(client, runtime)?;
            let selected = response
                .payload()
                .get("runtime")
                .and_then(Value::as_str)
                .unwrap_or(runtime);
            write_terminal_to(output, &format!("runtime switched to {selected}\n"))?;
            write_terminal_to(output, &format_runtime_detect(detect.payload()))?;
            Ok(true)
        }
        (Some("detect"), maybe_runtime, None) => {
            let response = client.runtime_detect(maybe_runtime)?;
            write_terminal_to(output, &format_runtime_detect(response.payload()))?;
            Ok(true)
        }
        _ => Err(DistributedError::new(
            StableExitCode::Usage,
            "unknown chat command",
        )),
    }
}

fn write_chat_startup_banner<S: Read + Write>(
    output: &mut impl Write,
    client: &mut ControllerWire<S>,
) -> Result<(), DistributedError> {
    let response = client.runtime_list()?;
    write_terminal_to(output, &format_runtime_list(response.payload()))
}

fn write_initial_runtime_switch<S: Read + Write>(
    output: &mut impl Write,
    client: &mut ControllerWire<S>,
    runtime: &str,
) -> Result<(), DistributedError> {
    let detect = client.runtime_detect(Some(runtime))?;
    if let Some(code) = blocking_code_for_runtime_detect(detect.payload()) {
        write_terminal_to(output, &format_runtime_detect(detect.payload()))?;
        write_terminal_to(output, "runtime not switched\n")?;
        return Err(DistributedError::new(
            code,
            format!("runtime {runtime} is not ready"),
        ));
    }
    let response = prepare_and_commit_runtime_switch(client, runtime)?;
    let selected = response
        .payload()
        .get("runtime")
        .and_then(Value::as_str)
        .unwrap_or(runtime);
    write_terminal_to(
        output,
        &format_runtime_switch_human(selected, detect.payload()),
    )
}

fn chat_help_text() -> &'static str {
    "/help - show chat commands\n/runtime or /runtimes - list selectable runtimes\n/runtime <id> - switch active runtime\n/detect [id] - show active or named runtime status\n/exit or /quit - leave chat\n"
}

fn format_runtime_detect(payload: &Value) -> String {
    let runtime = payload
        .get("runtime")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut text = format!("runtime: {runtime}\nstatus: {status}\n");
    if let Some(auth) = payload.get("auth_status").and_then(Value::as_str) {
        text.push_str(&format!("auth: {auth}\n"));
    }
    if let Some(capabilities) = payload.get("capabilities").and_then(Value::as_object) {
        for key in ["interactive_chat", "tool_approval", "elicitation"] {
            if let Some(value) = capabilities.get(key).and_then(Value::as_bool) {
                text.push_str(&format!("{key}: {}\n", if value { "yes" } else { "no" }));
            }
        }
    }
    if let Some(error) = payload.get("error_kind").and_then(Value::as_str) {
        text.push_str(&format!("error: {error}\n"));
    }
    if let Some(exit_code) = payload.get("exit_code").and_then(Value::as_i64) {
        text.push_str(&format!("exit_code: {exit_code}\n"));
    }
    if status != "available" {
        text.push_str(
            "hint: run /detect again after fixing the runtime, or /runtime <id> to switch.\n",
        );
    }
    text
}

fn format_runtime_switch_human(selected: &str, detect_payload: &Value) -> String {
    format!(
        "runtime switched to {selected}\n{}",
        format_runtime_detect(detect_payload)
    )
}

fn blocking_code_for_runtime_detect(payload: &Value) -> Option<StableExitCode> {
    match payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "available" => None,
        status => Some(
            payload
                .get("error_kind")
                .and_then(Value::as_str)
                .map(stable_code_for_runtime_error)
                .unwrap_or_else(|| stable_code_for_runtime_error(status)),
        ),
    }
}

fn format_runtime_list(payload: &Value) -> String {
    let current = payload
        .get("current")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut text = format!("runtime: {current}\n");
    if let Some(runtimes) = payload.get("runtimes").and_then(Value::as_array) {
        for runtime in runtimes {
            let id = runtime
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let label = runtime.get("label").and_then(Value::as_str).unwrap_or(id);
            let status = if runtime
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "available"
            } else {
                "unavailable"
            };
            let marker = if id == current { "*" } else { " " };
            text.push_str(&format!("{marker} {id} - {label} ({status})\n"));
        }
    }
    text.push_str("switch: /runtime <id>\ninspect: /detect <id>\n");
    text
}

fn install_interrupt_handler(
    interrupted: Arc<AtomicBool>,
    active_turn: Arc<Mutex<Option<String>>>,
) -> Result<(), DistributedError> {
    ctrlc::set_handler(move || {
        interrupted.store(true, Ordering::SeqCst);
        let turn_id = active_turn.lock().ok().and_then(|turn| turn.clone());
        if let Some(turn_id) = turn_id
            && let Ok(mut client) = ensure_controller()
        {
            let _ = client.interrupt_turn(&turn_id);
        }
    })
    .map_err(|_| DistributedError::new(StableExitCode::Internal, "Ctrl+C handler failed"))
}

fn write_terminal_to(output: &mut impl Write, text: &str) -> Result<(), DistributedError> {
    output
        .write_all(text.as_bytes())
        .and_then(|_| output.flush())
        .map_err(|_| DistributedError::new(StableExitCode::Internal, "terminal write failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_switch_prepare_response(id: u64, runtime: &str, token: &str) -> IpcMessage {
        IpcMessage::response(
            json!(id),
            json!({
                "status": "ready",
                "runtime": runtime,
                "current_runtime": "echo",
                "switch_token": token,
                "requires_compression": false,
                "context": {"strategy":"none", "reason":"turn_boundary_clean"},
                "tools": {"policy":"portable_or_replay_only"}
            }),
        )
        .unwrap()
    }

    fn runtime_switch_commit_response(id: u64, runtime: &str, token: &str) -> IpcMessage {
        IpcMessage::response(
            json!(id),
            json!({"status": "ok", "runtime": runtime, "switch_token": token}),
        )
        .unwrap()
    }

    #[test]
    fn chat_loop_sends_prompt_and_projects_runtime_events() {
        let text = RuntimeEventEnvelope::from_value(json!({
            "protocol_version": pinvou_protocol::IPC_VERSION,
            "schema_version": 1,
            "node_id": "node-a",
            "logical_session_id": "session-a",
            "attachment_id": "attachment-a",
            "work_id": null,
            "collaborative_run_id": null,
            "stream_id": "main",
            "turn_id": "turn-a",
            "seq": 1,
            "source_span": null,
            "timestamp": "2026-08-21T00:00:00Z",
            "rate_class": "R1",
            "kind": "text.delta",
            "payload": {"role":"assistant","content":"hello from node","merged_count":1}
        }))
        .unwrap();
        let ended = RuntimeEventEnvelope::from_value(json!({
            "protocol_version": pinvou_protocol::IPC_VERSION,
            "schema_version": 1,
            "node_id": "node-a",
            "logical_session_id": "session-a",
            "attachment_id": "attachment-a",
            "work_id": null,
            "collaborative_run_id": null,
            "stream_id": "control",
            "turn_id": "turn-a",
            "seq": 1,
            "source_span": null,
            "timestamp": "2026-08-21T00:00:00Z",
            "rate_class": "R0",
            "kind": "turn.ended",
            "payload": {"end_reason":"completed","error":null}
        }))
        .unwrap();
        let stream = TestDuplex::with_responses([
            IpcMessage::event("runtime.event", serde_json::to_value(text).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(ended).unwrap()).unwrap(),
        ]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"hello controller\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "You: hello from node");
        assert_eq!(active_turn.lock().unwrap().as_deref(), None);
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("chat.start"));
        assert_eq!(requests[0].payload()["prompt"], "hello controller");
        assert_eq!(
            requests[0].payload()["instance_id"],
            json!("controller-instance")
        );
    }

    #[test]
    fn chat_loop_sends_approval_over_a_dedicated_control_channel() {
        let approval = runtime_event(
            "turn-control",
            1,
            "control",
            "R0",
            "approval.requested",
            json!({
                "approval_id":"approval-1",
                "tool":"shell",
                "summary":"run tests",
                "options":["approved","denied"],
                "timeout_ms":30000
            }),
        );
        let text = runtime_event(
            "turn-control",
            2,
            "main",
            "R1",
            "text.delta",
            json!({"role":"assistant","content":"approved","merged_count":1}),
        );
        let ended = runtime_event(
            "turn-control",
            3,
            "control",
            "R0",
            "turn.ended",
            json!({"end_reason":"completed","error":null}),
        );
        let stream = TestDuplex::with_responses([
            IpcMessage::event("runtime.event", serde_json::to_value(approval).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(text).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(ended).unwrap()).unwrap(),
        ]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let mut approvals = Vec::new();
        let mut inputs = Vec::new();
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"run something\ny\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io_and_control(
            &mut input,
            &mut output,
            &mut client,
            &mut |approval_id, accepted| {
                approvals.push((approval_id.to_owned(), accepted));
                Ok(())
            },
            &mut |input_id, value| {
                inputs.push((input_id.to_owned(), value.to_owned()));
                Ok(())
            },
            interrupted,
            active_turn,
            true,
        )
        .unwrap();

        assert_eq!(approvals, [("approval-1".into(), true)]);
        assert!(inputs.is_empty());
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("chat.start"));
    }

    #[test]
    fn chat_loop_sends_input_over_a_dedicated_control_channel() {
        let requested = runtime_event(
            "turn-input",
            1,
            "control",
            "R0",
            "input.requested",
            json!({"input_id":"input-1","prompt":"Name?","schema":{"type":"string"}}),
        );
        let ended = runtime_event(
            "turn-input",
            2,
            "control",
            "R0",
            "turn.ended",
            json!({"end_reason":"completed","error":null}),
        );
        let stream = TestDuplex::with_responses([
            IpcMessage::event("runtime.event", serde_json::to_value(requested).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(ended).unwrap()).unwrap(),
        ]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let mut approvals = Vec::new();
        let mut inputs = Vec::new();
        let mut input = std::io::Cursor::new(b"answer a question\nPinvou\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io_and_control(
            &mut input,
            &mut output,
            &mut client,
            &mut |approval_id, accepted| {
                approvals.push((approval_id.to_owned(), accepted));
                Ok(())
            },
            &mut |input_id, value| {
                inputs.push((input_id.to_owned(), value.to_owned()));
                Ok(())
            },
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(None)),
            true,
        )
        .unwrap();

        assert!(approvals.is_empty());
        assert_eq!(inputs, [("input-1".into(), "Pinvou".into())]);
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("chat.start"));
    }

    #[test]
    fn chat_loop_accepts_multiple_prompts_until_exit() {
        let first_text = runtime_event(
            "turn-a",
            1,
            "main",
            "R1",
            "text.delta",
            json!({"role":"assistant","content":"first reply","merged_count":1}),
        );
        let first_ended = runtime_event(
            "turn-a",
            1,
            "control",
            "R0",
            "turn.ended",
            json!({"end_reason":"completed","error":null}),
        );
        let second_text = runtime_event(
            "turn-b",
            1,
            "main",
            "R1",
            "text.delta",
            json!({"role":"assistant","content":"second reply","merged_count":1}),
        );
        let second_ended = runtime_event(
            "turn-b",
            1,
            "control",
            "R0",
            "turn.ended",
            json!({"end_reason":"completed","error":null}),
        );
        let stream = TestDuplex::with_responses([
            IpcMessage::event("runtime.event", serde_json::to_value(first_text).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(first_ended).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(second_text).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(second_ended).unwrap())
                .unwrap(),
        ]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"first\nsecond\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "You: first reply\nYou: second reply\nYou: "
        );
        assert_eq!(active_turn.lock().unwrap().as_deref(), None);
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method(), Some("chat.start"));
        assert_eq!(requests[0].payload()["prompt"], "first");
        assert_eq!(requests[1].method(), Some("chat.start"));
        assert_eq!(requests[1].payload()["prompt"], "second");
    }

    #[test]
    fn chat_loop_clears_active_turn_after_turn_end_before_waiting_for_next_prompt() {
        let text = runtime_event(
            "turn-clean",
            1,
            "main",
            "R1",
            "text.delta",
            json!({"role":"assistant","content":"finished","merged_count":1}),
        );
        let ended = runtime_event(
            "turn-clean",
            1,
            "control",
            "R0",
            "turn.ended",
            json!({"end_reason":"completed","error":null}),
        );
        let stream = TestDuplex::with_responses([
            IpcMessage::event("runtime.event", serde_json::to_value(text).unwrap()).unwrap(),
            IpcMessage::event("runtime.event", serde_json::to_value(ended).unwrap()).unwrap(),
        ]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"finish once\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "You: finished");
        assert_eq!(active_turn.lock().unwrap().as_deref(), None);
    }

    #[test]
    fn chat_loop_supports_runtime_slash_commands_without_starting_a_turn() {
        let list_response = IpcMessage::response(
            json!(1),
            json!({
                "current": "echo",
                "runtimes": [
                    {"id": "echo", "label": "Stage 1 Echo", "available": true}
                ]
            }),
        )
        .unwrap();
        let prepare_response = runtime_switch_prepare_response(3, "echo", "switch-token-1");
        let commit_response = runtime_switch_commit_response(4, "echo", "switch-token-1");
        let detect_response = IpcMessage::response(
            json!(2),
            json!({
                "runtime": "echo",
                "status": "available",
                "auth_status": "not_required",
                "capabilities": {
                    "interactive_chat": true
                }
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([
            list_response,
            detect_response,
            prepare_response,
            commit_response,
        ]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/runtime\n/runtime echo\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime: echo"));
        assert!(output.contains("* echo - Stage 1 Echo (available)"));
        assert!(output.contains("switch: /runtime <id>"));
        assert!(output.contains("inspect: /detect <id>"));
        assert!(output.contains("runtime switched to echo"));
        assert!(output.contains("status: available"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method(), Some("runtime.list"));
        assert_eq!(requests[1].method(), Some("runtime.detect"));
        assert_eq!(requests[1].payload()["runtime"], "echo");
        assert_eq!(requests[2].method(), Some("runtime.switch.prepare"));
        assert_eq!(requests[2].payload()["runtime"], "echo");
        assert_eq!(requests[3].method(), Some("runtime.switch.commit"));
        assert_eq!(requests[3].payload()["runtime"], "echo");
        assert_eq!(requests[3].payload()["switch_token"], "switch-token-1");
    }

    #[test]
    fn chat_loop_accepts_runtimes_alias_without_starting_a_turn() {
        let list_response = IpcMessage::response(
            json!(1),
            json!({
                "current": "codex",
                "runtimes": [
                    {"id": "echo", "label": "Stage 1 Echo", "available": true},
                    {"id": "codex", "label": "Codex App Server", "available": false}
                ]
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([list_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/runtimes\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("  echo - Stage 1 Echo (available)"));
        assert!(output.contains("* codex - Codex App Server (unavailable)"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.list"));
        assert!(requests[0].payload().get("text").is_none());
    }

    #[test]
    fn chat_startup_banner_prints_active_runtime_without_starting_a_turn() {
        let list_response = IpcMessage::response(
            json!(1),
            json!({
                "current": "codex",
                "runtimes": [
                    {"id": "echo", "label": "Stage 1 Echo", "available": true},
                    {"id": "codex", "label": "Codex App Server", "available": true}
                ]
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([list_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let mut output = Vec::new();

        write_chat_startup_banner(&mut output, &mut client).unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("* codex - Codex App Server (available)"));
        assert!(output.contains("switch: /runtime <id>"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.list"));
    }

    #[test]
    fn chat_runtime_switch_prints_the_new_runtime_status() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "available",
                "auth_status": "signed_in",
                "capabilities": {
                    "interactive_chat": true
                }
            }),
        )
        .unwrap();
        let prepare_response = runtime_switch_prepare_response(2, "codex", "switch-token-2");
        let commit_response = runtime_switch_commit_response(3, "codex", "switch-token-2");
        let stream =
            TestDuplex::with_responses([detect_response, prepare_response, commit_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/runtime codex\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            active_turn,
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime switched to codex"));
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("status: available"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
        assert_eq!(requests[1].method(), Some("runtime.switch.prepare"));
        assert_eq!(requests[1].payload()["runtime"], "codex");
        assert_eq!(requests[2].method(), Some("runtime.switch.commit"));
        assert_eq!(requests[2].payload()["runtime"], "codex");
        assert_eq!(requests[2].payload()["switch_token"], "switch-token-2");
    }

    #[test]
    fn chat_runtime_switch_refuses_unavailable_target_without_changing_active_runtime() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "blocked_auth",
                "error_kind": "blocked_auth",
                "exit_code": 4,
                "message": "runtime authentication is blocked"
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([detect_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/runtime codex\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            active_turn,
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("status: blocked_auth"));
        assert!(output.contains("runtime not switched"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
    }

    #[test]
    fn runtime_switch_command_detects_target_before_persisting_the_selection() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "available",
                "auth_status": "signed_in",
                "capabilities": {
                    "interactive_chat": true
                }
            }),
        )
        .unwrap();
        let prepare_response = runtime_switch_prepare_response(2, "codex", "switch-token-3");
        let commit_response = runtime_switch_commit_response(3, "codex", "switch-token-3");
        let stream =
            TestDuplex::with_responses([detect_response, prepare_response, commit_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");

        let output =
            execute_runtime_switch_with_controller(&mut client, "codex", OutputMode::Human)
                .unwrap();

        assert!(output.contains("runtime switched to codex"));
        assert!(output.contains("status: available"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
        assert_eq!(requests[1].method(), Some("runtime.switch.prepare"));
        assert_eq!(requests[1].payload()["runtime"], "codex");
        assert_eq!(requests[2].method(), Some("runtime.switch.commit"));
        assert_eq!(requests[2].payload()["runtime"], "codex");
        assert_eq!(requests[2].payload()["switch_token"], "switch-token-3");
    }

    #[test]
    fn runtime_switch_command_refuses_unavailable_target_without_persisting_it() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "blocked_auth",
                "error_kind": "blocked_auth",
                "exit_code": 4,
                "message": "runtime authentication is blocked"
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([detect_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");

        let error = execute_runtime_switch_with_controller(&mut client, "codex", OutputMode::Human)
            .unwrap_err();

        assert_eq!(error.exit_code(), StableExitCode::BlockedAuth);
        assert!(error.to_string().contains("runtime codex is not ready"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
    }

    #[test]
    fn chat_startup_runtime_switch_detects_before_persisting_target() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "available",
                "auth_status": "signed_in",
                "capabilities": {
                    "interactive_chat": true
                }
            }),
        )
        .unwrap();
        let prepare_response = runtime_switch_prepare_response(2, "codex", "switch-token-4");
        let commit_response = runtime_switch_commit_response(3, "codex", "switch-token-4");
        let stream =
            TestDuplex::with_responses([detect_response, prepare_response, commit_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let mut output = Vec::new();

        write_initial_runtime_switch(&mut output, &mut client, "codex").unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime switched to codex"));
        assert!(output.contains("status: available"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
        assert_eq!(requests[1].method(), Some("runtime.switch.prepare"));
        assert_eq!(requests[1].payload()["runtime"], "codex");
        assert_eq!(requests[2].method(), Some("runtime.switch.commit"));
        assert_eq!(requests[2].payload()["runtime"], "codex");
        assert_eq!(requests[2].payload()["switch_token"], "switch-token-4");
    }

    #[test]
    fn chat_startup_runtime_switch_prints_status_and_refuses_unavailable_target() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "blocked_auth",
                "error_kind": "blocked_auth",
                "exit_code": 4,
                "message": "runtime authentication is blocked"
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([detect_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let mut output = Vec::new();

        let error = write_initial_runtime_switch(&mut output, &mut client, "codex").unwrap_err();

        assert_eq!(error.exit_code(), StableExitCode::BlockedAuth);
        assert!(error.to_string().contains("runtime codex is not ready"));
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("status: blocked_auth"));
        assert!(output.contains("runtime not switched"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
    }

    #[test]
    fn chat_loop_help_lists_runtime_switching_commands_without_starting_a_turn() {
        let stream = TestDuplex::with_responses([]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/help\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("/runtime or /runtimes - list selectable runtimes"));
        assert!(output.contains("/runtime <id> - switch active runtime"));
        assert!(output.contains("/detect [id] - show active or named runtime status"));
        assert!(client.into_inner().requests().is_empty());
    }

    #[test]
    fn chat_loop_detect_prints_the_runtime_status_card_without_starting_a_turn() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "blocked_auth",
                "error_kind": "blocked_auth",
                "exit_code": 4
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([detect_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/detect\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("status: blocked_auth"));
        assert!(output.contains("error: blocked_auth"));
        assert!(output.contains("exit_code: 4"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
    }

    #[test]
    fn chat_loop_detect_can_probe_a_named_runtime_without_starting_a_turn() {
        let detect_response = IpcMessage::response(
            json!(1),
            json!({
                "runtime": "codex",
                "status": "available",
                "auth_status": "authenticated",
                "capabilities": {
                    "interactive_chat": true
                }
            }),
        )
        .unwrap();
        let stream = TestDuplex::with_responses([detect_response]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"/detect codex\n/exit\n".to_vec());
        let mut output = Vec::new();

        execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            active_turn,
            true,
        )
        .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("status: available"));
        let requests = client.into_inner().requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method(), Some("runtime.detect"));
        assert_eq!(requests[0].payload()["runtime"], "codex");
    }

    #[test]
    fn chat_loop_prints_actionable_runtime_errors_before_returning() {
        let runtime_error = runtime_event(
            "turn-auth",
            1,
            "control",
            "R0",
            "error.raised",
            json!({
                "code": "blocked_auth",
                "message": "Codex runtime is not signed in",
                "fatal": true,
                "source": "runtime"
            }),
        );
        let stream = TestDuplex::with_responses([IpcMessage::event(
            "runtime.event",
            serde_json::to_value(runtime_error).unwrap(),
        )
        .unwrap()]);
        let mut client = ControllerWire::from_authenticated(stream, "controller-instance");
        let interrupted = Arc::new(AtomicBool::new(false));
        let active_turn = Arc::new(Mutex::new(None));
        let mut input = std::io::Cursor::new(b"hello codex\n".to_vec());
        let mut output = Vec::new();

        let error = execute_chat_with_io(
            &mut input,
            &mut output,
            &mut client,
            interrupted,
            Arc::clone(&active_turn),
            true,
        )
        .unwrap_err();

        assert_eq!(error.exit_code(), StableExitCode::BlockedAuth);
        assert_eq!(error.to_string(), "runtime blocked_auth");
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("runtime error: blocked_auth"));
        assert!(output.contains("Codex runtime is not signed in"));
        assert!(output.contains("hint: run /detect"));
        assert_eq!(active_turn.lock().unwrap().as_deref(), None);
    }

    #[test]
    fn runtime_detect_status_card_shows_auth_capabilities_and_errors() {
        let available = format_runtime_detect(&json!({
            "runtime": "codex",
            "status": "available",
            "auth_status": "authenticated",
            "capabilities": {
                "interactive_chat": true,
                "tool_approval": true,
                "elicitation": false
            }
        }));
        assert!(available.contains("runtime: codex"));
        assert!(available.contains("status: available"));
        assert!(available.contains("auth: authenticated"));
        assert!(available.contains("interactive_chat: yes"));
        assert!(available.contains("tool_approval: yes"));
        assert!(available.contains("elicitation: no"));

        let blocked = format_runtime_detect(&json!({
            "runtime": "codex",
            "status": "blocked_auth",
            "error_kind": "blocked_auth",
            "exit_code": 4,
            "message": "runtime authentication is blocked"
        }));
        assert!(blocked.contains("status: blocked_auth"));
        assert!(blocked.contains("error: blocked_auth"));
        assert!(blocked.contains("exit_code: 4"));
    }

    #[test]
    fn runtime_switch_human_output_includes_the_follow_up_detect_card() {
        let output = format_runtime_switch_human(
            "codex",
            &json!({
                "runtime": "codex",
                "status": "blocked_auth",
                "error_kind": "blocked_auth",
                "exit_code": 4
            }),
        );

        assert!(output.contains("runtime switched to codex"));
        assert!(output.contains("runtime: codex"));
        assert!(output.contains("status: blocked_auth"));
        assert!(output.contains("hint: run /detect"));
    }

    #[test]
    fn initial_runtime_detect_status_blocks_chat_before_user_prompt() {
        assert_eq!(
            blocking_code_for_runtime_detect(&json!({"status":"available"})),
            None
        );
        assert_eq!(
            blocking_code_for_runtime_detect(&json!({
                "status":"blocked_auth",
                "error_kind":"blocked_auth",
                "exit_code": 4
            })),
            Some(StableExitCode::BlockedAuth)
        );
        assert_eq!(
            blocking_code_for_runtime_detect(&json!({
                "status":"quota_exceeded",
                "error_kind":"quota_exceeded"
            })),
            Some(StableExitCode::RuntimeFailed)
        );
    }

    #[test]
    fn distributed_boundary_uses_the_protocol_crate() {
        assert_eq!(PROTOCOL_CRATE_NAME, "pinvou-protocol");
    }

    fn runtime_event(
        turn_id: &str,
        seq: u64,
        stream_id: &str,
        rate_class: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope::from_value(json!({
            "protocol_version": pinvou_protocol::IPC_VERSION,
            "schema_version": 1,
            "node_id": "node-a",
            "logical_session_id": "session-a",
            "attachment_id": "attachment-a",
            "work_id": null,
            "collaborative_run_id": null,
            "stream_id": stream_id,
            "turn_id": turn_id,
            "seq": seq,
            "source_span": null,
            "timestamp": "2026-08-21T00:00:00Z",
            "rate_class": rate_class,
            "kind": kind,
            "payload": payload
        }))
        .unwrap()
    }

    #[derive(Default)]
    struct TestDuplex {
        inbound: std::io::Cursor<Vec<u8>>,
        outbound: Vec<u8>,
    }

    impl TestDuplex {
        fn with_responses(responses: impl IntoIterator<Item = IpcMessage>) -> Self {
            Self {
                inbound: std::io::Cursor::new(
                    responses
                        .into_iter()
                        .flat_map(|message| encode_frame(&message).unwrap())
                        .collect(),
                ),
                outbound: Vec::new(),
            }
        }

        fn requests(&self) -> Vec<IpcMessage> {
            let mut bytes = self.outbound.as_slice();
            let mut requests = Vec::new();
            while !bytes.is_empty() {
                let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                requests.push(pinvou_protocol::decode_frame(&bytes[..4 + len]).unwrap());
                bytes = &bytes[4 + len..];
            }
            requests
        }
    }

    impl Read for TestDuplex {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.inbound.read(buffer)
        }
    }

    impl Write for TestDuplex {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.outbound.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
