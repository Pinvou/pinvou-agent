use crate::desktop_profile::{DesktopRuntimeProfile, load_desktop_runtime_profiles};
use pinvou_protocol::{RateClass, RuntimeEventEnvelope, RuntimeEventKind, StreamId};
use pinvou_runtime_api::{
    AdapterError, AgentRuntimeAdapter, ApprovalProfile, AuthStatus, ControlStrength,
    LogicalSessionId, ModelCatalog, ModelDescriptor, ModelId, NegotiatedCapabilities,
    PermissionCapability, RuntimeCapabilities, RuntimeCommand, RuntimeEventSubscription,
    RuntimeOperation, RuntimeSession, SessionDescriptor, SessionSnapshot, SessionStatus,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REQUIRED_METHODS: &[&str] = &[
    "healthz",
    "thread/start",
    "thread/resume",
    "thread/list",
    "thread/read",
    "thread/message",
    "thread/interrupt",
    "approval/resolve",
    "app/models",
    "app/config/get",
    "app/config/set",
    "app/runtime-profile/set",
    "app/credential/set",
    "shutdown",
];

static ATTACHMENT_NONCE: AtomicU64 = AtomicU64::new(1);

type PendingRequests = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, AdapterError>>>>>;

#[derive(Clone, Debug)]
pub struct CodeWhaleAdapterConfig {
    pub executable: PathBuf,
    pub app_server_args: Vec<OsString>,
    pub doctor_args: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
    pub handshake_timeout: Duration,
    pub request_timeout: Duration,
    pub node_id: String,
    pub attachment_id: String,
    pub share_desktop_provider: bool,
}

impl Default for CodeWhaleAdapterConfig {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("codewhale"),
            app_server_args: vec!["app-server".into(), "--stdio".into()],
            doctor_args: vec!["doctor".into(), "--json".into()],
            working_directory: None,
            handshake_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            node_id: "local-node".into(),
            attachment_id: format!(
                "codewhale-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                ATTACHMENT_NONCE.fetch_add(1, Ordering::Relaxed)
            ),
            share_desktop_provider: true,
        }
    }
}

pub struct CodeWhaleAdapter {
    config: CodeWhaleAdapterConfig,
    negotiated: NegotiatedCapabilities,
    connection: Option<Connection>,
    event_rx: Option<mpsc::Receiver<Result<RuntimeEventEnvelope, AdapterError>>>,
    sessions: HashSet<String>,
    doctor: Option<Value>,
    current_model: Option<String>,
    current_selection: Option<String>,
    current_reasoning_level: Option<String>,
    desktop_profile_revision: Option<String>,
    desktop_catalog_revision: Option<String>,
    desktop_profiles: Vec<DesktopRuntimeProfile>,
}

impl CodeWhaleAdapter {
    pub fn new(config: CodeWhaleAdapterConfig) -> Self {
        Self {
            config,
            negotiated: NegotiatedCapabilities::default(),
            connection: None,
            event_rx: None,
            sessions: HashSet::new(),
            doctor: None,
            current_model: None,
            current_selection: None,
            current_reasoning_level: None,
            desktop_profile_revision: None,
            desktop_catalog_revision: None,
            desktop_profiles: Vec::new(),
        }
    }

    fn ensure_connection(&mut self) -> Result<&Connection, AdapterError> {
        if self.connection.is_none() {
            let (connection, event_rx) = Connection::spawn(&self.config)?;
            self.connection = Some(connection);
            self.event_rx = Some(event_rx);
        }
        Ok(self.connection.as_ref().expect("connection initialized"))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, AdapterError> {
        let timeout = self.config.request_timeout;
        self.ensure_connection()?.request(method, params, timeout)
    }

    fn known_session(&self, session: &RuntimeSession) -> Result<(), AdapterError> {
        if self.sessions.contains(session.as_str()) {
            Ok(())
        } else {
            Err(AdapterError::InvalidRequest {
                details: "unknown CodeWhale runtime session".into(),
            })
        }
    }

    fn apply_reasoning_level(&mut self, options: &Value) -> Result<(), AdapterError> {
        let Some(level) = options.get("reasoning_level").and_then(Value::as_str) else {
            return Ok(());
        };
        let response = if self.config.share_desktop_provider {
            let Some(mut profile) = self
                .current_selection
                .as_deref()
                .and_then(|selection| {
                    self.desktop_profiles
                        .iter()
                        .find(|profile| profile.selection_id == selection)
                })
                .cloned()
            else {
                return Err(AdapterError::InvalidRequest {
                    details: "Pinvou Desktop provider profile is unavailable".into(),
                });
            };
            profile.payload["reasoning_effort"] = Value::String(level.to_owned());
            self.desktop_profile_revision = Some(profile.revision);
            self.request("app/runtime-profile/set", profile.payload)?
        } else {
            self.request(
                "app/config/set",
                json!({"key":"reasoning_effort","value":level}),
            )?
        };
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            self.current_reasoning_level = Some(level.to_owned());
            Ok(())
        } else {
            Err(protocol(
                "app/config/set",
                "CodeWhale rejected the reasoning level",
            ))
        }
    }

    fn refresh_desktop_profile(&mut self, refresh_credential: bool) -> Result<(), AdapterError> {
        if !self.config.share_desktop_provider {
            return Ok(());
        }
        let profiles = load_desktop_runtime_profiles()
            .map_err(|details| AdapterError::InvalidRequest { details })?;
        let Some(active_profile) = profiles.first().cloned() else {
            return Ok(());
        };
        let catalog_changed = self.desktop_catalog_revision.as_deref()
            != Some(active_profile.catalog_revision.as_str());
        self.desktop_profiles = profiles;
        let mut profile = if !catalog_changed {
            self.current_selection
                .as_deref()
                .and_then(|selection| {
                    self.desktop_profiles
                        .iter()
                        .find(|profile| profile.selection_id == selection)
                })
                .cloned()
                .unwrap_or(active_profile)
        } else {
            active_profile
        };
        let profile_changed = self.desktop_profile_revision.as_deref() != Some(&profile.revision);
        let desktop_reasoning_level = profile.reasoning_level.clone();
        if !profile_changed && let Some(level) = self.current_reasoning_level.as_deref() {
            profile.payload["reasoning_effort"] = Value::String(level.to_owned());
        }
        if refresh_credential && !profile.configured {
            return Err(AdapterError::InvalidRequest {
                details: format!(
                    "{} requires an API key; open /model and configure the Provider",
                    profile.display_name
                ),
            });
        }
        if profile.configured && (refresh_credential || profile_changed) {
            // Turn boundaries always push the reference so a credential
            // replaced or deleted in secure storage takes effect even when
            // settings.json itself did not change. Read-only catalog refreshes
            // avoid repeatedly touching Keychain for an unchanged revision.
            let response = self.request("app/runtime-profile/set", profile.payload)?;
            if response.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(protocol(
                    "app/runtime-profile/set",
                    "CodeWhale rejected the Pinvou Desktop provider profile",
                ));
            }
            self.desktop_profile_revision = Some(profile.revision);
        }
        // A Desktop route revision establishes the default selection. A model
        // or level explicitly selected in this CLI remains active until the
        // Desktop route changes again.
        if profile_changed || self.current_model.is_none() {
            self.current_model = Some(profile.model_id);
            self.current_selection = Some(profile.selection_id);
            self.current_reasoning_level = desktop_reasoning_level;
        }
        self.desktop_catalog_revision = Some(profile.catalog_revision);
        Ok(())
    }

    fn activate_desktop_selection(
        &mut self,
        selection: &str,
    ) -> Result<Option<String>, AdapterError> {
        let Some(profile) = self
            .desktop_profiles
            .iter()
            .find(|profile| profile.selection_id == selection)
            .cloned()
        else {
            return Ok(None);
        };
        let response = self.request("app/runtime-profile/set", profile.payload.clone())?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(protocol(
                "app/runtime-profile/set",
                "CodeWhale rejected the selected Pinvou Desktop provider profile",
            ));
        }
        self.desktop_profile_revision = Some(profile.revision);
        self.desktop_catalog_revision = Some(profile.catalog_revision);
        self.current_selection = Some(profile.selection_id);
        self.current_model = Some(profile.model_id.clone());
        self.current_reasoning_level = profile.reasoning_level;
        Ok(Some(profile.model_id))
    }
}

impl Default for CodeWhaleAdapter {
    fn default() -> Self {
        Self::new(CodeWhaleAdapterConfig::default())
    }
}

impl AgentRuntimeAdapter for CodeWhaleAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        let doctor = run_doctor(&self.config)?;
        self.current_model = doctor
            .get("default_text_model")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        self.doctor = Some(doctor);

        let handshake_timeout = self.config.handshake_timeout;
        let health = self
            .ensure_connection()?
            .request("healthz", json!({}), handshake_timeout)?;
        if health.get("status").and_then(Value::as_str) != Some("ok")
            || health.get("transport").and_then(Value::as_str) != Some("stdio")
        {
            return Err(protocol("healthz", "invalid CodeWhale health response"));
        }
        let capabilities = self.request("capabilities", json!({}))?;
        let methods = capabilities
            .get("methods")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("capabilities", "missing method catalog"))?;
        let methods = methods
            .iter()
            .filter_map(Value::as_str)
            .collect::<HashSet<_>>();
        if let Some(missing) = REQUIRED_METHODS
            .iter()
            .find(|method| !methods.contains(**method))
        {
            return Err(protocol(
                "capabilities",
                format!("CodeWhale app-server is missing {missing}"),
            ));
        }
        self.negotiated.complete(RuntimeCapabilities {
            interactive_chat: true,
            native_resume: true,
            history_import: false,
            tool_approval: methods.contains("approval/resolve"),
            elicitation: methods.contains("app/request"),
            steering: false,
            image_input: false,
            file_reference: true,
            session_listing: true,
            model_catalog: true,
            model_switching: true,
            permission_profiles: true,
            session_modes: vec!["interactive".into(), "agent".into()],
            config_options: vec!["model".into(), "cwd".into()],
            auth_flows: vec!["codewhale_auth".into(), "provider_config".into()],
        });
        self.refresh_desktop_profile(false)?;
        Ok(())
    }

    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        self.negotiated.snapshot()
    }

    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        self.capabilities()?;
        let availability = self
            .doctor
            .as_ref()
            .and_then(|value| value.pointer("/api_key/availability"))
            .and_then(Value::as_str);
        Ok(match availability {
            Some("present") => AuthStatus::Authenticated,
            Some("not_required") | Some("local_runtime") => AuthStatus::NotRequired,
            Some("unavailable") => AuthStatus::Blocked,
            _ => AuthStatus::Unknown,
        })
    }

    fn create(&mut self, operation: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        self.refresh_desktop_profile(true)?;
        self.apply_reasoning_level(&operation.options)?;
        let cwd = operation
            .options
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                self.config
                    .working_directory
                    .as_ref()
                    .map(|path| path.display().to_string())
            });
        let requested_model = operation
            .options
            .get("model_id")
            .or_else(|| operation.options.get("model"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.current_selection.clone())
            .or_else(|| self.current_model.clone());
        let model = match requested_model.as_deref() {
            Some(selection) => self
                .activate_desktop_selection(selection)?
                .or_else(|| Some(selection.to_owned())),
            None => self.current_model.clone(),
        };
        if model.is_some() {
            self.current_model = model.clone();
        }
        let response = self.request(
            "thread/start",
            json!({"cwd":cwd,"model":model,"persist_extended_history":true}),
        )?;
        let id = thread_id(&response, "thread/start")?;
        let session = RuntimeSession::new(id.clone())?;
        self.sessions.insert(id);
        Ok(session)
    }

    fn resume(&mut self, operation: RuntimeOperation) -> Result<RuntimeSession, AdapterError> {
        self.refresh_desktop_profile(true)?;
        self.apply_reasoning_level(&operation.options)?;
        let id = operation
            .options
            .get("thread_id")
            .and_then(Value::as_str)
            .unwrap_or(&operation.operation_id)
            .to_owned();
        let requested_model = operation
            .options
            .get("model_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let model = match requested_model.as_deref() {
            Some(selection) => self
                .activate_desktop_selection(selection)?
                .or_else(|| Some(selection.to_owned())),
            None => None,
        };
        if model.is_some() {
            self.current_model = model.clone();
        }
        let response = self.request(
            "thread/resume",
            json!({
                "thread_id":id,
                "cwd":operation.options.get("cwd"),
                "model":model,
                "persist_extended_history":true
            }),
        )?;
        let id = thread_id(&response, "thread/resume")?;
        let session = RuntimeSession::new(id.clone())?;
        self.sessions.insert(id);
        Ok(session)
    }

    fn list_sessions(
        &mut self,
        operation: RuntimeOperation,
    ) -> Result<Vec<SessionDescriptor>, AdapterError> {
        let limit = operation
            .options
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(100);
        let response = self.request(
            "thread/list",
            json!({"include_archived":false,"limit":limit}),
        )?;
        response
            .get("threads")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("thread/list", "missing threads"))?
            .iter()
            .map(parse_session_descriptor)
            .collect()
    }

    fn read_session(
        &mut self,
        operation: RuntimeOperation,
    ) -> Result<SessionSnapshot, AdapterError> {
        let id = operation
            .options
            .get("thread_id")
            .and_then(Value::as_str)
            .unwrap_or(&operation.operation_id);
        let response = self.request("thread/read", json!({"thread_id":id}))?;
        let thread = response
            .get("thread")
            .ok_or_else(|| protocol("thread/read", "missing thread"))?;
        let descriptor = parse_session_descriptor(thread)?;
        Ok(SessionSnapshot {
            cursor: 0,
            descriptor,
            normalized_events: Vec::new(),
        })
    }

    fn list_models(&mut self, _: RuntimeOperation) -> Result<ModelCatalog, AdapterError> {
        self.refresh_desktop_profile(false)?;
        if !self.desktop_profiles.is_empty() {
            let current = self.current_selection.clone().or_else(|| {
                self.desktop_profiles
                    .first()
                    .map(|profile| profile.selection_id.clone())
            });
            let mut models = self
                .desktop_profiles
                .iter()
                .map(|profile| {
                    let descriptor = ModelDescriptor::new(
                        profile.selection_id.clone(),
                        profile.display_name.clone(),
                        true,
                        current.as_deref() == Some(profile.selection_id.as_str()),
                    )?
                    .with_provider(
                        profile.provider_id.clone(),
                        profile.provider_display_name.clone(),
                        profile.configured,
                        profile.requires_api_key,
                    );
                    descriptor.with_reasoning_levels(
                        profile
                            .reasoning_level
                            .clone()
                            .or_else(|| Some("auto".into())),
                        ["auto", "off", "low", "medium", "high", "max"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    )
                })
                .collect::<Result<Vec<_>, AdapterError>>()?;
            models.sort_by_key(|model| {
                (
                    current.as_deref() != Some(model.id.as_str()),
                    !model.configured,
                )
            });
            return ModelCatalog::new("codewhale", current.map(ModelId::new).transpose()?, models)?
                .with_current_reasoning_level(
                    self.current_reasoning_level
                        .clone()
                        .or_else(|| Some("auto".into())),
                );
        }
        let response = self.request("app/models", json!({}))?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(protocol("app/models", "model catalog request failed"));
        }
        let values = response
            .pointer("/data/models")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("app/models", "missing models"))?;
        let current_reasoning_level = self
            .current_reasoning_level
            .clone()
            .or_else(|| {
                self.request("app/config/get", json!({"key":"reasoning_effort"}))
                    .ok()
                    .and_then(|response| {
                        response
                            .pointer("/data/value")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
            })
            .or_else(|| Some("auto".into()));
        let mut models = Vec::new();
        let mut seen = HashSet::new();
        for value in values {
            let Some(id) = value
                .get("id")
                .or_else(|| value.get("model"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let display = value
                .get("name")
                .or_else(|| value.get("display_name"))
                .and_then(Value::as_str)
                .unwrap_or(id);
            let mut descriptor =
                ModelDescriptor::new(id, display, true, self.current_model.as_deref() == Some(id))?;
            if value
                .get("supports_reasoning")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                descriptor = descriptor.with_reasoning_levels(
                    current_reasoning_level.clone(),
                    [
                        "auto",
                        "off",
                        "low",
                        "medium",
                        "high",
                        "max",
                        "xhigh",
                        "ultracode",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                )?;
            }
            models.push(descriptor);
        }
        if let Some(current) = self.current_model.as_deref()
            && !seen.contains(current)
        {
            models.push(
                ModelDescriptor::new(current, current, true, true)?.with_reasoning_levels(
                    current_reasoning_level.clone(),
                    ["auto", "off", "low", "medium", "high", "max"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                )?,
            );
        }
        if models.is_empty()
            && let Some(current) = self.current_model.as_deref()
        {
            models.push(ModelDescriptor::new(current, current, true, true)?);
        }
        ModelCatalog::new(
            "codewhale",
            self.current_model.clone().map(ModelId::new).transpose()?,
            models,
        )?
        .with_current_reasoning_level(current_reasoning_level)
    }

    fn configure_model_credential(
        &mut self,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.refresh_desktop_profile(false)?;
        let selection = operation
            .options
            .get("model_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "model credential selection is missing".into(),
            })?;
        let api_key = operation
            .options
            .get("api_key")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "API key is empty".into(),
            })?;
        if !self
            .desktop_profiles
            .iter()
            .any(|profile| profile.selection_id == selection && profile.requires_api_key)
        {
            return Err(AdapterError::InvalidRequest {
                details: "selected Desktop model does not accept an API key".into(),
            });
        }
        let response = self.request(
            "app/credential/set",
            json!({
                "service":"pinvou3-model-api-key",
                "account":format!("model:{selection}"),
                "version":1,
                "api_key":api_key
            }),
        )?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(protocol("app/credential/set", "credential was not stored"));
        }
        crate::desktop_profile::mark_model_credential_configured(selection)
            .map_err(|details| AdapterError::InvalidRequest { details })?;
        self.desktop_catalog_revision = None;
        self.refresh_desktop_profile(false)?;
        self.activate_desktop_selection(selection)?.ok_or_else(|| {
            AdapterError::InvalidRequest {
                details: "configured Desktop model disappeared during refresh".into(),
            }
        })?;
        Ok(())
    }

    fn inspect_permissions(
        &mut self,
        _: RuntimeOperation,
    ) -> Result<PermissionCapability, AdapterError> {
        self.capabilities()?;
        Ok(PermissionCapability {
            supported_profiles: vec![
                ApprovalProfile::Request,
                ApprovalProfile::Assisted,
                ApprovalProfile::FullAccess,
            ],
            control_strength: ControlStrength::Partial,
            native_mode: Some("CodeWhale approval policy + sandbox".into()),
            sandbox: Some("runtime-enforced".into()),
            residual_guards: vec!["CodeWhale permissions.toml".into(), "OS policy".into()],
            evidence_version: "codewhale-app-server-stdio-v2".into(),
        })
    }

    fn send(
        &mut self,
        session: &RuntimeSession,
        command: RuntimeCommand,
    ) -> Result<(), AdapterError> {
        self.known_session(session)?;
        self.refresh_desktop_profile(true)?;
        if command.kind != "text" {
            return Err(AdapterError::InvalidRequest {
                details: "CodeWhale adapter currently accepts text commands".into(),
            });
        }
        let input = command
            .payload
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "CodeWhale text command must be a non-empty string".into(),
            })?;
        self.ensure_connection()?.send_turn(session.as_str(), input)
    }

    fn approve(
        &mut self,
        session: &RuntimeSession,
        operation: RuntimeOperation,
    ) -> Result<(), AdapterError> {
        self.known_session(session)?;
        let decision = match operation.options.get("decision").and_then(Value::as_str) {
            Some("accept" | "allow" | "approved") => "allow",
            Some("decline" | "deny" | "denied") => "deny",
            _ => {
                return Err(AdapterError::InvalidRequest {
                    details: "CodeWhale approval decision must be accept or decline".into(),
                });
            }
        };
        self.ensure_connection()?.resolve_approval(
            session.as_str(),
            &operation.operation_id,
            decision,
        )
    }

    fn interrupt(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        self.known_session(session)?;
        self.request("thread/interrupt", json!({"thread_id":session.as_str()}))?;
        Ok(())
    }

    fn subscribe_events(
        &mut self,
        session: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError> {
        self.known_session(session)?;
        let receiver = self
            .event_rx
            .take()
            .ok_or_else(|| AdapterError::InvalidRequest {
                details: "CodeWhale event stream is already subscribed".into(),
            })?;
        Ok(Box::new(receiver.into_iter()))
    }

    fn close(&mut self, session: &RuntimeSession) -> Result<(), AdapterError> {
        self.sessions.remove(session.as_str());
        if self.sessions.is_empty()
            && let Some(mut connection) = self.connection.take()
        {
            connection.close()?;
        }
        Ok(())
    }
}

impl Drop for CodeWhaleAdapter {
    fn drop(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            let _ = connection.close();
        }
    }
}

struct Connection {
    child: Arc<Mutex<Option<Child>>>,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    pending: PendingRequests,
    projector: Arc<Mutex<Projector>>,
    next_id: AtomicU64,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
}

impl Connection {
    fn spawn(
        config: &CodeWhaleAdapterConfig,
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
        let mut child = command.spawn().map_err(|error| AdapterError::ProcessExit {
            code: None,
            signal: None,
            unexpected_eof: true,
            details: format!("failed to start CodeWhale: {error}"),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| process_pipe("stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| process_pipe("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| process_pipe("stderr"))?;
        let child = Arc::new(Mutex::new(Some(child)));
        let writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let projector = Arc::new(Mutex::new(Projector::new(
            config.node_id.clone(),
            config.attachment_id.clone(),
        )));
        let (event_tx, event_rx) = mpsc::sync_channel(1024);
        let reader_pending = Arc::clone(&pending);
        let reader_projector = Arc::clone(&projector);
        let stdout_thread = thread::Builder::new()
            .name("pinvou-codewhale-stdout".into())
            .spawn(move || {
                read_stdout(stdout, reader_pending, reader_projector, event_tx);
            })
            .map_err(|error| AdapterError::ProcessExit {
                code: None,
                signal: None,
                unexpected_eof: true,
                details: format!("failed to start CodeWhale reader: {error}"),
            })?;
        let stderr_thread = thread::Builder::new()
            .name("pinvou-codewhale-stderr".into())
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut sink = [0_u8; 8192];
                while reader.read(&mut sink).is_ok_and(|read| read > 0) {}
            })
            .map_err(|error| AdapterError::ProcessExit {
                code: None,
                signal: None,
                unexpected_eof: true,
                details: format!("failed to start CodeWhale stderr drain: {error}"),
            })?;
        Ok((
            Self {
                child,
                writer,
                pending,
                projector,
                next_id: AtomicU64::new(1),
                stdout_thread: Some(stdout_thread),
                stderr_thread: Some(stderr_thread),
            },
            event_rx,
        ))
    }

    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().map_err(lock_error)?.insert(id, tx);
        if let Err(error) = self.write(json!({
            "jsonrpc":"2.0","id":id,"method":method,"params":params
        })) {
            self.pending.lock().map_err(lock_error)?.remove(&id);
            return Err(error);
        }
        rx.recv_timeout(timeout).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => AdapterError::HandshakeTimeout,
            mpsc::RecvTimeoutError::Disconnected => process_pipe("response channel"),
        })?
    }

    fn send_turn(&self, session: &str, input: &str) -> Result<(), AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.projector
            .lock()
            .map_err(lock_error)?
            .begin_turn(session, id);
        self.write(json!({
            "jsonrpc":"2.0","id":id,"method":"thread/message",
            "params":{"thread_id":session,"input":input}
        }))
    }

    fn resolve_approval(
        &self,
        session: &str,
        approval_id: &str,
        decision: &str,
    ) -> Result<(), AdapterError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // A streaming stdio turn owns CodeWhale's stdout writer. CodeWhale
        // applies this decision immediately but cannot serialize the JSON-RPC
        // acknowledgement until the turn releases that writer, so do not
        // block the TUI on the acknowledgement.
        self.write(json!({
            "jsonrpc":"2.0","id":id,"method":"approval/resolve",
            "params":{
                "thread_id":session,
                "approval_id":approval_id,
                "decision":decision,
                "remember":false
            }
        }))
    }

    fn write(&self, value: Value) -> Result<(), AdapterError> {
        let mut writer = self.writer.lock().map_err(lock_error)?;
        serde_json::to_writer(&mut *writer, &value)
            .map_err(|error| protocol("stdio/write", error.to_string()))?;
        writer
            .write_all(b"\n")
            .and_then(|_| writer.flush())
            .map_err(|error| process_pipe(&format!("write: {error}")))
    }

    fn close(&mut self) -> Result<(), AdapterError> {
        let _ = self.request("shutdown", json!({}), Duration::from_secs(3));
        if let Ok(mut child) = self.child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
        Ok(())
    }
}

fn read_stdout(
    stdout: impl Read,
    pending: PendingRequests,
    projector: Arc<Mutex<Projector>>,
    event_tx: mpsc::SyncSender<Result<RuntimeEventEnvelope, AdapterError>>,
) {
    for line in BufReader::new(stdout).lines() {
        let value = match line {
            Ok(line) if line.len() <= 16 * 1024 * 1024 => serde_json::from_str::<Value>(&line)
                .map_err(|error| protocol("stdio/read", error.to_string())),
            Ok(_) => Err(protocol("stdio/read", "JSON line exceeds 16 MiB")),
            Err(error) => Err(process_pipe(&format!("read: {error}"))),
        };
        let value = match value {
            Ok(value) => value,
            Err(error) => {
                let _ = event_tx.send(Err(error));
                return;
            }
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            if let Ok(mut pending) = pending.lock()
                && let Some(sender) = pending.remove(&id)
            {
                let result = parse_response(value);
                let _ = sender.send(result);
                continue;
            }
            if let Ok(mut projector) = projector.lock()
                && let Some(events) = projector.finish_async_response(id, &value)
            {
                for event in events {
                    let _ = event_tx.send(event);
                }
            }
            continue;
        }
        if value.get("type").is_some()
            && let Ok(mut projector) = projector.lock()
        {
            for event in projector.project(&value) {
                if event_tx.send(event).is_err() {
                    return;
                }
            }
        }
    }
}

fn parse_response(value: Value) -> Result<Value, AdapterError> {
    if let Some(error) = value.get("error") {
        return Err(AdapterError::Protocol {
            code: error.get("code").and_then(Value::as_i64),
            method: None,
            details: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("CodeWhale request failed")
                .to_owned(),
        });
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| protocol("stdio/read", "response has no result"))
}

struct Projector {
    node_id: String,
    attachment_id: String,
    logical_session_id: String,
    turn_id: Option<String>,
    async_requests: HashMap<u64, String>,
    next_control_seq: u64,
    next_main_seq: u64,
}

impl Projector {
    fn new(node_id: String, attachment_id: String) -> Self {
        Self {
            node_id,
            attachment_id,
            logical_session_id: "codewhale-pending".into(),
            turn_id: None,
            async_requests: HashMap::new(),
            next_control_seq: 1,
            next_main_seq: 1,
        }
    }

    fn begin_turn(&mut self, session: &str, request_id: u64) {
        self.logical_session_id = session.to_owned();
        self.turn_id = None;
        self.async_requests.insert(request_id, session.to_owned());
    }

    fn finish_async_response(
        &mut self,
        id: u64,
        response: &Value,
    ) -> Option<Vec<Result<RuntimeEventEnvelope, AdapterError>>> {
        self.async_requests.remove(&id)?;
        let mut events = Vec::new();
        if self.turn_id.is_none() {
            self.turn_id = Some(format!("codewhale-turn-{id}"));
            events.push(self.event(
                RuntimeEventKind::TurnStarted,
                RateClass::R0,
                json!({"user_input_ref":"codewhale:thread/message"}),
            ));
        }
        let (end_reason, message) = if response.get("error").is_some() {
            let message = response
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(actionable_runtime_error);
            ("error", message)
        } else {
            ("completed", None)
        };
        if let Some(message) = message.as_deref() {
            events.push(self.event(
                RuntimeEventKind::ErrorRaised,
                RateClass::R0,
                json!({
                    "code":"codewhale_error",
                    "message":message,
                    "fatal":true,
                    "source":"runtime"
                }),
            ));
        }
        events.push(self.event(
            RuntimeEventKind::TurnEnded,
            RateClass::R0,
            json!({"end_reason":end_reason,"message":message}),
        ));
        Some(events)
    }

    fn project(&mut self, frame: &Value) -> Vec<Result<RuntimeEventEnvelope, AdapterError>> {
        let event_type = frame
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match event_type {
            "response_start" => {
                // A single CodeWhale thread/message can contain several model
                // responses separated by tool calls. They are model rounds of
                // one agent turn, not independent Pinvou turns.
                if self.turn_id.is_some() {
                    return Vec::new();
                }
                let turn_id = frame
                    .get("response_id")
                    .and_then(Value::as_str)
                    .and_then(|value| value.rsplit_once(':').map(|(_, turn)| turn))
                    .unwrap_or("codewhale-turn")
                    .to_owned();
                self.turn_id = Some(turn_id);
                vec![self.event(
                    RuntimeEventKind::TurnStarted,
                    RateClass::R0,
                    json!({"user_input_ref":"codewhale:thread/message"}),
                )]
            }
            "response_delta" => vec![self.event(
                RuntimeEventKind::TextDelta,
                RateClass::R1,
                json!({"role":"assistant","content":frame.get("delta").and_then(Value::as_str).unwrap_or("")}),
            )],
            "thinking_delta" => vec![self.event(
                RuntimeEventKind::ThinkingDelta,
                RateClass::R1,
                json!({"content":frame.get("delta").and_then(Value::as_str).unwrap_or("")}),
            )],
            // CodeWhale emits response_end before the JSON-RPC result. Only the
            // latter distinguishes completed, failed, interrupted, and canceled.
            "response_end" => Vec::new(),
            "tool_call_start" => vec![self.event(
                RuntimeEventKind::ToolCallStarted,
                RateClass::R1,
                json!({"tool_id":frame.get("id").and_then(Value::as_str).unwrap_or("codewhale-tool"),"name":frame.get("name").and_then(Value::as_str).unwrap_or("tool")}),
            )],
            "tool_call_result" => vec![self.event(
                RuntimeEventKind::ToolCallCompleted,
                RateClass::R1,
                json!({"tool_id":frame.get("id").and_then(Value::as_str).unwrap_or("codewhale-tool"),"result":frame.get("result").cloned().unwrap_or(Value::Null),"is_error":false}),
            )],
            "approval_required" => {
                let approval_id = frame
                    .get("approval_id")
                    .or_else(|| frame.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("codewhale-approval");
                let tool = frame
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let summary = frame
                    .get("description")
                    .or_else(|| frame.get("intent_summary"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("CodeWhale requests approval");
                vec![self.event(
                    RuntimeEventKind::ApprovalRequested,
                    RateClass::R0,
                    json!({
                        "approval_id":approval_id,
                        "tool":tool,
                        "summary":summary,
                        "options":["allow","deny"]
                    }),
                )]
            }
            "approval_resolved" => vec![self.event(
                RuntimeEventKind::ApprovalResolved,
                RateClass::R0,
                json!({
                    "approval_id":frame.get("approval_id").or_else(||frame.get("id")).and_then(Value::as_str).unwrap_or("codewhale-approval"),
                    "outcome":match frame.get("outcome").and_then(Value::as_str) {
                        Some("allow" | "approved") => "approved",
                        Some("deny" | "denied") => "denied",
                        _ => "cancelled",
                    }
                }),
            )],
            _ => Vec::new(),
        }
    }

    fn event(
        &mut self,
        kind: RuntimeEventKind,
        rate: RateClass,
        payload: Value,
    ) -> Result<RuntimeEventEnvelope, AdapterError> {
        let seq = if rate == RateClass::R0 {
            let seq = self.next_control_seq;
            self.next_control_seq = self.next_control_seq.saturating_add(1);
            seq
        } else {
            let seq = self.next_main_seq;
            self.next_main_seq = self.next_main_seq.saturating_add(1);
            seq
        };
        RuntimeEventEnvelope::from_value(json!({
            "protocol_version":1,
            "schema_version":1,
            "node_id":self.node_id,
            "logical_session_id":self.logical_session_id,
            "attachment_id":self.attachment_id,
            "work_id":null,
            "collaborative_run_id":null,
            "stream_id":if rate == RateClass::R0 { StreamId::Control } else { StreamId::Main },
            "turn_id":self.turn_id,
            "seq":seq,
            "source_span":null,
            "timestamp":rfc3339_now(),
            "rate_class":rate,
            "kind":kind,
            "payload":payload
        }))
        .map_err(|error| protocol("event/project", error.to_string()))
    }
}

fn actionable_runtime_error(message: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("api key not found") || normalized.contains("run /auth") {
        return "CodeWhale provider authentication is required. Configure it with `codewhale auth set --provider <provider>`.".into();
    }
    message.chars().take(500).collect()
}

fn run_doctor(config: &CodeWhaleAdapterConfig) -> Result<Value, AdapterError> {
    let mut command = Command::new(&config.executable);
    command.args(&config.doctor_args);
    if let Some(cwd) = &config.working_directory {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .map_err(|error| AdapterError::ProcessExit {
            code: None,
            signal: None,
            unexpected_eof: true,
            details: format!("failed to run CodeWhale doctor: {error}"),
        })?;
    if !output.status.success() {
        return Err(AdapterError::ProcessExit {
            code: output.status.code(),
            signal: None,
            unexpected_eof: false,
            details: "CodeWhale doctor failed".into(),
        });
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| protocol("doctor --json", error.to_string()))
}

fn thread_id(response: &Value, method: &str) -> Result<String, AdapterError> {
    response
        .get("thread_id")
        .or_else(|| response.pointer("/thread/id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| protocol(method, "missing thread id"))
}

fn parse_session_descriptor(value: &Value) -> Result<SessionDescriptor, AdapterError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| protocol("thread/list", "thread has no id"))?;
    let status = match value.get("status").and_then(Value::as_str) {
        Some("completed") => SessionStatus::Completed,
        Some("failed") => SessionStatus::Failed,
        Some("running" | "idle" | "paused") => SessionStatus::Active,
        _ => SessionStatus::Unknown,
    };
    Ok(SessionDescriptor {
        id: LogicalSessionId::new(id)?,
        title: value
            .get("name")
            .or_else(|| value.get("preview"))
            .and_then(Value::as_str)
            .unwrap_or("CodeWhale session")
            .to_owned(),
        last_active_at: value
            .get("updated_at")
            .map(Value::to_string)
            .unwrap_or_default(),
        runtime_id: "codewhale".into(),
        model_id: value
            .get("model")
            .and_then(Value::as_str)
            .map(ModelId::new)
            .transpose()?,
        status,
        native_session_id: Some(id.to_owned()),
    })
}

fn protocol(method: impl Into<String>, details: impl Into<String>) -> AdapterError {
    AdapterError::Protocol {
        code: None,
        method: Some(method.into()),
        details: details.into(),
    }
}

fn process_pipe(name: &str) -> AdapterError {
    AdapterError::ProcessExit {
        code: None,
        signal: None,
        unexpected_eof: true,
        details: format!("CodeWhale {name} is unavailable"),
    }
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> AdapterError {
    AdapterError::ProcessExit {
        code: None,
        signal: None,
        unexpected_eof: false,
        details: "CodeWhale adapter state is unavailable".into(),
    }
}

fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_seconds / 3_600,
        (day_seconds % 3_600) / 60,
        day_seconds % 60,
        duration.subsec_millis()
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projector_waits_for_rpc_result_before_emitting_the_terminal_event() {
        let mut projector = Projector::new("node-a".into(), "attachment-a".into());
        projector.begin_turn("session-a", 7);
        let started = projector.project(&json!({
            "type":"response_start",
            "response_id":"session-a:turn-a"
        }));
        assert_eq!(started.len(), 1);
        assert_eq!(
            started[0].as_ref().unwrap().event_kind(),
            RuntimeEventKind::TurnStarted
        );
        assert!(
            projector
                .project(&json!({"type":"response_end"}))
                .is_empty()
        );

        let ended = projector
            .finish_async_response(7, &json!({"id":7,"result":{"status":"accepted"}}))
            .unwrap()
            .pop()
            .unwrap()
            .unwrap();
        assert_eq!(ended.event_kind(), RuntimeEventKind::TurnEnded);
        assert_eq!(ended.turn_id(), Some("turn-a"));
        let payload: Value = serde_json::from_str(ended.payload().get()).unwrap();
        assert_eq!(payload["end_reason"], "completed");
    }

    #[test]
    fn projector_maps_stream_deltas_to_the_pinvou_event_schema() {
        let mut projector = Projector::new("node-a".into(), "attachment-a".into());
        projector.begin_turn("session-a", 9);
        let _ = projector.project(&json!({
            "type":"response_start",
            "response_id":"session-a:turn-b"
        }));
        let delta = projector.project(&json!({
            "type":"response_delta",
            "response_id":"session-a:turn-b",
            "delta":"hello"
        }));
        let delta = delta[0].as_ref().unwrap();
        assert_eq!(delta.event_kind(), RuntimeEventKind::TextDelta);
        assert_eq!(delta.logical_session_id(), "session-a");
        assert_eq!(delta.turn_id(), Some("turn-b"));
        let payload: Value = serde_json::from_str(delta.payload().get()).unwrap();
        assert_eq!(payload["content"], "hello");

        let thinking = projector.project(&json!({
            "type":"thinking_delta",
            "response_id":"session-a:turn-b",
            "delta":"inspect the route"
        }));
        let thinking = thinking[0].as_ref().unwrap();
        assert_eq!(thinking.event_kind(), RuntimeEventKind::ThinkingDelta);
        let payload: Value = serde_json::from_str(thinking.payload().get()).unwrap();
        assert_eq!(payload["content"], "inspect the route");
    }

    #[test]
    fn projector_keeps_tool_loop_model_rounds_in_one_turn() {
        let mut projector = Projector::new("node-a".into(), "attachment-a".into());
        projector.begin_turn("session-a", 12);

        let started = projector.project(&json!({
            "type":"response_start",
            "response_id":"session-a:turn-tool-loop-round-1"
        }));
        assert_eq!(started.len(), 1);
        assert_eq!(
            started[0].as_ref().unwrap().event_kind(),
            RuntimeEventKind::TurnStarted
        );

        let tool_started = projector.project(&json!({
            "type":"tool_call_start",
            "id":"call-1",
            "name":"shell"
        }));
        assert_eq!(
            tool_started[0].as_ref().unwrap().turn_id(),
            Some("turn-tool-loop-round-1")
        );

        let next_round = projector.project(&json!({
            "type":"response_start",
            "response_id":"session-a:turn-tool-loop-round-2"
        }));
        assert!(next_round.is_empty());

        let delta = projector.project(&json!({
            "type":"response_delta",
            "response_id":"session-a:turn-tool-loop-round-2",
            "delta":"done"
        }));
        assert_eq!(
            delta[0].as_ref().unwrap().turn_id(),
            Some("turn-tool-loop-round-1")
        );

        let ended = projector
            .finish_async_response(12, &json!({"id":12,"result":{"status":"accepted"}}))
            .unwrap();
        assert_eq!(ended.len(), 1);
        assert_eq!(
            ended[0].as_ref().unwrap().event_kind(),
            RuntimeEventKind::TurnEnded
        );
        assert_eq!(
            ended[0].as_ref().unwrap().turn_id(),
            Some("turn-tool-loop-round-1")
        );
    }

    #[test]
    fn projector_maps_codewhale_approval_lifecycle() {
        let mut projector = Projector::new("node-a".into(), "attachment-a".into());
        projector.begin_turn("session-a", 10);
        let _ = projector.project(&json!({
            "type":"response_start",
            "response_id":"session-a:turn-approval"
        }));

        let requested = projector.project(&json!({
            "type":"approval_required",
            "approval_id":"call-weather",
            "tool_name":"js_execution",
            "description":"Fetch current weather"
        }));
        let requested = requested[0].as_ref().unwrap();
        assert_eq!(requested.event_kind(), RuntimeEventKind::ApprovalRequested);
        let payload: Value = serde_json::from_str(requested.payload().get()).unwrap();
        assert_eq!(payload["approval_id"], "call-weather");
        assert_eq!(payload["tool"], "js_execution");
        assert_eq!(payload["summary"], "Fetch current weather");

        let resolved = projector.project(&json!({
            "type":"approval_resolved",
            "approval_id":"call-weather",
            "outcome":"allow"
        }));
        assert_eq!(
            resolved[0].as_ref().unwrap().event_kind(),
            RuntimeEventKind::ApprovalResolved
        );
    }

    #[test]
    fn pre_stream_runtime_failure_still_has_a_valid_turn_and_actionable_error() {
        let mut projector = Projector::new("node-a".into(), "attachment-a".into());
        projector.begin_turn("session-a", 11);
        let events = projector
            .finish_async_response(
                11,
                &json!({"id":11,"error":{"code":-32000,"message":"authentication required"}}),
            )
            .unwrap()
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(RuntimeEventEnvelope::event_kind)
                .collect::<Vec<_>>(),
            [
                RuntimeEventKind::TurnStarted,
                RuntimeEventKind::ErrorRaised,
                RuntimeEventKind::TurnEnded
            ]
        );
        assert!(events.iter().all(|event| event.turn_id().is_some()));
    }

    #[test]
    fn provider_auth_failure_is_reduced_to_a_safe_actionable_message() {
        let message = actionable_runtime_error(
            "DeepSeek API key not found in a verbose provider diagnostic. Run /auth.",
        );
        assert_eq!(
            message,
            "CodeWhale provider authentication is required. Configure it with `codewhale auth set --provider <provider>`."
        );
    }

    #[cfg(unix)]
    #[test]
    fn public_stdio_fixture_covers_probe_create_models_and_chat() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "pinvou-codewhale-adapter-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = root.join("codewhale-fixture.sh");
        std::fs::write(
            &fixture,
            r#"#!/bin/sh
if [ "$1" = "doctor" ]; then
  printf '%s\n' '{"version":"0.9.5","default_text_model":"model-a","api_key":{"availability":"present"}}'
  exit 0
fi
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"healthz"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"status":"ok","service":"deepseek-app-server","transport":"stdio"}}\n' "$id" ;;
    *'"method":"capabilities"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"methods":["healthz","thread/start","thread/resume","thread/list","thread/read","thread/message","thread/interrupt","approval/resolve","app/models","app/config/get","app/config/set","app/runtime-profile/set","app/credential/set","app/request","shutdown"]}}\n' "$id" ;;
    *'"method":"thread/start"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"thread_id":"thread-a","status":"ok"}}\n' "$id" ;;
    *'"method":"app/models"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true,"data":{"models":[{"id":"model-a","name":"Model A"}]},"events":[]}}\n' "$id" ;;
    *'"method":"app/config/get"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true,"data":{"key":"reasoning_effort","value":"high"},"events":[]}}\n' "$id" ;;
    *'"method":"thread/message"'*)
      printf '%s\n' '{"type":"response_start","response_id":"thread-a:turn-a"}'
      printf '%s\n' '{"type":"response_delta","response_id":"thread-a:turn-a","delta":"fixture reply"}'
      printf '%s\n' '{"type":"response_end","response_id":"thread-a:turn-a"}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"thread_id":"thread-a","status":"accepted"}}\n' "$id"
      ;;
    *'"method":"shutdown"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"ok":true,"status":"stopped"}}\n' "$id"; exit 0 ;;
  esac
done
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fixture).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fixture, permissions).unwrap();

        let mut adapter = CodeWhaleAdapter::new(CodeWhaleAdapterConfig {
            executable: fixture,
            working_directory: Some(root.clone()),
            share_desktop_provider: false,
            ..CodeWhaleAdapterConfig::default()
        });
        adapter.probe().unwrap();
        assert_eq!(adapter.auth_status().unwrap(), AuthStatus::Authenticated);
        let models = adapter
            .list_models(RuntimeOperation::new("models", json!({})).unwrap())
            .unwrap();
        assert_eq!(models.runtime_id, "codewhale");
        assert_eq!(models.current_model.unwrap().as_str(), "model-a");

        let session = adapter
            .create(RuntimeOperation::new("create", json!({})).unwrap())
            .unwrap();
        adapter
            .send(&session, RuntimeCommand::text("hello").unwrap())
            .unwrap();
        let mut events = adapter.subscribe_events(&session).unwrap();
        assert_eq!(
            events.next().unwrap().unwrap().event_kind(),
            RuntimeEventKind::TurnStarted
        );
        assert_eq!(
            events.next().unwrap().unwrap().event_kind(),
            RuntimeEventKind::TextDelta
        );
        assert_eq!(
            events.next().unwrap().unwrap().event_kind(),
            RuntimeEventKind::TurnEnded
        );
        adapter.close(&session).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires a configured CodeWhale provider and spends model tokens"]
    fn live_codewhale_stdio_chat_smoke() {
        let executable = std::env::var_os("PINVOU_CODEWHALE_LIVE_BIN")
            .expect("PINVOU_CODEWHALE_LIVE_BIN is required");
        let mut adapter = CodeWhaleAdapter::new(CodeWhaleAdapterConfig {
            executable: executable.into(),
            working_directory: std::env::current_dir().ok(),
            ..CodeWhaleAdapterConfig::default()
        });
        adapter.probe().unwrap();
        let session = adapter
            .create(RuntimeOperation::new("live", json!({})).unwrap())
            .unwrap();
        adapter
            .send(
                &session,
                RuntimeCommand::text("Reply with LIVE_OK only").unwrap(),
            )
            .unwrap();
        let mut events = adapter.subscribe_events(&session).unwrap();
        loop {
            let event = events.next().expect("event stream ended").unwrap();
            eprintln!("{} {}", event.kind(), event.payload().get());
            if event.event_kind() == RuntimeEventKind::TurnEnded {
                break;
            }
        }
        adapter.close(&session).unwrap();
    }
}
