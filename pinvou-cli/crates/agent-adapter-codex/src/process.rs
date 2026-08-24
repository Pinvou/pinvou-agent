use std::{
    collections::HashMap,
    ffi::OsString,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pinvou_protocol::RuntimeEventEnvelope;
use pinvou_runtime_api::{
    AdapterError, AgentRuntimeAdapter, AuthStatus, NegotiatedCapabilities, RuntimeCapabilities,
    RuntimeCommand, RuntimeEventSubscription, RuntimeOperation, RuntimeSession,
};
use serde_json::{Value, json};

use crate::{
    ApprovalResponse, CodexEventProjector, MAX_JSON_LINE_BYTES, PendingControl, ProjectedFrame,
    redact_diagnostic,
};

static ATTACHMENT_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExecutableIdentity {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}
impl ExecutableIdentity {
    fn resolve(requested: &std::path::Path) -> Result<Self, AdapterError> {
        let path = if requested.components().count() > 1 {
            requested.to_owned()
        } else {
            find_on_path(requested).ok_or_else(|| AdapterError::ProcessExit {
                code: None,
                signal: None,
                unexpected_eof: false,
                details: "Codex executable was not found on PATH".into(),
            })?
        };
        let path = path
            .canonicalize()
            .map_err(process_io("canonicalize executable"))?;
        let metadata = std::fs::metadata(&path).map_err(process_io("inspect executable"))?;
        if !metadata.is_file() {
            return Err(AdapterError::InvalidRequest {
                details: "Codex executable is not a regular file".into(),
            });
        }
        Ok(Self {
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
    fn verify(&self, path: &std::path::Path) -> Result<(), AdapterError> {
        let current = Self::resolve(path)?;
        if &current != self {
            return Err(AdapterError::ProcessExit {
                code: None,
                signal: None,
                unexpected_eof: false,
                details: "Codex executable identity changed after probe".into(),
            });
        }
        Ok(())
    }
}
fn find_on_path(name: &std::path::Path) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let extensions: Vec<OsString> = std::env::var_os("PATHEXT")
        .unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into())
        .to_string_lossy()
        .split(';')
        .map(OsString::from)
        .collect();
    #[cfg(not(windows))]
    let extensions: Vec<OsString> = vec![OsString::new()];
    for directory in std::env::split_paths(&path) {
        #[cfg(windows)]
        if name.extension().is_none() {
            for extension in &extensions {
                let candidate = directory.join(format!(
                    "{}{}",
                    name.to_string_lossy(),
                    extension.to_string_lossy()
                ));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        let direct = directory.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        #[cfg(windows)]
        for extension in &extensions {
            let candidate = directory.join(format!(
                "{}{}",
                name.to_string_lossy(),
                extension.to_string_lossy()
            ));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct CodexAdapterConfig {
    pub executable: PathBuf,
    pub app_server_args: Vec<OsString>,
    pub version_args: Vec<OsString>,
    pub doctor_args: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub handshake_timeout: Duration,
    pub request_timeout: Duration,
    pub node_id: String,
    pub attachment_id: String,
}

impl Default for CodexAdapterConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("codex"),
            app_server_args: vec![
                "app-server".into(),
                "--disable".into(),
                "hooks".into(),
                "--disable".into(),
                "plugins".into(),
                "--disable".into(),
                "apps".into(),
                "--disable".into(),
                "shell_snapshot".into(),
                "-c".into(),
                "notify=[]".into(),
            ],
            version_args: vec!["--version".into()],
            doctor_args: vec!["doctor".into()],
            working_directory: None,
            handshake_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(30),
            node_id: "local-node".into(),
            attachment_id: format!(
                "codex-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                ATTACHMENT_NONCE.fetch_add(1, Ordering::Relaxed)
            ),
        }
    }
}

pub struct CodexAdapter {
    config: CodexAdapterConfig,
    negotiated: NegotiatedCapabilities,
    connection: Option<Connection>,
    event_rx: Option<mpsc::Receiver<Result<RuntimeEventEnvelope, AdapterError>>>,
    sessions: Arc<Mutex<HashMap<String, Option<String>>>>,
    executable_identity: Option<ExecutableIdentity>,
    auth_blocked: Arc<AtomicBool>,
    default_model: Option<String>,
}

impl CodexAdapter {
    pub fn new(config: CodexAdapterConfig) -> Self {
        Self {
            config,
            negotiated: NegotiatedCapabilities::default(),
            connection: None,
            event_rx: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            executable_identity: None,
            auth_blocked: Arc::new(AtomicBool::new(false)),
            default_model: None,
        }
    }

    fn ensure_connection(&mut self) -> Result<&Connection, AdapterError> {
        if let Some(identity) = self.executable_identity.as_ref() {
            identity.verify(&self.config.executable)?;
        }
        if self.connection.is_none() {
            let (connection, receiver) = Connection::spawn(
                &self.config,
                Arc::clone(&self.auth_blocked),
                Arc::clone(&self.sessions),
            )?;
            connection.request_with_timeout("initialize", json!({"clientInfo":{"name":"pinvou-agent-adapter-codex","version":env!("CARGO_PKG_VERSION")}}), self.config.handshake_timeout, true)?;
            connection.notify("initialized", json!({}))?;
            self.connection = Some(connection);
            self.event_rx = Some(receiver);
        }
        Ok(self.connection.as_ref().expect("connection initialized"))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, AdapterError> {
        let timeout = self.config.request_timeout;
        let result = self
            .ensure_connection()?
            .request_with_timeout(method, params, timeout, false);
        if matches!(
            &result,
            Err(AdapterError::Protocol {
                code: Some(-32601),
                ..
            })
        ) {
            let operation = match method {
                "thread/resume" => Some("resume"),
                "thread/inject_items" => Some("import_context"),
                "turn/steer" => Some("steer"),
                _ => None,
            };
            if let Some(operation) = operation {
                let _ = self.negotiated.method_not_found(operation);
            }
        }
        result
    }

    fn session_turn(&self, session: &RuntimeSession) -> Result<Option<String>, AdapterError> {
        self.sessions
            .lock()
            .map_err(lock_error)?
            .get(session.as_str())
            .cloned()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "unknown runtime session".into(),
            })
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new(CodexAdapterConfig::default())
    }
}

impl AgentRuntimeAdapter for CodexAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        let identity = ExecutableIdentity::resolve(&self.config.executable)?;
        self.config.executable = identity.path.clone();
        self.executable_identity = Some(identity);
        let output = run_version_probe(&self.config)?;
        let version = parse_version(&output).ok_or_else(|| AdapterError::Protocol {
            code: None,
            method: Some("codex --version".into()),
            details: "could not parse Codex version".into(),
        })?;
        if version < (0, 139, 0) || version >= (0, 150, 0) {
            return Err(AdapterError::Unsupported {
                operation: "codex_version",
            });
        }
        run_diagnostic_doctor_probe(&self.config);
        self.ensure_connection()?;
        self.request("account/read", json!({"refreshToken":false}))?;
        let models = self.request("model/list", json!({"limit":1}))?;
        self.default_model = parse_default_model(&models);
        self.negotiated.complete(RuntimeCapabilities {
            interactive_chat: true,
            native_resume: true,
            history_import: false,
            tool_approval: true,
            elicitation: false,
            steering: false,
            image_input: false,
            file_reference: false,
            session_modes: vec!["interactive".into()],
            config_options: vec!["model".into(), "effort".into()],
            auth_flows: vec![
                "existing_credential".into(),
                "browser_url".into(),
                "local_interactive".into(),
            ],
        });
        Ok(())
    }

    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        self.negotiated.snapshot()
    }

    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        if self.auth_blocked.load(Ordering::Acquire) {
            return Ok(AuthStatus::Blocked);
        }
        let account = self.request("account/read", json!({"refreshToken":false}))?;
        let required = account
            .get("requiresOpenaiAuth")
            .and_then(Value::as_bool)
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: Some("account/read".into()),
                details: "missing requiresOpenaiAuth".into(),
            })?;
        let account_value = account
            .get("account")
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: Some("account/read".into()),
                details: "missing account field".into(),
            })?;
        if required && account_value.is_null() {
            return Ok(AuthStatus::Blocked);
        }
        if required {
            let limits = self.request("account/rateLimits/read", json!({}))?;
            if limits
                .pointer("/rateLimits/primary/usedPercent")
                .and_then(Value::as_f64)
                .is_some_and(|used| used >= 100.0)
                || !limits
                    .pointer("/rateLimits/rateLimitReachedType")
                    .is_none_or(Value::is_null)
            {
                return Err(AdapterError::QuotaExceeded);
            }
            Ok(AuthStatus::Authenticated)
        } else {
            Ok(AuthStatus::NotRequired)
        }
    }

    fn start_auth(&mut self, operation: RuntimeOperation) -> Result<(), AdapterError> {
        self.request("account/login/start", operation.options)
            .map(|_| ())
    }

    fn create(&mut self, operation: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        let options =
            operation
                .options
                .as_object()
                .ok_or_else(|| AdapterError::InvalidRequest {
                    details: "create options must be an object".into(),
                })?;
        if options
            .keys()
            .any(|key| !matches!(key.as_str(), "cwd" | "model"))
        {
            return Err(AdapterError::InvalidRequest {
                details: "create accepts only cwd and model".into(),
            });
        }
        let cwd = options
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| self.config.working_directory.clone())
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "working directory is unavailable".into(),
            })?;
        let cwd = cwd
            .canonicalize()
            .map_err(|_| AdapterError::InvalidRequest {
                details: "working directory must exist".into(),
            })?;
        let mut params = json!({"cwd":cwd,"approvalPolicy":"on-request","sandbox":"workspace-write","ephemeral":false});
        if let Some(model) = options
            .get("model")
            .and_then(Value::as_str)
            .or(self.default_model.as_deref())
        {
            if model.is_empty() {
                return Err(AdapterError::InvalidRequest {
                    details: "model is empty".into(),
                });
            }
            params["model"] = json!(model);
        }
        let result = self.request("thread/start", params)?;
        let id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: Some("thread/start".into()),
                details: "missing thread id".into(),
            })?
            .to_owned();
        let session = RuntimeSession::new(id.clone())?;
        self.sessions.lock().map_err(lock_error)?.insert(id, None);
        Ok(session)
    }

    fn resume(&mut self, operation: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        let thread_id = operation
            .options
            .get("thread_id")
            .and_then(Value::as_str)
            .unwrap_or(&operation.operation_id)
            .to_owned();
        let result = self.request("thread/resume", json!({"threadId":thread_id}))?;
        let id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .unwrap_or(&thread_id)
            .to_owned();
        let session = RuntimeSession::new(id.clone())?;
        self.sessions.lock().map_err(lock_error)?.insert(id, None);
        Ok(session)
    }

    fn import_context(
        &mut self,
        session: &RuntimeSession,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.session_turn(session)?;
        self.request("thread/inject_items", json!({"threadId":session.as_str(),"items":operation.options.get("items").cloned().unwrap_or_else(||json!([]))})).map(|_| ())
    }

    fn send(
        &mut self,
        session: &RuntimeSession,
        command: RuntimeCommand,
    ) -> Result<(), AdapterError> {
        self.session_turn(session)?;
        if command.kind != "text" {
            return Err(AdapterError::InvalidRequest {
                details: "Codex only accepts text commands in stage one".into(),
            });
        }
        let text = command
            .payload
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "Codex text command must be a string".into(),
            })?;
        let result = self.request(
            "turn/start",
            json!({"threadId":session.as_str(),"input":[{"type":"text","text":text}],"approvalPolicy":"on-request","sandboxPolicy":{"type":"workspaceWrite","networkAccess":false}}),
        )?;
        let turn = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: Some("turn/start".into()),
                details: "missing turn id".into(),
            })?
            .to_owned();
        self.sessions
            .lock()
            .map_err(lock_error)?
            .insert(session.as_str().into(), Some(turn));
        Ok(())
    }

    fn approve(
        &mut self,
        session: &RuntimeSession,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.session_turn(session)?;
        let decision = operation
            .options
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or("deny");
        let decision = if matches!(decision, "allow" | "accept" | "approved") {
            "accept"
        } else {
            "decline"
        };
        self.ensure_connection()?.resolve_approval(
            session.as_str(),
            &operation.operation_id,
            decision,
            operation.options.get("scope").and_then(Value::as_str),
        )
    }

    fn respond_input(
        &mut self,
        session: &RuntimeSession,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.session_turn(session)?;
        let value = operation
            .options
            .get("value")
            .cloned()
            .unwrap_or(Value::Null);
        self.ensure_connection()?
            .resolve_input(session.as_str(), &operation.operation_id, &value)
    }

    fn steer(
        &mut self,
        session: &RuntimeSession,
        command: RuntimeCommand,
    ) -> Result<(), AdapterError> {
        let turn = self
            .session_turn(session)?
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "session has no active turn".into(),
            })?
            .to_owned();
        let text = command
            .payload
            .as_str()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "Codex text command must be a string".into(),
            })?;
        self.request("turn/steer", json!({"threadId":session.as_str(),"turnId":turn,"input":[{"type":"text","text":text}]})).map(|_| ())
    }

    fn interrupt(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        let turn = self
            .session_turn(session)?
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "session has no active turn".into(),
            })?
            .to_owned();
        self.request(
            "turn/interrupt",
            json!({"threadId":session.as_str(),"turnId":turn}),
        )
        .map(|_| ())
    }

    fn subscribe_events(
        &mut self,
        session: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError> {
        self.session_turn(session)?;
        self.ensure_connection()?;
        let receiver = self
            .event_rx
            .take()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "event stream is already subscribed".into(),
            })?;
        Ok(Box::new(receiver.into_iter()))
    }

    fn close(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        self.sessions
            .lock()
            .map_err(lock_error)?
            .remove(session.as_str());
        if let Some(connection) = self.connection.as_ref() {
            connection.clear_session_controls(session.as_str())?;
        }
        if self.sessions.lock().map_err(lock_error)?.is_empty() {
            if let Some(mut connection) = self.connection.take() {
                connection.close()?;
            }
        }
        Ok(())
    }
}

impl Drop for CodexAdapter {
    fn drop(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            let _ = connection.close();
        }
    }
}

struct Connection {
    process: Option<Arc<Mutex<ManagedProcess>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, AdapterError>>>>>,
    controls: Arc<Mutex<HashMap<String, PendingControl>>>,
    projector: Arc<Mutex<CodexEventProjector>>,
    event_tx: mpsc::SyncSender<Result<RuntimeEventEnvelope, AdapterError>>,
    next_id: AtomicU64,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
}

impl Connection {
    fn spawn(
        config: &CodexAdapterConfig,
        auth_blocked: Arc<AtomicBool>,
        sessions: Arc<Mutex<HashMap<String, Option<String>>>>,
    ) -> Result<
        (
            Self,
            mpsc::Receiver<Result<RuntimeEventEnvelope, AdapterError>>,
        ),
        AdapterError,
    > {
        let mut command = Command::new(&config.executable);
        command
            .args(&config.app_server_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &config.working_directory {
            command.current_dir(cwd);
        }
        configure_child(&mut command)?;
        let mut child = command.spawn().map_err(process_io("spawn"))?;
        let writer = Arc::new(Mutex::new(BufWriter::new(
            child
                .stdin
                .take()
                .ok_or_else(|| process_error("missing child stdin"))?,
        )));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| process_error("missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| process_error("missing child stderr"))?;
        let process = Arc::new(Mutex::new(ManagedProcess::new(child)?));
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, AdapterError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let controls = Arc::new(Mutex::new(HashMap::new()));
        let projector = Arc::new(Mutex::new(CodexEventProjector::new(
            &config.node_id,
            &config.attachment_id,
        )));
        let (event_tx, event_rx) = mpsc::sync_channel(1024);
        let stdout_thread = {
            let pending = Arc::clone(&pending);
            let controls = Arc::clone(&controls);
            let projector = Arc::clone(&projector);
            let event_tx = event_tx.clone();
            let writer = Arc::clone(&writer);
            let process = Arc::clone(&process);
            thread::spawn(move || {
                stdout_loop(
                    stdout,
                    pending,
                    controls,
                    projector,
                    writer,
                    process,
                    auth_blocked,
                    sessions,
                    event_tx,
                )
            })
        };
        let stderr_thread = {
            let projector = Arc::clone(&projector);
            let stderr_event_tx = event_tx.clone();
            thread::spawn(move || stderr_loop(stderr, projector, stderr_event_tx))
        };
        Ok((
            Self {
                process: Some(process),
                writer,
                pending,
                controls,
                projector,
                event_tx,
                next_id: AtomicU64::new(1),
                stdout_thread: Some(stdout_thread),
                stderr_thread: Some(stderr_thread),
            },
            event_rx,
        ))
    }

    fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        handshake: bool,
    ) -> Result<Value, AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        let mut pending = self.pending.lock().map_err(lock_error)?;
        if pending.len() >= 256 {
            return Err(AdapterError::Protocol {
                code: None,
                method: Some(method.into()),
                details: "too many pending runtime requests".into(),
            });
        }
        pending.insert(id, tx);
        drop(pending);
        if let Err(error) = self.write_json(&json!({"id":id,"method":method,"params":params})) {
            self.pending.lock().map_err(lock_error)?.remove(&id);
            return Err(error);
        }
        match rx.recv_timeout(timeout) {
            Ok(Err(AdapterError::Protocol {
                code,
                method: None,
                details,
            })) => Err(AdapterError::Protocol {
                code,
                method: Some(method.into()),
                details,
            }),
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) if handshake => {
                self.pending.lock().map_err(lock_error)?.remove(&id);
                if let Some(process) = self.process.as_ref() {
                    if let Ok(mut process) = process.lock() {
                        let _ = process.terminate_tree();
                    }
                }
                Err(AdapterError::HandshakeTimeout)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().map_err(lock_error)?.remove(&id);
                if let Some(process) = self.process.as_ref() {
                    if let Ok(mut process) = process.lock() {
                        let _ = process.terminate_tree();
                    }
                }
                Err(AdapterError::ProcessExit {
                    code: None,
                    signal: None,
                    unexpected_eof: false,
                    details: format!(
                        "{method} timed out with unknown outcome; attachment terminated"
                    ),
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(process_error("response reader stopped"))
            }
        }
    }
    fn notify(&self, method: &str, params: Value) -> Result<(), AdapterError> {
        self.write_json(&json!({"method":method,"params":params}))
    }
    fn control(&self, id: &str, session: &str) -> Result<PendingControl, AdapterError> {
        let control = self
            .controls
            .lock()
            .map_err(lock_error)?
            .get(id)
            .cloned()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "unknown or already resolved control request".into(),
            })?;
        let thread = match &control {
            PendingControl::Approval { thread_id, .. }
            | PendingControl::Input { thread_id, .. } => Some(thread_id.as_str()),
            PendingControl::AuthRefresh { .. } => None,
        };
        if thread.is_some_and(|thread| thread != session) {
            return Err(AdapterError::InvalidRequest {
                details: "control request belongs to another session".into(),
            });
        }
        Ok(control)
    }
    fn resolve_approval(
        &self,
        session: &str,
        id: &str,
        decision: &str,
        scope: Option<&str>,
    ) -> Result<(), AdapterError> {
        let control = self.control(id, session)?;
        let PendingControl::Approval {
            request_id,
            approval_id,
            response,
            ..
        } = control
        else {
            return Err(AdapterError::InvalidRequest {
                details: "control request is not an approval".into(),
            });
        };
        let accepted = decision == "accept";
        let result = match response {
            ApprovalResponse::Decision => json!({"decision":decision}),
            ApprovalResponse::Permissions { requested } => {
                json!({"permissions":if accepted{requested}else{json!({})},"scope":match scope{Some("session")=>"session",_=>"turn"}})
            }
        };
        self.write_json(&json!({"id":request_id,"result":result}))?;
        self.controls.lock().map_err(lock_error)?.remove(id);
        let projected = self
            .projector
            .lock()
            .map_err(lock_error)?
            .approval_resolved(&approval_id, if accepted { "approved" } else { "denied" });
        self.event_tx
            .send(projected)
            .map_err(|_| process_error("event reader stopped"))
    }
    fn resolve_input(&self, session: &str, id: &str, value: &Value) -> Result<(), AdapterError> {
        let control = self.control(id, session)?;
        let PendingControl::Input {
            request_id,
            input_id,
            questions,
            ..
        } = control
        else {
            return Err(AdapterError::InvalidRequest {
                details: "control request is not an input request".into(),
            });
        };
        let supplied = value
            .as_object()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "input value must map question ids to answers".into(),
            })?;
        let mut answers = serde_json::Map::new();
        for question in questions.as_array().ok_or_else(|| AdapterError::Protocol {
            code: None,
            method: Some("item/tool/requestUserInput".into()),
            details: "questions is not an array".into(),
        })? {
            let question_id = question.get("id").and_then(Value::as_str).ok_or_else(|| {
                AdapterError::Protocol {
                    code: None,
                    method: Some("item/tool/requestUserInput".into()),
                    details: "question has no id".into(),
                }
            })?;
            let raw = supplied
                .get(question_id)
                .ok_or_else(|| AdapterError::InvalidRequest {
                    details: format!("missing answer for question {question_id}"),
                })?;
            let values = raw.as_array().cloned().unwrap_or_else(|| vec![raw.clone()]);
            if !values.iter().all(Value::is_string) {
                return Err(AdapterError::InvalidRequest {
                    details: "input answers must be strings".into(),
                });
            }
            answers.insert(question_id.into(), json!({"answers":values}));
        }
        let result = json!({"answers":answers});
        self.write_json(&json!({"id":request_id,"result":result}))?;
        self.controls.lock().map_err(lock_error)?.remove(id);
        let projected = self
            .projector
            .lock()
            .map_err(lock_error)?
            .input_resolved(&input_id, value);
        self.event_tx
            .send(projected)
            .map_err(|_| process_error("event reader stopped"))
    }
    fn clear_session_controls(&self, session: &str) -> Result<(), AdapterError> {
        self.controls
            .lock()
            .map_err(lock_error)?
            .retain(|_, control| match control {
                PendingControl::Approval { thread_id, .. }
                | PendingControl::Input { thread_id, .. } => thread_id != session,
                PendingControl::AuthRefresh { .. } => true,
            });
        Ok(())
    }
    fn _obsolete_resolve_control(&self, id: &str, result: Value) -> Result<(), AdapterError> {
        let control = self
            .controls
            .lock()
            .map_err(lock_error)?
            .remove(id)
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "unknown or already resolved control request".into(),
            })?;
        let request_id = match &control {
            PendingControl::Approval { request_id, .. }
            | PendingControl::Input { request_id, .. }
            | PendingControl::AuthRefresh { request_id } => request_id.clone(),
        };
        self.write_json(&json!({"id":request_id,"result":result}))?;
        let projected = match control {
            PendingControl::Approval { approval_id, .. } => {
                let accepted = result.get("decision").and_then(Value::as_str) == Some("accept");
                self.projector
                    .lock()
                    .map_err(lock_error)?
                    .approval_resolved(&approval_id, if accepted { "approved" } else { "denied" })
            }
            PendingControl::Input { input_id, .. } => self
                .projector
                .lock()
                .map_err(lock_error)?
                .input_resolved(&input_id, result.get("answers").unwrap_or(&Value::Null)),
            PendingControl::AuthRefresh { .. } => return Ok(()),
        };
        self.event_tx
            .send(projected)
            .map_err(|_| process_error("event reader stopped"))
    }
    fn write_json(&self, value: &Value) -> Result<(), AdapterError> {
        let bytes = serde_json::to_vec(value).map_err(protocol_json)?;
        if bytes.len() + 1 > MAX_JSON_LINE_BYTES {
            return Err(AdapterError::InvalidRequest {
                details: "JSON-RPC frame exceeds 16 MiB".into(),
            });
        }
        let mut writer = self.writer.lock().map_err(lock_error)?;
        writer
            .write_all(&bytes)
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| writer.flush())
            .map_err(process_io("write"))
    }
    fn close(&mut self) -> Result<(), AdapterError> {
        drop(self.writer.lock().map_err(lock_error)?);
        let result: Result<(), AdapterError> = if let Some(process) = self.process.as_ref() {
            process
                .lock()
                .map_err(lock_error)?
                .terminate_tree()
                .map_err(process_io("terminate process tree"))
        } else {
            Ok(())
        };
        self.process.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        for handle in [&mut self.stdout_thread, &mut self.stderr_thread] {
            while handle.as_ref().is_some_and(|handle| !handle.is_finished())
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
            if handle.as_ref().is_some_and(|handle| handle.is_finished()) {
                if let Some(handle) = handle.take() {
                    let _ = handle.join();
                }
            }
        }
        if self.stdout_thread.is_some() || self.stderr_thread.is_some() {
            return Err(AdapterError::ProcessExit {
                code: None,
                signal: None,
                unexpected_eof: true,
                details: "runtime readers did not close before deadline".into(),
            });
        }
        result
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.process.is_some() {
            let _ = self.close();
        }
    }
}

fn stdout_loop(
    stdout: impl std::io::Read,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, AdapterError>>>>>,
    controls: Arc<Mutex<HashMap<String, PendingControl>>>,
    projector: Arc<Mutex<CodexEventProjector>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    process: Arc<Mutex<ManagedProcess>>,
    auth_blocked: Arc<AtomicBool>,
    sessions: Arc<Mutex<HashMap<String, Option<String>>>>,
    events: mpsc::SyncSender<Result<RuntimeEventEnvelope, AdapterError>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let line = match read_capped_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => {
                if let Ok(mut projector) = projector.lock() {
                    if let Ok(event) = projector.attachment_failed("unexpected stdout EOF") {
                        let _ = events.try_send(Ok(event));
                    }
                }
                let _ = events.try_send(Err(process_error("unexpected stdout EOF")));
                if let Ok(mut process) = process.lock() {
                    let _ = process.terminate_tree();
                }
                break;
            }
            Err(error) => {
                if let Ok(mut projector) = projector.lock() {
                    if let Ok(event) = projector.attachment_failed(&error.to_string()) {
                        let _ = events.try_send(Ok(event));
                    }
                }
                let _ = events.try_send(Err(error));
                if let Ok(mut process) = process.lock() {
                    let _ = process.terminate_tree();
                }
                break;
            }
        };
        let frame: Value = match serde_json::from_slice(&line) {
            Ok(frame) => frame,
            Err(error) => {
                if let Ok(mut projector) = projector.lock() {
                    if let Ok(event) = projector.attachment_failed("invalid JSON-RPC frame") {
                        let _ = events.try_send(Ok(event));
                    }
                }
                let _ = events.try_send(Err(AdapterError::Protocol {
                    code: None,
                    method: None,
                    details: format!("invalid JSON-RPC frame: {error}"),
                }));
                if let Ok(mut process) = process.lock() {
                    let _ = process.terminate_tree();
                }
                break;
            }
        };
        if frame.get("method").and_then(Value::as_str) == Some("account/updated") {
            let blocked = frame.pointer("/params/account").is_none_or(Value::is_null);
            auth_blocked.store(blocked, Ordering::Release);
        }
        if frame.get("method").and_then(Value::as_str) == Some("turn/completed") {
            if let Some(thread_id) = frame.pointer("/params/threadId").and_then(Value::as_str) {
                if let Ok(mut sessions) = sessions.lock() {
                    if let Some(turn) = sessions.get_mut(thread_id) {
                        *turn = None;
                    }
                }
            }
        }
        if frame.get("method").is_none() {
            if let Some(id) = frame.get("id").and_then(Value::as_u64) {
                if let Ok(mut map) = pending.lock() {
                    if let Some(sender) = map.remove(&id) {
                        let _ = sender.send(decode_response(&frame));
                    }
                }
            }
            continue;
        }
        let projected = projector
            .lock()
            .map_err(lock_error)
            .and_then(|mut projector| {
                let result = projector.project(&frame);
                while let Some(control) = projector.take_pending_control() {
                    let id = match &control {
                        PendingControl::Approval { approval_id, .. } => approval_id,
                        PendingControl::Input { input_id, .. } => input_id,
                        PendingControl::AuthRefresh { .. } => "auth-refresh",
                    };
                    let mut controls = controls.lock().map_err(lock_error)?;
                    if controls.len() >= 256 || controls.contains_key(id) {
                        return Err(AdapterError::Protocol {
                            code: None,
                            method: Some("server/control".into()),
                            details: "control request queue is full or duplicated".into(),
                        });
                    }
                    controls.insert(id.to_owned(), control);
                }
                result
            });
        match projected {
            Ok(ProjectedFrame::Event(event)) => {
                if events.try_send(Ok(event)).is_err() {
                    if let Ok(mut process) = process.lock() {
                        let _ = process.terminate_tree();
                    }
                    break;
                }
            }
            Ok(ProjectedFrame::Control(PendingControl::AuthRefresh { request_id })) => {
                let _ = write_shared(
                    &writer,
                    &json!({"id":request_id,"error":{"code":-32001,"message":"credential refresh is unavailable; Codex must reload its credential store"}}),
                );
            }
            Ok(_) => {}
            Err(error) => {
                let fatal = true;
                if fatal {
                    if let Some(request_id) = frame.get("id") {
                        let _ = write_shared(
                            &writer,
                            &json!({"id":request_id,"error":{"code":-32002,"message":"unsupported_control_event"}}),
                        );
                    }
                }
                let _ = events.try_send(Err(error));
                if fatal {
                    if let Ok(mut projector) = projector.lock() {
                        if let Ok(event) =
                            projector.attachment_failed("unsupported Codex protocol event")
                        {
                            let _ = events.try_send(Ok(event));
                        }
                    }
                    if let Ok(mut process) = process.lock() {
                        let _ = process.terminate_tree();
                    }
                    break;
                }
            }
        }
    }
    if let Ok(mut map) = pending.lock() {
        for (_, sender) in map.drain() {
            let _ = sender.send(Err(process_error("stdout reader stopped")));
        }
    }
}

fn stderr_loop(
    stderr: impl std::io::Read,
    projector: Arc<Mutex<CodexEventProjector>>,
    events: mpsc::SyncSender<Result<RuntimeEventEnvelope, AdapterError>>,
) {
    let mut reader = BufReader::new(stderr);
    loop {
        match read_capped_line(&mut reader) {
            Ok(Some(line)) => {
                let message = redact_diagnostic(&String::from_utf8_lossy(&line));
                let frame = json!({"method":"warning","params":{"message":message}});
                if let Ok(mut projector) = projector.lock()
                    && let Ok(ProjectedFrame::Event(event)) = projector.project(&frame)
                {
                    let _ = events.try_send(Ok(event));
                }
            }
            Err(error) => {
                let _ = events.try_send(Err(error));
            }
            Ok(None) => break,
        }
    }
}

fn read_capped_line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, AdapterError> {
    let mut output = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(process_io("read"))?;
        if available.is_empty() {
            return if output.is_empty() {
                Ok(None)
            } else {
                Ok(Some(output))
            };
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if output.len() + take > MAX_JSON_LINE_BYTES {
            let ended = available.get(take.saturating_sub(1)) == Some(&b'\n');
            reader.consume(take);
            if !ended {
                drain_to_newline(reader)?;
            }
            return Err(AdapterError::Protocol {
                code: None,
                method: None,
                details: "JSON-RPC frame exceeds 16 MiB".into(),
            });
        }
        output.extend_from_slice(&available[..take]);
        reader.consume(take);
        if output.last() == Some(&b'\n') {
            output.pop();
            if output.last() == Some(&b'\r') {
                output.pop();
            }
            return Ok(Some(output));
        }
    }
}

fn drain_to_newline(reader: &mut impl BufRead) -> Result<(), AdapterError> {
    loop {
        let chunk = reader.fill_buf().map_err(process_io("read"))?;
        if chunk.is_empty() {
            return Ok(());
        }
        let take = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(chunk.len(), |position| position + 1);
        let ended = chunk.get(take.saturating_sub(1)) == Some(&b'\n');
        reader.consume(take);
        if ended {
            return Ok(());
        }
    }
}

fn write_shared(
    writer: &Arc<Mutex<BufWriter<ChildStdin>>>,
    value: &Value,
) -> Result<(), AdapterError> {
    let bytes = serde_json::to_vec(value).map_err(protocol_json)?;
    if bytes.len() + 1 > MAX_JSON_LINE_BYTES {
        return Err(AdapterError::InvalidRequest {
            details: "JSON-RPC frame exceeds 16 MiB".into(),
        });
    }
    let mut writer = writer.lock().map_err(lock_error)?;
    writer
        .write_all(&bytes)
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(process_io("write"))
}

fn decode_response(frame: &Value) -> Result<Value, AdapterError> {
    if let Some(error) = frame.get("error") {
        let code = error.get("code").and_then(Value::as_i64);
        let message = redact_diagnostic(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex protocol error"),
        );
        if message.to_ascii_lowercase().contains("usage limit") {
            Err(AdapterError::QuotaExceeded)
        } else {
            Err(AdapterError::Protocol {
                code,
                method: None,
                details: message,
            })
        }
    } else {
        frame
            .get("result")
            .cloned()
            .ok_or_else(|| AdapterError::Protocol {
                code: None,
                method: None,
                details: "response has neither result nor error".into(),
            })
    }
}

fn run_version_probe(config: &CodexAdapterConfig) -> Result<String, AdapterError> {
    let mut command = Command::new(&config.executable);
    command
        .args(&config.version_args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped());
    configure_child(&mut command)?;
    let mut child = command.spawn().map_err(process_io("version probe"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| process_error("version probe stdout missing"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| process_error("version probe stderr missing"))?;
    let stdout_thread = thread::spawn(move || read_bounded_output(stdout));
    let stderr_thread = thread::spawn(move || read_bounded_output(stderr));
    let mut process = ManagedProcess::new(child)?;
    let deadline = Instant::now() + config.handshake_timeout;
    let status = loop {
        if let Some(status) = process
            .child
            .try_wait()
            .map_err(process_io("version probe"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = process.terminate_tree();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(AdapterError::HandshakeTimeout);
        }
        thread::sleep(Duration::from_millis(10));
    };
    // Close descendants that inherited the capture pipes before joining readers.
    process
        .terminate_tree()
        .map_err(process_io("version probe cleanup"))?;
    let stdout = stdout_thread
        .join()
        .map_err(|_| process_error("version probe stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| process_error("version probe stderr reader panicked"))??;
    if !status.success() {
        let detail = redact_diagnostic(&String::from_utf8_lossy(&stderr));
        return Err(AdapterError::ProcessExit {
            code: status.code(),
            signal: None,
            unexpected_eof: false,
            details: format!("codex version probe failed: {detail}"),
        });
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn run_doctor_probe(config: &CodexAdapterConfig) -> Result<(), AdapterError> {
    let mut doctor = config.clone();
    doctor.version_args = doctor.doctor_args.clone();
    run_version_probe(&doctor).map(|_| ())
}

fn run_diagnostic_doctor_probe(config: &CodexAdapterConfig) {
    let _ = run_doctor_probe(config);
}

fn read_bounded_output(reader: impl Read) -> Result<Vec<u8>, AdapterError> {
    let mut output = Vec::new();
    reader
        .take((MAX_JSON_LINE_BYTES + 1) as u64)
        .read_to_end(&mut output)
        .map_err(process_io("version probe read"))?;
    if output.len() > MAX_JSON_LINE_BYTES {
        return Err(AdapterError::Protocol {
            code: None,
            method: Some("codex --version".into()),
            details: "version output exceeds 16 MiB".into(),
        });
    }
    Ok(output)
}
fn parse_version(input: &str) -> Option<(u64, u64, u64)> {
    let version = input
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = version.split('.').map(|part| {
        part.trim_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .ok()
    });
    Some((parts.next()??, parts.next()??, parts.next()??))
}

fn parse_default_model(models: &Value) -> Option<String> {
    models
        .get("data")
        .and_then(Value::as_array)?
        .iter()
        .find(|model| model.get("isDefault").and_then(Value::as_bool) == Some(true))
        .or_else(|| models.get("data").and_then(Value::as_array)?.first())?
        .get("id")
        .or_else(|| {
            models
                .get("data")
                .and_then(Value::as_array)?
                .first()?
                .get("model")
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn protocol_json(error: serde_json::Error) -> AdapterError {
    AdapterError::Protocol {
        code: None,
        method: None,
        details: error.to_string(),
    }
}
fn lock_error<T>(_: std::sync::PoisonError<T>) -> AdapterError {
    AdapterError::Protocol {
        code: None,
        method: None,
        details: "adapter synchronization state is poisoned".into(),
    }
}
fn process_error(details: &str) -> AdapterError {
    AdapterError::ProcessExit {
        code: None,
        signal: None,
        unexpected_eof: true,
        details: details.into(),
    }
}
fn process_io(context: &'static str) -> impl FnOnce(std::io::Error) -> AdapterError {
    move |error| AdapterError::ProcessExit {
        code: error.raw_os_error(),
        signal: None,
        unexpected_eof: matches!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe
        ),
        details: format!("{context}: {error}"),
    }
}

struct ManagedProcess {
    child: Child,
    #[cfg(windows)]
    job: usize,
    #[cfg(unix)]
    process_group: i32,
    reaped: bool,
}
impl ManagedProcess {
    fn new(mut child: Child) -> Result<Self, AdapterError> {
        #[cfg(windows)]
        {
            let job = match create_kill_on_close_job(&child) {
                Ok(job) => job as usize,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(process_io("create process job")(error));
                }
            };
            if let Err(error) = resume_suspended_process(&child) {
                unsafe {
                    windows_sys::Win32::System::JobObjects::TerminateJobObject(job as _, 1);
                    windows_sys::Win32::Foundation::CloseHandle(job as _);
                }
                let _ = child.wait();
                return Err(process_io("resume process")(error));
            }
            Ok(Self {
                child,
                job,
                reaped: false,
            })
        }
        #[cfg(unix)]
        {
            let process_group = child.id() as i32;
            Ok(Self {
                child,
                process_group,
                reaped: false,
            })
        }
        #[cfg(not(any(windows, unix)))]
        {
            Ok(Self {
                child,
                reaped: false,
            })
        }
    }
    fn terminate_tree(&mut self) -> std::io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        #[cfg(windows)]
        unsafe {
            if windows_sys::Win32::System::JobObjects::TerminateJobObject(
                self.job as windows_sys::Win32::Foundation::HANDLE,
                1,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(unix)]
        unsafe {
            if libc::kill(-self.process_group, libc::SIGKILL) != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            if self.child.try_wait()?.is_none() {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
        self.reaped = true;
        Ok(())
    }
}
impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let _ = self.terminate_tree();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.job as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}

#[cfg(windows)]
fn configure_child(command: &mut Command) -> Result<(), AdapterError> {
    use std::os::windows::process::CommandExt;
    command.creation_flags(
        windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP
            | windows_sys::Win32::System::Threading::CREATE_SUSPENDED,
    );
    Ok(())
}
#[cfg(unix)]
fn configure_child(command: &mut Command) -> Result<(), AdapterError> {
    use std::os::unix::process::CommandExt;
    let parent = unsafe { libc::getpid() };
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "parent exited during spawn",
                    ));
                }
                // PDEATHSIG only kills the app-server leader. A tiny, detached
                // watchdog also kills the leader's process group so tool/MCP
                // descendants cannot survive an abnormal Node exit.
                spawn_linux_group_watchdog()?;
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
unsafe fn spawn_linux_group_watchdog() -> std::io::Result<()> {
    let runtime_pid = unsafe { libc::getpid() };
    let watcher = unsafe { libc::fork() };
    if watcher < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if watcher == 0 {
        let _ = unsafe { libc::setpgid(0, 0) };
        // The watchdog has no protocol role and must not keep any inherited
        // descriptor open. close_range covers descriptors above old 1024 assumptions.
        let close_range = unsafe { libc::syscall(libc::SYS_close_range, 0u32, u32::MAX, 0u32) };
        if close_range != 0 {
            let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
            if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } == 0 {
                for descriptor in 0..limit.rlim_cur.min(i32::MAX as u64) as i32 {
                    let _ = unsafe { libc::close(descriptor) };
                }
            }
        }
        let delay = libc::timespec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        while unsafe { libc::getppid() } == runtime_pid {
            let _ = unsafe { libc::nanosleep(&raw const delay, std::ptr::null_mut()) };
        }
        let _ = unsafe { libc::kill(-runtime_pid, libc::SIGKILL) };
        unsafe { libc::_exit(0) };
    }
    Ok(())
}
#[cfg(not(any(windows, unix)))]
fn configure_child(_: &mut Command) -> Result<(), AdapterError> {
    Ok(())
}

#[cfg(windows)]
fn create_kill_on_close_job(
    child: &Child,
) -> std::io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::{mem::size_of, os::windows::io::AsRawHandle};
    use windows_sys::Win32::System::JobObjects::*;
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } != 0;
    let assigned =
        configured && unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) } != 0;
    if !assigned {
        let error = std::io::Error::last_os_error();
        unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        Err(error)
    } else {
        Ok(job)
    }
}

#[cfg(windows)]
fn resume_suspended_process(child: &Child) -> std::io::Result<()> {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut found = false;
    let mut next = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
    while next {
        if entry.th32OwnerProcessID == child.id() {
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if !thread.is_null() {
                let resumed = unsafe { ResumeThread(thread) };
                unsafe { CloseHandle(thread) };
                if resumed != u32::MAX {
                    found = true;
                    break;
                }
            }
        }
        next = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if found {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn version_parser_accepts_pinned_codex_shape() {
        assert_eq!(parse_version("codex-cli 0.139.0"), Some((0, 139, 0)));
        assert_eq!(parse_version("not-a-version"), None);
    }

    #[test]
    fn model_list_default_is_used_to_override_stale_user_config_defaults() {
        let models = json!({
            "data": [
                {"id": "too-old", "isDefault": false},
                {"id": "gpt-5.5", "isDefault": true}
            ]
        });

        assert_eq!(parse_default_model(&models), Some("gpt-5.5".into()));
    }

    #[test]
    fn default_app_server_args_disable_user_hooks_and_plugins() {
        let args = CodexAdapterConfig::default()
            .app_server_args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args[0], "app-server");
        assert!(args.windows(2).any(|pair| pair == ["--disable", "hooks"]));
        assert!(args.windows(2).any(|pair| pair == ["--disable", "plugins"]));
        assert!(args.windows(2).any(|pair| pair == ["--disable", "apps"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--disable", "shell_snapshot"])
        );
        assert!(args.windows(2).any(|pair| pair == ["-c", "notify=[]"]));
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_resolution_prefers_executable_extension_over_extensionless_shim() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "pinvou-codex-path-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("codex"), b"not executable").unwrap();
        std::fs::write(root.join("codex.exe"), b"preferred").unwrap();
        let old_path = std::env::var_os("PATH");
        let old_pathext = std::env::var_os("PATHEXT");
        unsafe {
            std::env::set_var("PATH", &root);
            std::env::set_var("PATHEXT", ".EXE;.CMD");
        }

        let resolved = find_on_path(std::path::Path::new("codex")).unwrap();

        if let Some(old_path) = old_path {
            unsafe { std::env::set_var("PATH", old_path) };
        } else {
            unsafe { std::env::remove_var("PATH") };
        }
        if let Some(old_pathext) = old_pathext {
            unsafe { std::env::set_var("PATHEXT", old_pathext) };
        } else {
            unsafe { std::env::remove_var("PATHEXT") };
        }
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            resolved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_lowercase(),
            "codex.exe"
        );
    }

    #[test]
    fn doctor_probe_is_diagnostic_and_never_blocks_runtime_probe() {
        let config = CodexAdapterConfig {
            executable: PathBuf::from("definitely-missing-codex-doctor-fixture"),
            doctor_args: vec!["doctor".into()],
            ..CodexAdapterConfig::default()
        };

        run_diagnostic_doctor_probe(&config);
    }

    #[cfg(windows)]
    #[test]
    fn long_lived_transport_serves_multiple_requests_and_closes_its_job() {
        let script = r#"
while (($line = [Console]::In.ReadLine()) -ne $null) {
  $frame = $line | ConvertFrom-Json
  if ($frame.method -eq 'initialize') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"userAgent":"fixture/0.139"}}'))
  } elseif ($frame.method -eq 'account/read') {
    [Console]::Out.WriteLine(('{"id":' + $frame.id + ',"result":{"requiresOpenaiAuth":false,"account":null}}'))
  }
}
"#;
        let config = CodexAdapterConfig {
            executable: PathBuf::from("powershell.exe"),
            app_server_args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script.into(),
            ],
            handshake_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
            ..CodexAdapterConfig::default()
        };
        let (mut connection, _events) = Connection::spawn(
            &config,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Mutex::new(HashMap::new())),
        )
        .unwrap();
        let initialized = connection
            .request_with_timeout("initialize", json!({}), Duration::from_secs(5), true)
            .unwrap();
        assert_eq!(initialized["userAgent"], "fixture/0.139");
        let account = connection
            .request_with_timeout("account/read", json!({}), Duration::from_secs(5), false)
            .unwrap();
        assert_eq!(account["requiresOpenaiAuth"], false);
        connection.close().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn managed_process_terminates_job_even_after_leader_exits() {
        use std::{
            io::BufRead,
            os::windows::io::AsRawHandle,
            process::{Command, Stdio},
        };
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, WaitForSingleObject},
        };

        let script = r#"
$child = Start-Process powershell.exe -WindowStyle Hidden -PassThru -ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 60')
[Console]::Out.WriteLine($child.Id)
[Console]::Out.Flush()
"#;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_child(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut process = ManagedProcess::new(child).unwrap();
        let mut output = String::new();
        let mut stdout = BufReader::new(stdout);
        stdout.read_line(&mut output).unwrap();
        let grandchild: u32 = output.trim().parse().unwrap();

        let mut leader_exited = false;
        for _ in 0..100 {
            if process.child.try_wait().unwrap().is_some() {
                leader_exited = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(leader_exited);
        const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
        let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, grandchild) };
        assert!(!handle.is_null());

        process.terminate_tree().unwrap();
        assert_eq!(unsafe { WaitForSingleObject(handle, 5_000) }, 0);
        unsafe { CloseHandle(handle) };

        // Keep the raw handle observable to catch accidental plain Child::kill
        // implementations that do not bind the process into a kill-on-close job.
        let _ = process.child.as_raw_handle();
    }
}
