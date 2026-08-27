use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, IpcMessageKind, RateClass, RuntimeEventEnvelope,
    RuntimeEventKind, StreamId,
};
use pinvou_runtime_api::{
    ApprovalProfile, LogicalSessionId, ModelId, SessionDescriptor, SessionStatus,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use crate::{
    ControllerError, LocalNodeClient, SessionStore, SessionStoreError, StoredSessionMetadata,
    WorkspacePreferences, WorkspaceStore,
};

#[derive(Clone, Debug)]
pub struct ControllerSession {
    instance_id: String,
    local_node: Option<LocalNodeRoute>,
    persistent: Option<Arc<Mutex<PersistentControllerState>>>,
    pending: Arc<Mutex<HashMap<String, PreparedOperation>>>,
    next_operation: Arc<AtomicU64>,
    turn_active: Arc<AtomicBool>,
    #[cfg(debug_assertions)]
    scripted_chat: Option<Vec<serde_json::Value>>,
}

#[derive(Clone, Debug)]
struct LocalNodeRoute {
    endpoint: String,
    instance_id: String,
}

#[derive(Debug)]
struct PersistentControllerState {
    session_store: SessionStore,
    workspace_store: WorkspaceStore,
    workspace: PathBuf,
    workspace_key: String,
    preferences: WorkspacePreferences,
    active: Option<ActiveSession>,
}

#[derive(Clone, Debug)]
struct ActiveSession {
    logical_id: LogicalSessionId,
    native_session_id: String,
    attachment_id: Option<String>,
    attachment_epoch: u64,
    runtime_id: String,
    model_id: Option<String>,
    reasoning_level: Option<String>,
    approval_profile: ApprovalProfile,
}

#[derive(Clone, Debug)]
enum PreparedOperation {
    Resume {
        logical_id: LogicalSessionId,
        native_session_id: String,
        attachment_epoch: u64,
        model_id: Option<String>,
        reasoning_level: Option<String>,
    },
    Model {
        logical_id: Option<LogicalSessionId>,
        attachment_epoch: u64,
        runtime_id: String,
        model_id: String,
        reasoning_level: Option<String>,
    },
    Permissions {
        logical_id: Option<LogicalSessionId>,
        attachment_epoch: u64,
        profile: ApprovalProfile,
        full_access_confirmed: bool,
    },
}

impl ControllerSession {
    pub fn new(instance_id: impl Into<String>) -> Result<Self, ControllerError> {
        let instance_id = instance_id.into();
        if instance_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        Ok(Self {
            instance_id,
            local_node: None,
            persistent: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_operation: Arc::new(AtomicU64::new(1)),
            turn_active: Arc::new(AtomicBool::new(false)),
            #[cfg(debug_assertions)]
            scripted_chat: None,
        })
    }

    pub fn with_local_node(
        instance_id: impl Into<String>,
        endpoint: impl Into<String>,
        node_instance_id: impl Into<String>,
    ) -> Result<Self, ControllerError> {
        let mut session = Self::new(instance_id)?;
        let endpoint = endpoint.into();
        let node_instance_id = node_instance_id.into();
        if endpoint.is_empty() || node_instance_id.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        session.local_node = Some(LocalNodeRoute {
            endpoint,
            instance_id: node_instance_id,
        });
        Ok(session)
    }

    pub fn with_storage(
        instance_id: impl Into<String>,
        data_root: impl AsRef<Path>,
        workspace: impl AsRef<Path>,
    ) -> Result<Self, ControllerError> {
        let mut session = Self::new(instance_id)?;
        session.attach_storage(data_root.as_ref(), workspace.as_ref())?;
        Ok(session)
    }

    pub fn with_local_node_and_storage(
        instance_id: impl Into<String>,
        endpoint: impl Into<String>,
        node_instance_id: impl Into<String>,
        data_root: impl AsRef<Path>,
        workspace: impl AsRef<Path>,
    ) -> Result<Self, ControllerError> {
        let mut session = Self::with_local_node(instance_id, endpoint, node_instance_id)?;
        session.attach_storage(data_root.as_ref(), workspace.as_ref())?;
        Ok(session)
    }

    fn attach_storage(
        &mut self,
        data_root: &Path,
        workspace: &Path,
    ) -> Result<(), ControllerError> {
        let workspace = workspace.canonicalize()?;
        let workspace_store = WorkspaceStore::open(data_root)?;
        let workspace_key = workspace_store.workspace_key(&workspace)?;
        let preferences = workspace_store.load(&workspace)?.unwrap_or_default();
        self.persistent = Some(Arc::new(Mutex::new(PersistentControllerState {
            session_store: SessionStore::open(data_root)?,
            workspace_store,
            workspace,
            workspace_key,
            preferences,
            active: None,
        })));
        Ok(())
    }

    #[cfg(debug_assertions)]
    pub fn with_scripted_chat(
        instance_id: impl Into<String>,
        events: Vec<RuntimeEventEnvelope>,
    ) -> Result<Self, ControllerError> {
        if events.is_empty() {
            return Err(ControllerError::InvalidMessage);
        }
        let mut session = Self::new(instance_id)?;
        session.scripted_chat = Some(
            events
                .into_iter()
                .map(|event| {
                    serde_json::to_value(event).map_err(|_| ControllerError::InvalidMessage)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(session)
    }

    pub fn accept_hello(&self, hello: HelloClient) -> Result<HelloServer, ControllerError> {
        if hello.protocol_version() != pinvou_protocol::IPC_VERSION {
            return Err(ControllerError::ProtocolMismatch);
        }
        HelloServer::new(self.instance_id.clone()).map_err(|_| ControllerError::InvalidMessage)
    }

    pub fn handle(&self, request: IpcMessage) -> Result<IpcMessage, ControllerError> {
        let mut responses = self.handle_many(request)?;
        if responses.len() != 1 {
            return Err(ControllerError::UnsupportedRequest);
        }
        Ok(responses.remove(0))
    }

    pub fn handle_many(&self, request: IpcMessage) -> Result<Vec<IpcMessage>, ControllerError> {
        if request.kind() != IpcMessageKind::Req {
            return Err(ControllerError::UnsupportedRequest);
        }
        let id = request
            .id()
            .cloned()
            .ok_or(ControllerError::InvalidMessage)?;
        match request.method() {
            Some("health") => Ok(vec![
                IpcMessage::response(
                    id,
                    json!({
                        "status": "ok",
                        "instance_id": self.instance_id,
                        "protocol_version": pinvou_protocol::IPC_VERSION
                    }),
                )
                .map_err(|_| ControllerError::InvalidMessage)?,
            ]),
            Some("runtime.echo") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let text = request
                    .payload()
                    .get("text")
                    .and_then(|v| v.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let envelope = client.echo(text)?;
                Ok(vec![
                    IpcMessage::event(
                        "runtime.event",
                        serde_json::to_value(&envelope)
                            .map_err(|_| ControllerError::InvalidMessage)?,
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("runtime.detect") => Ok(vec![
                IpcMessage::response(
                    id,
                    if let Some(route) = &self.local_node {
                        let mut client =
                            LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                        let runtime = request
                            .payload()
                            .get("runtime")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.is_empty());
                        client.runtime_detect(runtime)?.payload().clone()
                    } else {
                        json!({
                            "status": "unavailable",
                            "runtime": "none",
                            "protocol_version": pinvou_protocol::IPC_VERSION
                        })
                    },
                )
                .map_err(|_| ControllerError::InvalidMessage)?,
            ]),
            Some("runtime.list") => Ok(vec![
                IpcMessage::response(
                    id,
                    if let Some(route) = &self.local_node {
                        let mut client =
                            LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                        client.runtime_list()?.payload().clone()
                    } else {
                        json!({"current":"none", "runtimes":[]})
                    },
                )
                .map_err(|_| ControllerError::InvalidMessage)?,
            ]),
            Some("runtime.switch") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.runtime_switch(runtime)?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("runtime.switch.prepare") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.runtime_switch_prepare(runtime)?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("runtime.switch.commit") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let runtime = request
                    .payload()
                    .get("runtime")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let switch_token = request
                    .payload()
                    .get("switch_token")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.runtime_switch_commit(runtime, switch_token)?;
                if response
                    .payload()
                    .get("status")
                    .and_then(|value| value.as_str())
                    == Some("ok")
                {
                    self.commit_runtime_attachment_switch(runtime)?;
                }
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("session.list") => {
                let sessions = self.list_sessions(request.payload())?;
                Ok(vec![
                    IpcMessage::response(id, json!({"sessions":sessions}))
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("session.resume.prepare") => {
                self.ensure_turn_idle()?;
                let session_id = request
                    .payload()
                    .get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ControllerError::InvalidMessage)?;
                let logical_id = LogicalSessionId::new(session_id)
                    .map_err(|_| ControllerError::InvalidMessage)?;
                let metadata = self.load_or_import_session(&logical_id)?;
                let native_session_id = metadata
                    .descriptor
                    .native_session_id
                    .clone()
                    .ok_or(ControllerError::InvalidMessage)?;
                let model_id = metadata
                    .descriptor
                    .model_id
                    .as_ref()
                    .map(|model| model.as_str().to_owned());
                let reasoning_level = self
                    .persistent
                    .as_ref()
                    .and_then(|persistent| persistent.lock().ok())
                    .and_then(|state| {
                        state
                            .preferences
                            .reasoning_level_by_runtime
                            .get(&metadata.descriptor.runtime_id)
                            .cloned()
                    });
                let token = self.prepare_token(PreparedOperation::Resume {
                    logical_id: logical_id.clone(),
                    native_session_id,
                    attachment_epoch: metadata.attachment_epoch,
                    model_id,
                    reasoning_level,
                })?;
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({
                            "status":"ready",
                            "session_id":logical_id,
                            "attachment_epoch":metadata.attachment_epoch,
                            "resume_token":token
                        }),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("session.resume.commit") => {
                self.ensure_turn_idle()?;
                let token = request
                    .payload()
                    .get("resume_token")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ControllerError::InvalidMessage)?
                    .to_owned();
                let prepared = self
                    .pending
                    .lock()
                    .map_err(|_| ControllerError::InvalidMessage)?
                    .get(&token)
                    .cloned()
                    .ok_or(ControllerError::InvalidMessage)?;
                let PreparedOperation::Resume {
                    logical_id,
                    native_session_id,
                    attachment_epoch,
                    model_id,
                    reasoning_level,
                } = prepared
                else {
                    return Err(ControllerError::InvalidMessage);
                };
                let persistent = self
                    .persistent
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let (profile, current_metadata) = {
                    let state = persistent
                        .lock()
                        .map_err(|_| ControllerError::InvalidMessage)?;
                    (
                        state.preferences.approval_profile,
                        state.session_store.metadata(&logical_id)?,
                    )
                };
                if current_metadata.attachment_epoch != attachment_epoch {
                    return Err(ControllerError::InvalidMessage);
                }
                let full_access_confirmed = request
                    .payload()
                    .get("full_access_confirmed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if profile == ApprovalProfile::FullAccess && !full_access_confirmed {
                    return Err(ControllerError::InvalidMessage);
                }
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let resumed = client.session_resume(
                    &native_session_id,
                    model_id.as_deref(),
                    reasoning_level.as_deref(),
                    approval_profile_name(profile),
                    full_access_confirmed,
                )?;
                let new_epoch = resumed
                    .payload()
                    .get("attachment_epoch")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or(ControllerError::InvalidMessage)?;
                let attachment_id = resumed
                    .payload()
                    .get("attachment_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ControllerError::InvalidMessage)?
                    .to_owned();
                let (snapshot, runtime_id) = {
                    let mut state = persistent
                        .lock()
                        .map_err(|_| ControllerError::InvalidMessage)?;
                    let mut metadata = current_metadata;
                    metadata.attachment_epoch = new_epoch;
                    state.session_store.update_metadata(metadata.clone())?;
                    let snapshot = state.session_store.restore(&logical_id)?;
                    let runtime_id = metadata.descriptor.runtime_id.clone();
                    state.active = Some(ActiveSession {
                        logical_id: logical_id.clone(),
                        native_session_id,
                        attachment_id: Some(attachment_id),
                        attachment_epoch: new_epoch,
                        runtime_id: runtime_id.clone(),
                        model_id: model_id.clone(),
                        reasoning_level: reasoning_level.clone(),
                        approval_profile: profile,
                    });
                    state.preferences.recent_session = Some(logical_id.clone());
                    state.preferences.runtime = Some(runtime_id.clone());
                    if let Some(model_id) = model_id.as_ref() {
                        state
                            .preferences
                            .model_by_runtime
                            .insert(runtime_id.clone(), model_id.clone());
                    }
                    if let Some(reasoning_level) = reasoning_level.as_ref() {
                        state
                            .preferences
                            .reasoning_level_by_runtime
                            .insert(runtime_id.clone(), reasoning_level.clone());
                    }
                    state
                        .workspace_store
                        .save(&state.workspace, &state.preferences)?;
                    (snapshot, runtime_id)
                };
                self.pending
                    .lock()
                    .map_err(|_| ControllerError::InvalidMessage)?
                    .remove(&token);
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({
                            "status":"ok",
                            "session_id":logical_id,
                            "snapshot":snapshot,
                            "runtime":runtime_id,
                            "model_id":model_id,
                            "approval_profile":profile,
                            "attachment_epoch":new_epoch
                        }),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("model.list") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let active = self.current_active()?;
                let remembered = self
                    .persistent
                    .as_ref()
                    .and_then(|persistent| persistent.lock().ok())
                    .and_then(|state| {
                        let runtime = state.preferences.runtime.as_ref()?;
                        Some((
                            state.preferences.model_by_runtime.get(runtime).cloned(),
                            state
                                .preferences
                                .reasoning_level_by_runtime
                                .get(runtime)
                                .cloned(),
                        ))
                    })
                    .unwrap_or((None, None));
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.model_list(
                    active
                        .as_ref()
                        .and_then(|active| active.model_id.as_deref())
                        .or(remembered.0.as_deref()),
                    active
                        .as_ref()
                        .and_then(|active| active.reasoning_level.as_deref())
                        .or(remembered.1.as_deref()),
                )?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("model.credential.set") => {
                self.ensure_turn_idle()?;
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let model_id = request
                    .payload()
                    .get("model_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let api_key = request
                    .payload()
                    .get("api_key")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.model_credential_set(model_id, api_key)?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("model.switch.prepare") => {
                self.ensure_turn_idle()?;
                let model_id = request
                    .payload()
                    .get("model_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?
                    .to_owned();
                let reasoning_level = request
                    .payload()
                    .get("reasoning_level")
                    .map(|value| {
                        value
                            .as_str()
                            .filter(|level| !level.is_empty())
                            .map(str::to_owned)
                            .ok_or(ControllerError::InvalidMessage)
                    })
                    .transpose()?;
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let active = self.current_active()?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.model_list(
                    active
                        .as_ref()
                        .and_then(|active| active.model_id.as_deref()),
                    active
                        .as_ref()
                        .and_then(|active| active.reasoning_level.as_deref()),
                )?;
                let models = response
                    .payload()
                    .pointer("/catalog/models")
                    .and_then(serde_json::Value::as_array)
                    .ok_or(ControllerError::InvalidMessage)?;
                let selected_model = models.iter().find(|model| {
                    model.get("id").and_then(serde_json::Value::as_str) == Some(&model_id)
                        && model.get("available").and_then(serde_json::Value::as_bool) == Some(true)
                });
                let Some(selected_model) = selected_model else {
                    return Err(ControllerError::InvalidMessage);
                };
                if let Some(level) = reasoning_level.as_deref()
                    && !selected_model
                        .get("supported_reasoning_levels")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|levels| {
                            levels.iter().any(|item| item.as_str() == Some(level))
                        })
                {
                    return Err(ControllerError::InvalidMessage);
                }
                let catalog_runtime_id = response
                    .payload()
                    .pointer("/catalog/runtime_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("codewhale")
                    .to_owned();
                let runtime_id = active
                    .as_ref()
                    .map(|active| active.runtime_id.clone())
                    .unwrap_or(catalog_runtime_id);
                let token = self.prepare_token(PreparedOperation::Model {
                    logical_id: active.as_ref().map(|active| active.logical_id.clone()),
                    attachment_epoch: active.as_ref().map_or(0, |active| active.attachment_epoch),
                    runtime_id,
                    model_id: model_id.clone(),
                    reasoning_level: reasoning_level.clone(),
                })?;
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({"status":"ready","model_id":model_id,"reasoning_level":reasoning_level,"switch_token":token}),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("model.switch.commit") => {
                self.ensure_turn_idle()?;
                let token = switch_token(request.payload())?;
                let prepared = self.prepared_operation(&token)?;
                let PreparedOperation::Model {
                    logical_id,
                    attachment_epoch,
                    runtime_id,
                    model_id,
                    reasoning_level,
                } = prepared
                else {
                    return Err(ControllerError::InvalidMessage);
                };
                let active = self.current_active()?;
                validate_prepared_active(&active, logical_id.as_ref(), attachment_epoch)?;
                let new_epoch = if let Some(active) = active.as_ref() {
                    let route = self
                        .local_node
                        .as_ref()
                        .ok_or(ControllerError::UnsupportedRequest)?;
                    let full_access = active.approval_profile == ApprovalProfile::FullAccess;
                    let full_access_confirmed = request
                        .payload()
                        .get("full_access_confirmed")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    if full_access && !full_access_confirmed {
                        return Err(ControllerError::InvalidMessage);
                    }
                    let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                    let response = client.session_resume(
                        &active.native_session_id,
                        Some(&model_id),
                        reasoning_level.as_deref(),
                        approval_profile_name(active.approval_profile),
                        full_access_confirmed,
                    )?;
                    Some(
                        response
                            .payload()
                            .get("attachment_epoch")
                            .and_then(serde_json::Value::as_u64)
                            .ok_or(ControllerError::InvalidMessage)?,
                    )
                } else {
                    None
                };
                self.commit_model_selection(
                    active,
                    &runtime_id,
                    &model_id,
                    reasoning_level.as_deref(),
                    new_epoch,
                    request.payload(),
                )?;
                self.remove_prepared(&token)?;
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({"status":"ok","model_id":model_id,"reasoning_level":reasoning_level,"attachment_epoch":new_epoch}),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("permissions.inspect") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.permissions_inspect()?;
                let current = self
                    .persistent
                    .as_ref()
                    .map(|persistent| {
                        persistent
                            .lock()
                            .map(|state| state.preferences.approval_profile)
                            .map_err(|_| ControllerError::InvalidMessage)
                    })
                    .transpose()?
                    .unwrap_or(ApprovalProfile::Request);
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({
                            "permissions":response.payload().get("permissions").cloned().unwrap_or(serde_json::Value::Null),
                            "current_profile":current
                        }),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("permissions.switch.prepare") => {
                self.ensure_turn_idle()?;
                let profile: ApprovalProfile = serde_json::from_value(
                    request
                        .payload()
                        .get("profile")
                        .cloned()
                        .ok_or(ControllerError::InvalidMessage)?,
                )
                .map_err(|_| ControllerError::InvalidMessage)?;
                let full_access_confirmed = request
                    .payload()
                    .get("full_access_confirmed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if profile == ApprovalProfile::FullAccess && !full_access_confirmed {
                    return Err(ControllerError::InvalidMessage);
                }
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.permissions_inspect()?;
                let supported = response
                    .payload()
                    .pointer("/permissions/supported_profiles")
                    .and_then(serde_json::Value::as_array)
                    .ok_or(ControllerError::InvalidMessage)?;
                if !supported
                    .iter()
                    .any(|candidate| candidate.as_str() == Some(approval_profile_name(profile)))
                {
                    return Err(ControllerError::InvalidMessage);
                }
                let active = self.current_active()?;
                let token = self.prepare_token(PreparedOperation::Permissions {
                    logical_id: active.as_ref().map(|active| active.logical_id.clone()),
                    attachment_epoch: active.as_ref().map_or(0, |active| active.attachment_epoch),
                    profile,
                    full_access_confirmed,
                })?;
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({"status":"ready","profile":profile,"switch_token":token}),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("permissions.switch.commit") => {
                self.ensure_turn_idle()?;
                let token = switch_token(request.payload())?;
                let prepared = self.prepared_operation(&token)?;
                let PreparedOperation::Permissions {
                    logical_id,
                    attachment_epoch,
                    profile,
                    full_access_confirmed,
                } = prepared
                else {
                    return Err(ControllerError::InvalidMessage);
                };
                let active = self.current_active()?;
                validate_prepared_active(&active, logical_id.as_ref(), attachment_epoch)?;
                let new_epoch = if let Some(active) = active.as_ref() {
                    let route = self
                        .local_node
                        .as_ref()
                        .ok_or(ControllerError::UnsupportedRequest)?;
                    let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                    let response = client.session_resume(
                        &active.native_session_id,
                        active.model_id.as_deref(),
                        active.reasoning_level.as_deref(),
                        approval_profile_name(profile),
                        full_access_confirmed,
                    )?;
                    Some(
                        response
                            .payload()
                            .get("attachment_epoch")
                            .and_then(serde_json::Value::as_u64)
                            .ok_or(ControllerError::InvalidMessage)?,
                    )
                } else {
                    None
                };
                self.commit_permission_selection(active, profile, new_epoch)?;
                self.remove_prepared(&token)?;
                Ok(vec![
                    IpcMessage::response(
                        id,
                        json!({"status":"ok","profile":profile,"attachment_epoch":new_epoch}),
                    )
                    .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("approval.resolve") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let approval_id = request
                    .payload()
                    .get("approval_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let accepted = request
                    .payload()
                    .get("accepted")
                    .and_then(|value| value.as_bool())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.resolve_approval(approval_id, accepted)?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("input.resolve") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let input_id = request
                    .payload()
                    .get("input_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let value = request
                    .payload()
                    .get("value")
                    .cloned()
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.resolve_input(input_id, value)?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            Some("turn.interrupt") => {
                let route = self
                    .local_node
                    .as_ref()
                    .ok_or(ControllerError::UnsupportedRequest)?;
                let turn_id = request
                    .payload()
                    .get("turn_id")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .ok_or(ControllerError::InvalidMessage)?;
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.interrupt_turn(turn_id)?;
                Ok(vec![
                    IpcMessage::response(id, response.payload().clone())
                        .map_err(|_| ControllerError::InvalidMessage)?,
                ])
            }
            _ => Err(ControllerError::UnsupportedRequest),
        }
    }

    pub fn handle_bound(&self, request: IpcMessage) -> Result<IpcMessage, ControllerError> {
        let mut responses = self.handle_bound_many(request)?;
        if responses.len() != 1 {
            return Err(ControllerError::UnsupportedRequest);
        }
        Ok(responses.remove(0))
    }

    pub fn handle_bound_many(
        &self,
        request: IpcMessage,
    ) -> Result<Vec<IpcMessage>, ControllerError> {
        if request
            .payload()
            .get("instance_id")
            .and_then(|value| value.as_str())
            != Some(&self.instance_id)
        {
            return Err(ControllerError::ProtocolMismatch);
        }
        self.handle_many(request)
    }

    fn ensure_turn_idle(&self) -> Result<(), ControllerError> {
        if self.turn_active.load(Ordering::Acquire) {
            Err(ControllerError::InvalidMessage)
        } else {
            Ok(())
        }
    }

    fn prepare_token(&self, operation: PreparedOperation) -> Result<String, ControllerError> {
        let sequence = self.next_operation.fetch_add(1, Ordering::Relaxed);
        let token = format!("{}:{sequence}", self.instance_id);
        self.pending
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?
            .insert(token.clone(), operation);
        Ok(token)
    }

    fn prepared_operation(&self, token: &str) -> Result<PreparedOperation, ControllerError> {
        self.pending
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?
            .get(token)
            .cloned()
            .ok_or(ControllerError::InvalidMessage)
    }

    fn remove_prepared(&self, token: &str) -> Result<(), ControllerError> {
        self.pending
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?
            .remove(token)
            .map(|_| ())
            .ok_or(ControllerError::InvalidMessage)
    }

    fn current_active(&self) -> Result<Option<ActiveSession>, ControllerError> {
        let Some(persistent) = &self.persistent else {
            return Ok(None);
        };
        Ok(persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?
            .active
            .clone())
    }

    fn commit_runtime_attachment_switch(&self, runtime_id: &str) -> Result<(), ControllerError> {
        let Some(persistent) = &self.persistent else {
            return Ok(());
        };
        let mut state = persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?;
        state.preferences.runtime = Some(runtime_id.to_owned());
        if let Some(active) = state.active.as_mut() {
            active.native_session_id.clear();
            active.attachment_id = None;
            active.attachment_epoch = 1;
            active.runtime_id = runtime_id.to_owned();
            active.model_id = None;
            active.reasoning_level = None;

            let logical_id = active.logical_id.clone();
            let mut snapshot = state.session_store.restore(&logical_id)?;
            snapshot.descriptor.runtime_id = runtime_id.to_owned();
            snapshot.descriptor.model_id = None;
            snapshot.descriptor.native_session_id = None;
            snapshot.descriptor.status = SessionStatus::Active;
            state.session_store.write_snapshot(&logical_id, snapshot)?;
            let mut metadata = state.session_store.metadata(&logical_id)?;
            metadata.attachment_epoch = 1;
            state.session_store.update_metadata(metadata)?;
        }
        state
            .workspace_store
            .save(&state.workspace, &state.preferences)?;
        Ok(())
    }

    fn commit_model_selection(
        &self,
        active: Option<ActiveSession>,
        runtime_id: &str,
        model_id: &str,
        reasoning_level: Option<&str>,
        new_epoch: Option<u64>,
        _: &serde_json::Value,
    ) -> Result<(), ControllerError> {
        let persistent = self
            .persistent
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let mut state = persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?;
        state
            .preferences
            .model_by_runtime
            .insert(runtime_id.to_owned(), model_id.to_owned());
        if let Some(reasoning_level) = reasoning_level {
            state
                .preferences
                .reasoning_level_by_runtime
                .insert(runtime_id.to_owned(), reasoning_level.to_owned());
        } else {
            state
                .preferences
                .reasoning_level_by_runtime
                .remove(runtime_id);
        }
        if let Some(mut active) = active {
            let new_epoch = new_epoch.ok_or(ControllerError::InvalidMessage)?;
            active.model_id = Some(model_id.to_owned());
            active.reasoning_level = reasoning_level.map(str::to_owned);
            active.attachment_epoch = new_epoch;
            let mut metadata = state.session_store.metadata(&active.logical_id)?;
            metadata.attachment_epoch = new_epoch;
            metadata.descriptor.model_id = Some(
                pinvou_runtime_api::ModelId::new(model_id)
                    .map_err(|_| ControllerError::InvalidMessage)?,
            );
            state.session_store.update_metadata(metadata)?;
            state.active = Some(active);
        }
        state
            .workspace_store
            .save(&state.workspace, &state.preferences)?;
        Ok(())
    }

    fn commit_permission_selection(
        &self,
        active: Option<ActiveSession>,
        profile: ApprovalProfile,
        new_epoch: Option<u64>,
    ) -> Result<(), ControllerError> {
        let persistent = self
            .persistent
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let mut state = persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?;
        state.preferences.approval_profile = profile;
        if let Some(mut active) = active {
            let new_epoch = new_epoch.ok_or(ControllerError::InvalidMessage)?;
            active.approval_profile = profile;
            active.attachment_epoch = new_epoch;
            let mut metadata = state.session_store.metadata(&active.logical_id)?;
            metadata.attachment_epoch = new_epoch;
            state.session_store.update_metadata(metadata)?;
            state.active = Some(active);
        }
        state
            .workspace_store
            .save(&state.workspace, &state.preferences)?;
        Ok(())
    }

    fn list_sessions(
        &self,
        payload: &serde_json::Value,
    ) -> Result<Vec<SessionDescriptor>, ControllerError> {
        let query = payload
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut sessions = BTreeMap::<String, SessionDescriptor>::new();
        let workspace = if let Some(persistent) = &self.persistent {
            let mut state = persistent
                .lock()
                .map_err(|_| ControllerError::InvalidMessage)?;
            repair_missing_native_session_ids(&mut state)?;
            for descriptor in state.session_store.list_for_workspace(&state.workspace_key) {
                sessions.insert(descriptor.id.as_str().to_owned(), descriptor);
            }
            let canonical_native_ids = sessions
                .values()
                .filter_map(|descriptor| {
                    descriptor
                        .native_session_id
                        .as_deref()
                        .filter(|native| descriptor.id.as_str() != *native)
                        .map(|native| (native.to_owned(), descriptor.id.as_str().to_owned()))
                })
                .collect::<HashMap<_, _>>();
            for (native_id, canonical_id) in &canonical_native_ids {
                sessions.remove(native_id);
                if state
                    .preferences
                    .recent_session
                    .as_ref()
                    .is_some_and(|recent| recent.as_str() == native_id)
                {
                    state.preferences.recent_session = Some(
                        LogicalSessionId::new(canonical_id.clone())
                            .map_err(|_| ControllerError::InvalidMessage)?,
                    );
                }
            }
            if !canonical_native_ids.is_empty() {
                state
                    .workspace_store
                    .save(&state.workspace, &state.preferences)?;
            }
            state.workspace.clone()
        } else {
            std::env::current_dir()?
        };
        if let Some(route) = &self.local_node {
            let native_result = (|| {
                let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
                let response = client.session_list(&workspace)?;
                let native = response
                    .payload()
                    .get("sessions")
                    .cloned()
                    .ok_or(ControllerError::InvalidMessage)?;
                serde_json::from_value::<Vec<SessionDescriptor>>(native)
                    .map_err(|_| ControllerError::InvalidMessage)
            })();
            match native_result {
                Ok(native) => {
                    let persisted_native_ids = sessions
                        .values()
                        .filter_map(|descriptor| descriptor.native_session_id.as_deref())
                        .map(str::to_owned)
                        .collect::<HashSet<_>>();
                    for descriptor in native {
                        if persisted_native_ids.contains(descriptor.id.as_str())
                            || descriptor
                                .native_session_id
                                .as_deref()
                                .is_some_and(|id| persisted_native_ids.contains(id))
                        {
                            continue;
                        }
                        sessions
                            .entry(descriptor.id.as_str().to_owned())
                            .or_insert(descriptor);
                    }
                }
                Err(error) if sessions.is_empty() => return Err(error),
                Err(_) => {}
            }
        }
        let mut sessions = sessions
            .into_values()
            .filter(|descriptor| {
                query.is_empty()
                    || descriptor.title.to_ascii_lowercase().contains(&query)
                    || descriptor.id.as_str().to_ascii_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| right.last_active_at.cmp(&left.last_active_at));
        Ok(sessions)
    }

    fn load_or_import_session(
        &self,
        logical_id: &LogicalSessionId,
    ) -> Result<StoredSessionMetadata, ControllerError> {
        let persistent = self
            .persistent
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        {
            let state = persistent
                .lock()
                .map_err(|_| ControllerError::InvalidMessage)?;
            match state.session_store.metadata(logical_id) {
                Ok(metadata) => {
                    let _ = state.session_store.restore(logical_id)?;
                    return Ok(metadata);
                }
                Err(SessionStoreError::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let route = self
            .local_node
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
        let response = client.session_read(logical_id.as_str())?;
        let snapshot: pinvou_runtime_api::SessionSnapshot = serde_json::from_value(
            response
                .payload()
                .get("snapshot")
                .cloned()
                .ok_or(ControllerError::InvalidMessage)?,
        )
        .map_err(|_| ControllerError::InvalidMessage)?;
        if snapshot.descriptor.id != *logical_id {
            return Err(ControllerError::InvalidMessage);
        }
        let mut state = persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?;
        let metadata = StoredSessionMetadata::for_workspace(
            snapshot.descriptor.clone(),
            0,
            state.workspace_key.clone(),
        );
        state.session_store.create_session(metadata.clone())?;
        state.session_store.write_snapshot(logical_id, snapshot)?;
        Ok(metadata)
    }

    fn ensure_active_for_chat(
        &self,
        prompt: &str,
    ) -> Result<Option<(LogicalSessionId, serde_json::Value)>, ControllerError> {
        let Some(persistent) = &self.persistent else {
            return Ok(None);
        };
        if let Some(active) = self.current_active()? {
            let mut context = json!({
                "attachment_epoch":active.attachment_epoch,
                "approval_profile":approval_profile_name(active.approval_profile)
            });
            if let Some(model_id) = active.model_id.as_ref() {
                context["model_id"] = json!(model_id);
            }
            if let Some(reasoning_level) = active.reasoning_level.as_ref() {
                context["reasoning_level"] = json!(reasoning_level);
            }
            if active.native_session_id.is_empty() && active.attachment_id.is_none() {
                let snapshot = persistent
                    .lock()
                    .map_err(|_| ControllerError::InvalidMessage)?
                    .session_store
                    .restore(&active.logical_id)?;
                let items = conversation_handoff_items(&snapshot.normalized_events);
                if !items.is_empty() {
                    context["history_items"] = Value::Array(items);
                }
            }
            return Ok(Some((active.logical_id, context)));
        }
        let route = self
            .local_node
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let (workspace, workspace_key, preferences) = {
            let state = persistent
                .lock()
                .map_err(|_| ControllerError::InvalidMessage)?;
            (
                state.workspace.clone(),
                state.workspace_key.clone(),
                state.preferences.clone(),
            )
        };
        let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
        let runtime_response = client.runtime_list()?;
        let runtime_id = runtime_response
            .payload()
            .get("current")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty() && *value != "none")
            .ok_or(ControllerError::InvalidMessage)?
            .to_owned();
        let catalog = client.model_list(None, None)?;
        let models = catalog
            .payload()
            .pointer("/catalog/models")
            .and_then(serde_json::Value::as_array)
            .ok_or(ControllerError::InvalidMessage)?;
        let remembered = preferences.model_by_runtime.get(&runtime_id);
        let model_id = catalog
            .payload()
            .pointer("/catalog/current_model")
            .and_then(serde_json::Value::as_str)
            .filter(|current| {
                models.iter().any(|model| {
                    model.get("id").and_then(serde_json::Value::as_str) == Some(*current)
                        && model
                            .get("available")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                })
            })
            .map(str::to_owned)
            .or_else(|| {
                remembered
                    .filter(|remembered| {
                        models.iter().any(|model| {
                            model.get("id").and_then(serde_json::Value::as_str)
                                == Some(remembered.as_str())
                                && model
                                    .get("available")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false)
                        })
                    })
                    .cloned()
            })
            .or_else(|| {
                models
                    .iter()
                    .find(|model| {
                        model
                            .get("is_default")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                            && model
                                .get("available")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                    })
                    .and_then(|model| model.get("id"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or(ControllerError::InvalidMessage)?;
        let model_supports_level = |level: &str| {
            models
                .iter()
                .find(|model| {
                    model.get("id").and_then(serde_json::Value::as_str) == Some(&model_id)
                })
                .and_then(|model| model.get("supported_reasoning_levels"))
                .and_then(serde_json::Value::as_array)
                .is_some_and(|levels| levels.iter().any(|item| item.as_str() == Some(level)))
        };
        let reasoning_level = catalog
            .payload()
            .pointer("/catalog/current_reasoning_level")
            .and_then(serde_json::Value::as_str)
            .filter(|level| model_supports_level(level))
            .map(str::to_owned)
            .or_else(|| {
                preferences
                    .reasoning_level_by_runtime
                    .get(&runtime_id)
                    .cloned()
            })
            .filter(|level| model_supports_level(level));
        let sequence = self.next_operation.fetch_add(1, Ordering::Relaxed);
        let logical_id = LogicalSessionId::new(format!("pinvou-{}-{sequence}", self.instance_id))
            .map_err(|_| ControllerError::InvalidMessage)?;
        let descriptor = SessionDescriptor {
            id: logical_id.clone(),
            title: prompt.chars().take(80).collect(),
            last_active_at: timestamp_now(),
            runtime_id: runtime_id.clone(),
            model_id: Some(
                ModelId::new(model_id.clone()).map_err(|_| ControllerError::InvalidMessage)?,
            ),
            status: SessionStatus::Active,
            native_session_id: None,
        };
        {
            let mut state = persistent
                .lock()
                .map_err(|_| ControllerError::InvalidMessage)?;
            state
                .session_store
                .create_session(StoredSessionMetadata::for_workspace(
                    descriptor,
                    1,
                    workspace_key,
                ))?;
            state.active = Some(ActiveSession {
                logical_id: logical_id.clone(),
                native_session_id: String::new(),
                attachment_id: None,
                attachment_epoch: 1,
                runtime_id: runtime_id.clone(),
                model_id: Some(model_id.clone()),
                reasoning_level: reasoning_level.clone(),
                approval_profile: preferences.approval_profile,
            });
            state.preferences.runtime = Some(runtime_id.clone());
            state
                .preferences
                .model_by_runtime
                .insert(runtime_id, model_id.clone());
            state.workspace_store.save(&workspace, &state.preferences)?;
        }
        Ok(Some((logical_id, {
            let mut context = json!({
            "cwd":workspace,
            "attachment_epoch":1,
            "model_id":model_id,
            "approval_profile":approval_profile_name(preferences.approval_profile)
            });
            if let Some(reasoning_level) = reasoning_level {
                context["reasoning_level"] = json!(reasoning_level);
            }
            context
        })))
    }

    fn persist_runtime_event(
        &self,
        logical_id: &LogicalSessionId,
        envelope: &RuntimeEventEnvelope,
    ) -> Result<(), ControllerError> {
        let persistent = self
            .persistent
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let mut state = persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?;
        let event = serde_json::to_value(envelope).map_err(|_| ControllerError::InvalidMessage)?;
        let cursor = state.session_store.append_event(logical_id, event)?;
        let native_session_id = envelope.logical_session_id().to_owned();
        let attachment_id = envelope.attachment_id().to_owned();
        let active = {
            let active = state
                .active
                .as_mut()
                .filter(|active| &active.logical_id == logical_id)
                .ok_or(ControllerError::InvalidMessage)?;
            active.native_session_id = native_session_id.clone();
            active.attachment_id = Some(attachment_id);
            active.clone()
        };
        let mut metadata = state.session_store.metadata(logical_id)?;
        if metadata.descriptor.native_session_id.as_deref() != Some(&native_session_id) {
            metadata.descriptor.native_session_id = Some(native_session_id);
            state.session_store.update_metadata(metadata)?;
        }
        if envelope.kind() == "turn.ended" {
            let mut snapshot = state.session_store.restore(logical_id)?;
            snapshot.cursor = cursor;
            snapshot.descriptor.native_session_id = Some(active.native_session_id.clone());
            snapshot.descriptor.last_active_at = timestamp_now();
            snapshot.descriptor.status = terminal_session_status(envelope);
            state.session_store.write_snapshot(logical_id, snapshot)?;
            state.preferences.recent_session = Some(logical_id.clone());
            state
                .workspace_store
                .save(&state.workspace, &state.preferences)?;
        }
        Ok(())
    }

    fn persist_user_prompt(
        &self,
        logical_id: &LogicalSessionId,
        prompt: &str,
    ) -> Result<(), ControllerError> {
        let persistent = self
            .persistent
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let mut state = persistent
            .lock()
            .map_err(|_| ControllerError::InvalidMessage)?;
        let seq = state
            .session_store
            .restore(logical_id)?
            .cursor
            .saturating_add(1);
        let event = RuntimeEventEnvelope::from_value(json!({
            "protocol_version":1,
            "schema_version":1,
            "node_id":"controller",
            "logical_session_id":logical_id,
            "attachment_id":"controller-history",
            "work_id":null,
            "collaborative_run_id":null,
            "stream_id":StreamId::Main,
            "turn_id":format!("controller-prompt-{seq}"),
            "seq":seq,
            "source_span":null,
            "timestamp":"1970-01-01T00:00:00.000Z",
            "rate_class":RateClass::R1,
            "kind":RuntimeEventKind::MessageCompleted,
            "payload":{"role":"user","content":prompt,"item_id":format!("controller-prompt-{seq}")}
        }))
        .map_err(|_| ControllerError::InvalidMessage)?;
        state.session_store.append_event(
            logical_id,
            serde_json::to_value(event).map_err(|_| ControllerError::InvalidMessage)?,
        )?;
        Ok(())
    }

    pub fn stream_bound<F>(&self, request: IpcMessage, mut emit: F) -> Result<(), ControllerError>
    where
        F: FnMut(IpcMessage) -> Result<(), ControllerError>,
    {
        if request.kind() != IpcMessageKind::Req || request.method() != Some("chat.start") {
            return Err(ControllerError::UnsupportedRequest);
        }
        if request
            .payload()
            .get("instance_id")
            .and_then(|value| value.as_str())
            != Some(&self.instance_id)
        {
            return Err(ControllerError::ProtocolMismatch);
        }
        let prompt = request
            .payload()
            .get("prompt")
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or(ControllerError::InvalidMessage)?;
        self.turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ControllerError::InvalidMessage)?;
        let _turn_guard = TurnActivityGuard(Arc::clone(&self.turn_active));

        #[cfg(debug_assertions)]
        if let Some(events) = &self.scripted_chat {
            let terminal = events
                .last()
                .cloned()
                .ok_or(ControllerError::InvalidMessage)
                .and_then(|value| {
                    RuntimeEventEnvelope::from_value(value)
                        .map_err(|_| ControllerError::InvalidMessage)
                })?;
            if terminal.kind() != "turn.ended" {
                return Err(ControllerError::InvalidMessage);
            }
            for event in events {
                let message = IpcMessage::event("runtime.event", event.clone())
                    .map_err(|_| ControllerError::InvalidMessage)?;
                emit(message)?;
            }
            return Ok(());
        }

        let route = self
            .local_node
            .as_ref()
            .ok_or(ControllerError::UnsupportedRequest)?;
        let active = self.ensure_active_for_chat(prompt)?;
        let mut client = LocalNodeClient::connect(&route.endpoint, &route.instance_id)?;
        if let Some((logical_id, context)) = active {
            self.persist_user_prompt(&logical_id, prompt)?;
            client.stream_chat_with_context(prompt, context, |message| {
                let envelope = RuntimeEventEnvelope::from_value(message.payload().clone())
                    .map_err(|_| ControllerError::InvalidMessage)?;
                self.persist_runtime_event(&logical_id, &envelope)?;
                emit(message)
            })
        } else {
            client.stream_chat(prompt, emit)
        }
    }
}

struct TurnActivityGuard(Arc<AtomicBool>);

impl Drop for TurnActivityGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn repair_missing_native_session_ids(
    state: &mut PersistentControllerState,
) -> Result<(), ControllerError> {
    let descriptors = state.session_store.list_for_workspace(&state.workspace_key);
    for descriptor in descriptors
        .into_iter()
        .filter(|descriptor| descriptor.native_session_id.is_none())
    {
        let attachment_prefix = match descriptor.runtime_id.as_str() {
            "codex" => "codex-",
            "codewhale" => "codewhale-",
            _ => continue,
        };
        let snapshot = state.session_store.restore(&descriptor.id)?;
        let native_session_id = snapshot
            .normalized_events
            .iter()
            .rev()
            .filter_map(|event| RuntimeEventEnvelope::from_value(event.clone()).ok())
            .find(|event| {
                event.attachment_id().starts_with(attachment_prefix)
                    && !event.logical_session_id().ends_with("-pending")
            })
            .map(|event| event.logical_session_id().to_owned());
        let Some(native_session_id) = native_session_id else {
            continue;
        };
        let mut metadata = state.session_store.metadata(&descriptor.id)?;
        metadata.descriptor.native_session_id = Some(native_session_id);
        state.session_store.update_metadata(metadata)?;
    }
    Ok(())
}

fn conversation_handoff_items(events: &[Value]) -> Vec<Value> {
    const MAX_MESSAGES: usize = 16;
    const MAX_CHARS: usize = 24_000;

    let mut messages = Vec::<(String, String)>::new();
    let mut assistant = String::new();
    let mut assistant_turn = None::<String>;
    let flush_assistant = |messages: &mut Vec<(String, String)>, assistant: &mut String| {
        if !assistant.trim().is_empty() {
            messages.push(("assistant".into(), std::mem::take(assistant)));
        }
    };

    for value in events {
        let Ok(event) = RuntimeEventEnvelope::from_value(value.clone()) else {
            continue;
        };
        let payload = serde_json::from_str::<Value>(event.payload().get()).unwrap_or(Value::Null);
        match event.event_kind() {
            RuntimeEventKind::TurnStarted => {
                flush_assistant(&mut messages, &mut assistant);
                assistant_turn = event.turn_id().map(str::to_owned);
            }
            RuntimeEventKind::TurnEnded => {
                flush_assistant(&mut messages, &mut assistant);
                assistant_turn = None;
            }
            RuntimeEventKind::TextDelta
                if payload.get("role").and_then(Value::as_str) == Some("assistant") =>
            {
                let event_turn = event.turn_id().map(str::to_owned);
                if assistant_turn.is_some() && assistant_turn != event_turn {
                    flush_assistant(&mut messages, &mut assistant);
                }
                assistant_turn = event_turn;
                if let Some(content) = payload.get("content").and_then(Value::as_str) {
                    assistant.push_str(content);
                }
            }
            RuntimeEventKind::MessageCompleted => {
                let Some(role @ ("user" | "assistant")) =
                    payload.get("role").and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(content) = payload.get("content").and_then(Value::as_str) else {
                    continue;
                };
                if role == "user" {
                    flush_assistant(&mut messages, &mut assistant);
                    messages.push((role.into(), content.into()));
                } else if assistant.is_empty() {
                    messages.push((role.into(), content.into()));
                }
            }
            _ => {}
        }
    }
    flush_assistant(&mut messages, &mut assistant);

    let mut selected = Vec::new();
    let mut chars = 0;
    for (role, mut content) in messages.into_iter().rev().take(MAX_MESSAGES) {
        let available = MAX_CHARS.saturating_sub(chars);
        if available == 0 {
            break;
        }
        let content_chars = content.chars().count();
        if content_chars > available {
            content = content
                .chars()
                .skip(content_chars.saturating_sub(available))
                .collect();
        }
        chars += content.chars().count();
        let content_type = if role == "user" {
            "input_text"
        } else {
            "output_text"
        };
        selected.push(json!({
            "type":"message",
            "role":role,
            "content":[{"type":content_type,"text":content}]
        }));
    }
    selected.reverse();
    selected
}

const fn approval_profile_name(profile: ApprovalProfile) -> &'static str {
    match profile {
        ApprovalProfile::Request => "request",
        ApprovalProfile::Assisted => "assisted",
        ApprovalProfile::FullAccess => "full_access",
    }
}

fn switch_token(payload: &serde_json::Value) -> Result<String, ControllerError> {
    payload
        .get("switch_token")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(ControllerError::InvalidMessage)
}

fn validate_prepared_active(
    active: &Option<ActiveSession>,
    logical_id: Option<&LogicalSessionId>,
    attachment_epoch: u64,
) -> Result<(), ControllerError> {
    match (active, logical_id) {
        (None, None) if attachment_epoch == 0 => Ok(()),
        (Some(active), Some(logical_id))
            if &active.logical_id == logical_id && active.attachment_epoch == attachment_epoch =>
        {
            Ok(())
        }
        _ => Err(ControllerError::InvalidMessage),
    }
}

fn timestamp_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

fn terminal_session_status(envelope: &RuntimeEventEnvelope) -> SessionStatus {
    let payload = serde_json::from_str::<serde_json::Value>(envelope.payload().get())
        .unwrap_or(serde_json::Value::Null);
    match payload
        .get("end_reason")
        .and_then(serde_json::Value::as_str)
    {
        Some("completed") => SessionStatus::Completed,
        Some("interrupted" | "cancelled") => SessionStatus::Interrupted,
        Some("failed") => SessionStatus::Failed,
        _ => SessionStatus::Unknown,
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;

    fn event(kind: RuntimeEventKind, turn_id: &str, seq: u64, payload: Value) -> Value {
        serde_json::to_value(
            RuntimeEventEnvelope::from_value(json!({
                "protocol_version":1,
                "schema_version":1,
                "node_id":"node",
                "logical_session_id":"session",
                "attachment_id":"attachment",
                "work_id":null,
                "collaborative_run_id":null,
                "stream_id":if matches!(kind, RuntimeEventKind::TurnStarted | RuntimeEventKind::TurnEnded) { StreamId::Control } else { StreamId::Main },
                "turn_id":turn_id,
                "seq":seq,
                "source_span":null,
                "timestamp":"2026-08-26T00:00:00.000Z",
                "rate_class":if matches!(kind, RuntimeEventKind::TurnStarted | RuntimeEventKind::TurnEnded) { RateClass::R0 } else { RateClass::R1 },
                "kind":kind,
                "payload":payload
            }))
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn handoff_extracts_conversation_without_tools_or_duplicate_completed_text() {
        let events = vec![
            event(
                RuntimeEventKind::MessageCompleted,
                "prompt",
                1,
                json!({"role":"user","content":"How should I learn LLM engineering?","item_id":"prompt"}),
            ),
            event(
                RuntimeEventKind::TurnStarted,
                "turn",
                1,
                json!({"user_input_ref":"prompt"}),
            ),
            event(
                RuntimeEventKind::TextDelta,
                "turn",
                2,
                json!({"role":"assistant","content":"Start with inference and agents."}),
            ),
            event(
                RuntimeEventKind::MessageCompleted,
                "turn",
                3,
                json!({"role":"assistant","content":"Start with inference and agents.","item_id":"answer"}),
            ),
            event(
                RuntimeEventKind::ToolCallCompleted,
                "turn",
                4,
                json!({"tool_id":"tool","result":"secret output","is_error":false}),
            ),
            event(
                RuntimeEventKind::TurnEnded,
                "turn",
                2,
                json!({"end_reason":"completed"}),
            ),
        ];

        let items = conversation_handoff_items(&events);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[1]["role"], "assistant");
        assert_eq!(
            items[1]["content"][0]["text"],
            "Start with inference and agents."
        );
        assert!(
            !items
                .iter()
                .any(|item| item.to_string().contains("secret output"))
        );
    }
}
