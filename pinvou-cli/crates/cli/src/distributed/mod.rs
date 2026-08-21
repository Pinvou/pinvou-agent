//! Feature-gated terminal client for the stage-one Controller IPC contract.

use std::fmt;
use std::io::{self, IsTerminal, Read, Write};
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
    Chat,
    RuntimeDetect,
}

pub(crate) fn parse_command(values: &[String]) -> Result<Option<DistributedCommand>, CliError> {
    match values
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["chat"] => Ok(Some(DistributedCommand::Chat)),
        ["runtime", "detect"] => Ok(Some(DistributedCommand::RuntimeDetect)),
        ["chat", ..] => Err(CliError::usage("usage: pinvou chat")),
        ["runtime", ..] => Err(CliError::usage("usage: pinvou runtime detect")),
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
    AskApproval { approval_id: String, prompt: String },
    AskInput { input_id: String, prompt: String },
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
            RuntimeEventKind::ErrorRaised => Ok(ProjectionAction::TurnEnded(
                stable_code_for_runtime_error(&required_string(&payload, "code")?),
            )),
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
    pub fn runtime_detect(&mut self) -> Result<IpcMessage, DistributedError> {
        self.request("runtime.detect", json!({}))
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
    DetachedLaunch::for_platform(
        HostPlatform::current()
            .map_err(|_| DistributedError::controller("controller platform is unsupported"))?,
    )
    .spawn(&executable, &[])
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
        DistributedCommand::Chat => execute_chat(),
        DistributedCommand::RuntimeDetect => {
            let response = ensure_controller()?.runtime_detect()?;
            Ok(match output {
                OutputMode::Json => response.payload().to_string(),
                OutputMode::Human => {
                    serde_json::to_string_pretty(response.payload()).unwrap_or_else(|_| "{}".into())
                }
            })
        }
    }
}

fn execute_chat() -> Result<String, DistributedError> {
    require_interactive_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;
    write_terminal("You: ")?;
    let mut prompt = String::new();
    io::stdin()
        .read_line(&mut prompt)
        .map_err(|_| DistributedError::new(StableExitCode::Internal, "terminal read failed"))?;
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(DistributedError::new(
            StableExitCode::Usage,
            "chat prompt must not be empty",
        ));
    }

    let interrupted = Arc::new(AtomicBool::new(false));
    let active_turn = Arc::new(Mutex::new(None::<String>));
    install_interrupt_handler(Arc::clone(&interrupted), Arc::clone(&active_turn))?;
    let mut client = ensure_controller()?;
    let first = client.chat_start(prompt)?;
    let mut projection = TerminalProjection::new(Instant::now());
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
        let event = RuntimeEventEnvelope::from_value(message.payload().clone()).map_err(|_| {
            DistributedError::runtime("controller returned an invalid runtime event")
        })?;
        if let Some(turn_id) = event.turn_id()
            && let Ok(mut active) = active_turn.lock()
        {
            *active = Some(turn_id.to_owned());
        }
        match projection.push(&event, Instant::now())? {
            ProjectionAction::None => {}
            ProjectionAction::WriteText(text) => write_terminal(&text)?,
            ProjectionAction::AskApproval {
                approval_id,
                prompt,
            } => {
                write_terminal(&prompt)?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer).map_err(|_| {
                    DistributedError::new(StableExitCode::Internal, "terminal read failed")
                })?;
                let accepted = projection.parse_approval(&answer).ok_or_else(|| {
                    DistributedError::new(StableExitCode::Usage, "approval requires y or n")
                })?;
                client.resolve_approval(&approval_id, accepted)?;
            }
            ProjectionAction::AskInput { input_id, prompt } => {
                write_terminal(&format!("{prompt} "))?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer).map_err(|_| {
                    DistributedError::new(StableExitCode::Internal, "terminal read failed")
                })?;
                client.resolve_input(&input_id, answer.trim_end())?;
            }
            ProjectionAction::TurnEnded(code) => {
                if let Some(text) = projection.flush_pending() {
                    write_terminal(&text)?;
                }
                if code == StableExitCode::Success {
                    return Ok(String::new());
                }
                return Err(DistributedError::new(code, "chat turn did not complete"));
            }
        }
    }
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

fn write_terminal(text: &str) -> Result<(), DistributedError> {
    print!("{text}");
    io::stdout()
        .flush()
        .map_err(|_| DistributedError::new(StableExitCode::Internal, "terminal write failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distributed_boundary_uses_the_protocol_crate() {
        assert_eq!(PROTOCOL_CRATE_NAME, "pinvou-protocol");
    }
}
