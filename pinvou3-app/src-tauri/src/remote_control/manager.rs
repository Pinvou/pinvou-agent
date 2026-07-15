use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::protocol::{
    envelope, MobileAction, RemoteControlStatus, RemoteControlStatusKind, RemotePairingInfo,
};
use super::relay_client::{self, RelayInbound, RelayOutbound, RelaySender};
use super::snapshot;
use crate::bridge::mode_state::SerializableMode;
use crate::bridge::prefs::{SavedModel, UserPrefs};
use crate::bridge::{paths, sessions::SessionStore};
use crate::connector_cli;
use crate::engine_pool::EnginePool;

const PREVIEW_LIMIT_BYTES: usize = 256 * 1024;
const DEFAULT_PUBLIC_BASE_URL: &str = "https://pinvou.com/pinvou3/remote";
const DEFAULT_RELAY_WS_URL: &str = "wss://pinvou.com/pinvou3/remote/ws";

#[derive(Clone)]
pub struct RemoteControlManager {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    room: Option<ActiveRoom>,
    seen: Dedup,
}

#[derive(Clone)]
struct ActiveRoom {
    room_id: String,
    session_id: String,
    url: String,
    relay_ws_url: String,
    status: RemoteControlStatusKind,
    last_error: Option<String>,
    sender: RelaySender,
}

#[derive(Default)]
struct Dedup {
    ids: HashSet<String>,
    order: VecDeque<(String, Instant)>,
}

impl Dedup {
    fn remember(&mut self, id: &str) -> bool {
        self.prune();
        if self.ids.contains(id) {
            return false;
        }
        self.ids.insert(id.to_string());
        self.order.push_back((id.to_string(), Instant::now()));
        while self.order.len() > 200 {
            if let Some((old, _)) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
        true
    }

    fn prune(&mut self) {
        let cutoff = Instant::now() - Duration::from_secs(600);
        while let Some((_, at)) = self.order.front() {
            if *at > cutoff {
                break;
            }
            if let Some((old, _)) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }
}

impl RemoteControlManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn start(
        &self,
        session_id: String,
        store: SessionStore,
        pool: EnginePool,
    ) -> Result<RemotePairingInfo, String> {
        self.close_current("qr_refreshed");
        let relay_ws_url = remote_relay_ws_url();
        let public_base = remote_public_base_url();
        let room_id = format!("rc_{}", crate::remote_control::short_token(18));
        let pairing_token = crate::remote_control::short_token(32);
        let desktop_secret = crate::remote_control::short_token(32);
        // 二维码与当前 room 同寿命：只有刷新二维码、停止远控或关闭桌面端才失效。
        let url = format!(
            "{}/r/{}#token={}",
            public_base.trim_end_matches('/'),
            room_id,
            pairing_token
        );
        let qr_data_url = connector_cli::make_qr(&url);
        let (sender, mut receiver) = relay_client::spawn(
            relay_ws_url.clone(),
            room_id.clone(),
            session_id.clone(),
            pairing_token,
            desktop_secret,
        );

        {
            let mut inner = self.inner.lock();
            inner.seen = Dedup::default();
            inner.room = Some(ActiveRoom {
                room_id: room_id.clone(),
                session_id: session_id.clone(),
                url: url.clone(),
                relay_ws_url: relay_ws_url.clone(),
                status: RemoteControlStatusKind::ConnectingRelay,
                last_error: None,
                sender: sender.clone(),
            });
        }
        self.emit_status();

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(inbound) = receiver.recv().await {
                match inbound {
                    RelayInbound::MobileAction {
                        room_id,
                        session_id,
                        payload,
                    } => {
                        manager
                            .handle_mobile_action(&room_id, &session_id, payload, &store, &pool)
                            .await;
                    }
                    RelayInbound::Status {
                        room_id,
                        status,
                        message,
                    } => manager.update_status_from_relay(&room_id, &status, message),
                    RelayInbound::Error(err) => manager.set_error(err),
                }
            }
        });

        Ok(RemotePairingInfo {
            room_id,
            session_id,
            url,
            qr_data_url,
            status: RemoteControlStatusKind::WaitingMobile,
        })
    }

    pub fn stop_current(&self) {
        self.close_current("stopped");
    }

    fn close_current(&self, reason: &str) {
        let old = {
            let mut inner = self.inner.lock();
            inner.room.take()
        };
        if let Some(room) = old {
            let _ = room.sender.send(RelayOutbound::Close {
                room_id: room.room_id,
                session_id: room.session_id,
                reason: reason.to_string(),
            });
            self.emit_status();
        }
    }

    pub fn status(&self) -> RemoteControlStatus {
        let inner = self.inner.lock();
        if let Some(room) = &inner.room {
            RemoteControlStatus {
                active: true,
                room_id: Some(room.room_id.clone()),
                session_id: if room.session_id.is_empty() {
                    None
                } else {
                    Some(room.session_id.clone())
                },
                url: Some(room.url.clone()),
                status: room.status,
                relay_url: room.relay_ws_url.clone(),
                last_error: room.last_error.clone(),
            }
        } else {
            RemoteControlStatus {
                active: false,
                room_id: None,
                session_id: None,
                url: None,
                status: RemoteControlStatusKind::Idle,
                relay_url: remote_relay_ws_url(),
                last_error: None,
            }
        }
    }

    pub fn forward_local_event(&self, source_event: &str, payload: Value) {
        let Some(session_id) = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
        else {
            return;
        };
        let mapped = match source_event {
            "chat:delta" => "assistant_delta",
            "chat:tool_start" => "tool_call_start",
            "chat:tool_end" => "tool_call_end",
            "chat:plan_snapshot" => "plan_snapshot",
            "chat:plan_ready" => "plan_ready",
            "chat:user_input_required" => "user_input_required",
            "chat:transient_error" => "error",
            "chat:done" => "session_status",
            "chat:usage" => "usage_update",
            "chat:compaction" => "compaction_update",
            "artifact:disk" => "artifact_summary",
            _ => return,
        };
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        };
        if let Some(room) = room {
            let _ = room.sender.send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                mapped,
                payload,
            )));
        }
    }

    async fn handle_mobile_action(
        &self,
        room_id: &str,
        session_id: &str,
        payload: Value,
        store: &SessionStore,
        pool: &EnginePool,
    ) {
        let Ok(action) = serde_json::from_value::<MobileAction>(payload) else {
            self.send_error("bad_mobile_action", "invalid mobile action payload");
            return;
        };
        let active_session_id = match self.active_session_for_room(room_id) {
            Some(active_session_id) => active_session_id,
            None => {
                self.send_error("room_mismatch", "mobile action room mismatch");
                return;
            }
        };
        if session_id != active_session_id {
            eprintln!(
                "[remote-control] mobile action session mismatch ignored: relay={session_id}, active={active_session_id}"
            );
        }
        if !self.room_exists(room_id) {
            self.send_error("room_mismatch", "mobile action room/session mismatch");
            return;
        }
        if !matches!(
            action.kind.as_str(),
            "ping"
                | "request_snapshot"
                | "request_session_list"
                | "request_artifacts"
                | "request_artifact_preview"
                | "request_chips"
                | "disconnect"
        ) {
            let Some(id) = action.client_message_id.as_deref() else {
                self.send_error("missing_client_message_id", "client_message_id required");
                return;
            };
            if !self.inner.lock().seen.remember(id) {
                return;
            }
        }
        if active_session_id.is_empty()
            && !matches!(
                action.kind.as_str(),
                "ping"
                    | "request_snapshot"
                    | "request_session_list"
                    | "create_remote_session"
                    | "switch_remote_session"
                    | "disconnect"
            )
        {
            self.send_error("session_required", "please select a session first");
            return;
        }

        let result = match action.kind.as_str() {
            "user_message" => {
                let content = action
                    .payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if content.is_empty() {
                    Err("empty message".to_string())
                } else {
                    self.app
                        .emit(
                            "remote_control:mobile_user_message",
                            json!({
                                "session_id": active_session_id,
                                "content": content,
                                "client_message_id": action.client_message_id,
                            }),
                        )
                        .map_err(|e| format!("emit mobile_user_message: {e}"))
                }
            }
            "cancel_generation" => {
                pool.cancel(&active_session_id).await;
                Ok(())
            }
            "submit_user_input" => {
                let tool_call_id = action
                    .payload
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing tool_call_id".to_string())
                    .map(ToString::to_string);
                let answers = action
                    .payload
                    .get("answers")
                    .cloned()
                    .ok_or_else(|| "missing answers".to_string())
                    .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
                match (tool_call_id, answers) {
                    (Ok(tool_call_id), Ok(answers)) => {
                        let response =
                            deepseek_tui::tools::user_input::UserInputResponse { answers };
                        pool.submit_user_input(&active_session_id, tool_call_id, response)
                            .await
                            .map_err(|e| format!("{e:?}"))
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e),
                }
            }
            "cancel_user_input" => {
                let tool_call_id = action
                    .payload
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing tool_call_id".to_string());
                match tool_call_id {
                    Ok(tool_call_id) => pool
                        .cancel_user_input(&active_session_id, tool_call_id.to_string())
                        .await
                        .map_err(|e| format!("{e:?}")),
                    Err(e) => Err(e),
                }
            }
            "request_snapshot" | "ping" if active_session_id.is_empty() => {
                self.send_session_list(store, "")
            }
            "request_snapshot" | "ping" => {
                self.send_snapshot_with_live_request(store, &active_session_id)
            }
            "request_session_list" => self.send_session_list(store, &active_session_id),
            "create_remote_session" => self.create_remote_session(store, pool),
            "switch_remote_session" => {
                match action.payload.get("session_id").and_then(|v| v.as_str()) {
                    Some(id) => self.switch_remote_session(store, id),
                    None => Err("missing session_id".to_string()),
                }
            }
            "request_artifacts" => self.send_artifact_list(store, &active_session_id),
            "request_artifact_preview" => {
                if let Some(id) = action.payload.get("artifact_id").and_then(|v| v.as_str()) {
                    self.send_artifact_preview(store, &active_session_id, id)
                } else if let Some(path) =
                    action.payload.get("artifact_path").and_then(|v| v.as_str())
                {
                    self.send_artifact_preview_by_path(&active_session_id, path)
                } else {
                    Err("missing artifact_id".to_string())
                }
            }
            "request_chips" => self.send_chips_snapshot(store, &active_session_id),
            "accept_plan" => {
                let plan_markdown = action
                    .payload
                    .get("plan_markdown")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match store.set_mode(&active_session_id, SerializableMode::Yolo) {
                    Ok(()) => {
                        let instruction =
                            format!("用户已批准方案,立即开始执行。方案:\n\n{plan_markdown}");
                        pool.send_user_message(
                            &active_session_id,
                            instruction,
                            SerializableMode::Yolo.to_app_mode(),
                            false,
                        )
                        .await
                        .map_err(|e| format!("accept_plan send_user_message: {e:?}"))
                        .and_then(|_| self.send_chips_snapshot(store, &active_session_id))
                    }
                    Err(error) => Err(format!("accept_plan set_mode: {error:#}")),
                }
            }
            "discard_plan" => self.send_chips_snapshot(store, &active_session_id),
            "set_mode" => {
                let mode = match action.payload.get("mode").and_then(|v| v.as_str()) {
                    Some(mode) => mode,
                    None => return self.send_error("mobile_action_failed", "missing mode"),
                };
                let mode = match mode {
                    "plan" => SerializableMode::Plan,
                    "yolo" => SerializableMode::Yolo,
                    other => {
                        return self
                            .send_error("mobile_action_failed", &format!("invalid mode: {other}"))
                    }
                };
                store
                    .set_mode(&active_session_id, mode)
                    .map_err(|error| format!("set_mode: {error:#}"))
                    .and_then(|_| self.send_chips_snapshot(store, &active_session_id))
            }
            "set_model" => {
                let model_id = action.payload.get("model_id").and_then(|v| {
                    if v.is_null() {
                        None
                    } else {
                        v.as_str().map(ToString::to_string)
                    }
                });
                if let Some(mid) = &model_id {
                    if UserPrefs::load().model_by_id(mid).is_none() {
                        return self.send_error(
                            "mobile_action_failed",
                            &format!("model not found: {mid}"),
                        );
                    }
                }
                match pool
                    .switch_session_model(&active_session_id, model_id)
                    .await
                {
                    Ok(()) => self.send_chips_snapshot(store, &active_session_id),
                    Err(error) => Err(format!("set_model: {error:#}")),
                }
            }
            "disconnect" => {
                self.stop_current();
                Ok(())
            }
            other => Err(format!("action not allowed: {other}")),
        };
        if let Err(err) = result {
            self.send_error("mobile_action_failed", &err);
        }
    }

    pub fn send_snapshot(&self, store: &SessionStore, session_id: &str) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        let snapshot = snapshot::build_session_snapshot(store, session_id)?;
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "session_snapshot",
                snapshot,
            )))
            .map_err(|e| format!("send snapshot: {e}"))?;
        self.send_session_list(store, session_id)?;
        self.send_chips_snapshot(store, session_id)?;
        self.send_artifact_list(store, session_id)
    }

    pub fn send_snapshot_with_live_request(
        &self,
        store: &SessionStore,
        session_id: &str,
    ) -> Result<(), String> {
        let result = self.send_snapshot(store, session_id);
        let _ = self.app.emit(
            "remote_control:snapshot_requested",
            json!({ "session_id": session_id }),
        );
        result
    }

    pub fn send_session_list(
        &self,
        store: &SessionStore,
        active_session_id: &str,
    ) -> Result<(), String> {
        let room = self
            .active_room()
            .ok_or_else(|| "remote control not active".to_string())?;
        let sessions = store
            .list()
            .map_err(|e| format!("list sessions: {e:?}"))?
            .into_iter()
            .map(|s| {
                let id = s.id.clone();
                let active = id == active_session_id;
                json!({
                    "id": id,
                    "title": s.title,
                    "updated_at": s.updated_at,
                    "message_count": s.message_count,
                    "active": active,
                })
            })
            .collect::<Vec<_>>();
        let sessions = if active_session_id.is_empty() {
            sessions
        } else {
            ensure_active_session_in_list(store, sessions, active_session_id)
        };
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "session_list",
                json!({
                    "active_session_id": active_session_id,
                    "sessions": sessions,
                }),
            )))
            .map_err(|e| format!("send session list: {e}"))
    }

    pub fn switch_remote_session(
        &self,
        store: &SessionStore,
        target_session_id: &str,
    ) -> Result<(), String> {
        let saved = store
            .load(target_session_id)
            .map_err(|e| format!("load target session({target_session_id}): {e:?}"))?;
        let room = {
            let mut inner = self.inner.lock();
            let room = inner
                .room
                .as_mut()
                .ok_or_else(|| "remote control not active".to_string())?;
            room.session_id = target_session_id.to_string();
            room.clone()
        };
        self.emit_status();
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "remote_session_switched",
                json!({
                    "session": {
                        "id": saved.metadata.id,
                        "title": saved.metadata.title,
                        "updated_at": saved.metadata.updated_at,
                        "message_count": saved.metadata.message_count,
                    }
                }),
            )))
            .map_err(|e| format!("send remote session switched: {e}"))?;
        self.send_session_list(store, target_session_id)?;
        self.send_snapshot_with_live_request(store, target_session_id)
    }

    pub fn create_remote_session(
        &self,
        store: &SessionStore,
        pool: &EnginePool,
    ) -> Result<(), String> {
        let (model, model_id) = pool.default_model_for_new_session();
        let workspace = pool.bridge.workspace.clone();
        let saved = store
            .create_new(model, model_id, workspace)
            .map_err(|e| format!("create remote session: {e:?}"))?;
        let target_session_id = saved.metadata.id.clone();
        let room = {
            let mut inner = self.inner.lock();
            let room = inner
                .room
                .as_mut()
                .ok_or_else(|| "remote control not active".to_string())?;
            room.session_id = target_session_id.clone();
            room.clone()
        };
        let session_payload = json!({
            "id": saved.metadata.id,
            "title": saved.metadata.title,
            "updated_at": saved.metadata.updated_at,
            "message_count": saved.metadata.message_count,
        });
        let _ = self.app.emit(
            "remote_control:session_created",
            json!({ "session": session_payload.clone() }),
        );
        self.emit_status();
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "remote_session_switched",
                json!({ "session": session_payload }),
            )))
            .map_err(|e| format!("send remote session created: {e}"))?;
        self.send_session_list(store, &target_session_id)?;
        self.send_snapshot_with_live_request(store, &target_session_id)
    }

    pub fn send_artifact_list(&self, store: &SessionStore, session_id: &str) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        let saved = store
            .load(session_id)
            .map_err(|e| format!("load artifacts({session_id}): {e:?}"))?;
        let artifacts = saved
            .artifacts
            .iter()
            .map(|a| {
                let path = a.storage_path.to_string_lossy().to_string();
                json!({
                    "id": a.id,
                    "basename": a.storage_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                    "path_tail": tail_path(&path),
                    "kind": format!("{:?}", a.kind),
                    "byte_size": a.byte_size,
                    "created_at": a.created_at,
                })
            })
            .collect::<Vec<_>>();
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "artifact_list",
                json!({
                    "session_id": session_id,
                    "artifacts": artifacts,
                }),
            )))
            .map_err(|e| format!("send artifact list: {e}"))
    }

    pub fn send_artifact_preview(
        &self,
        store: &SessionStore,
        session_id: &str,
        artifact_id: &str,
    ) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        let saved = store
            .load(session_id)
            .map_err(|e| format!("load artifact preview({session_id}): {e:?}"))?;
        let artifact = saved
            .artifacts
            .iter()
            .find(|a| a.id == artifact_id)
            .ok_or_else(|| format!("artifact not found: {artifact_id}"))?;
        let preview = build_artifact_preview(&artifact.storage_path)?;
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "artifact_preview",
                json!({
                    "session_id": session_id,
                    "artifact_id": artifact.id,
                    "basename": artifact.storage_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                    "path_tail": tail_path(&artifact.storage_path.to_string_lossy()),
                    "kind": format!("{:?}", artifact.kind),
                    "byte_size": artifact.byte_size,
                    "preview": preview,
                }),
            )))
            .map_err(|e| format!("send artifact preview: {e}"))
    }

    pub fn send_artifact_preview_by_path(
        &self,
        session_id: &str,
        artifact_path: &str,
    ) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        let path = resolve_session_preview_path(session_id, artifact_path)?;
        let metadata =
            std::fs::metadata(&path).map_err(|e| format!("read artifact metadata: {e}"))?;
        let preview = build_artifact_preview(&path)?;
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "artifact_preview",
                json!({
                    "session_id": session_id,
                    "artifact_id": Value::Null,
                    "basename": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                    "path_tail": tail_path(&path.to_string_lossy()),
                    "kind": display_kind_for_path(&path),
                    "byte_size": metadata.len(),
                    "preview": preview,
                }),
            )))
            .map_err(|e| format!("send artifact preview: {e}"))
    }

    pub fn send_chips_snapshot(
        &self,
        store: &SessionStore,
        session_id: &str,
    ) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        let prefs = UserPrefs::load();
        let session_model_id = store.session_model_id(session_id);
        let effective_model = session_model_id
            .as_deref()
            .and_then(|id| prefs.model_by_id(id))
            .or_else(|| prefs.active_model());
        let mode_state = store.mode_state(session_id);
        let models = prefs
            .advanced
            .saved_models
            .iter()
            .map(model_chip)
            .collect::<Vec<_>>();
        let effective_model_id = effective_model.map(|m| m.id.clone());
        let effective_model_name = effective_model.map(|m| m.name.clone());
        let global_model_id = prefs.advanced.active_model_id.clone();
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "chips_snapshot",
                json!({
                    "session_id": session_id,
                    "mode": mode_state.mode,
                    "pinvou_review_enabled": mode_state.pinvou_review_enabled,
                    "model_id": session_model_id,
                    "effective_model_id": effective_model_id,
                    "effective_model_name": effective_model_name,
                    "global_model_id": global_model_id,
                    "models": models,
                    "skill": mode_state.active_skill.as_ref().map(|skill| json!({
                        "name": skill.name,
                        "project_dir": skill.project_dir,
                    })),
                    "persona_id": mode_state.active_persona,
                    "mounted_collection": mode_state.mounted_collection,
                }),
            )))
            .map_err(|e| format!("send chips snapshot: {e}"))
    }

    pub fn publish_user_message(
        &self,
        session_id: &str,
        content: String,
        client_message_id: Option<String>,
    ) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "message_append",
                json!({
                    "role": "user",
                    "content": content,
                    "client_message_id": client_message_id,
                }),
            )))
            .map_err(|e| format!("publish user message: {e}"))
    }

    pub fn publish_desktop_event(
        &self,
        session_id: &str,
        kind: &str,
        payload: Value,
    ) -> Result<(), String> {
        let room = {
            let inner = self.inner.lock();
            inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
        }
        .ok_or_else(|| "remote control not active".to_string())?;
        room.sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                kind,
                payload,
            )))
            .map_err(|e| format!("publish desktop event: {e}"))
    }

    fn active_room(&self) -> Option<ActiveRoom> {
        self.inner.lock().room.as_ref().cloned()
    }

    fn active_session_for_room(&self, room_id: &str) -> Option<String> {
        let inner = self.inner.lock();
        inner
            .room
            .as_ref()
            .filter(|r| r.room_id == room_id)
            .map(|r| r.session_id.clone())
    }

    fn room_exists(&self, room_id: &str) -> bool {
        let inner = self.inner.lock();
        inner.room.as_ref().is_some_and(|r| r.room_id == room_id)
    }

    fn update_status_from_relay(&self, room_id: &str, status: &str, message: Option<String>) {
        {
            let mut inner = self.inner.lock();
            let Some(room) = inner.room.as_mut().filter(|r| r.room_id == room_id) else {
                return;
            };
            room.status = match status {
                "connecting_relay" => RemoteControlStatusKind::ConnectingRelay,
                "room_registered" => RemoteControlStatusKind::WaitingMobile,
                "mobile_connected" => RemoteControlStatusKind::MobileConnected,
                "mobile_disconnected" => RemoteControlStatusKind::MobileDisconnected,
                _ => room.status,
            };
            room.last_error = message;
        }
        self.emit_status();
    }

    fn set_error(&self, err: String) {
        {
            let mut inner = self.inner.lock();
            if let Some(room) = inner.room.as_mut() {
                room.status = RemoteControlStatusKind::Error;
                room.last_error = Some(err);
            }
        }
        self.emit_status();
    }

    fn send_error(&self, code: &str, message: &str) {
        let room = {
            let inner = self.inner.lock();
            inner.room.as_ref().cloned()
        };
        if let Some(room) = room {
            let _ = room.sender.send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "error",
                json!({ "code": code, "message": message }),
            )));
        }
    }

    fn emit_status(&self) {
        let _ = self.app.emit("remote_control:status", self.status());
    }
}

fn model_chip(model: &SavedModel) -> Value {
    json!({
        "id": model.id,
        "name": model.name,
        "model": model.model,
        "preset": model.preset,
    })
}

fn remote_public_base_url() -> String {
    std::env::var("PINVOU_REMOTE_PUBLIC_URL")
        .unwrap_or_else(|_| DEFAULT_PUBLIC_BASE_URL.to_string())
}

fn remote_relay_ws_url() -> String {
    std::env::var("PINVOU_REMOTE_RELAY_WS_URL").unwrap_or_else(|_| DEFAULT_RELAY_WS_URL.to_string())
}

fn ensure_active_session_in_list(
    store: &SessionStore,
    mut sessions: Vec<Value>,
    active_session_id: &str,
) -> Vec<Value> {
    if sessions
        .iter()
        .any(|s| s.get("id").and_then(|v| v.as_str()) == Some(active_session_id))
    {
        return sessions;
    }
    if let Ok(saved) = store.load(active_session_id) {
        sessions.insert(
            0,
            json!({
                "id": saved.metadata.id,
                "title": saved.metadata.title,
                "updated_at": saved.metadata.updated_at,
                "message_count": saved.metadata.message_count,
                "active": true,
            }),
        );
    }
    sessions
}

fn resolve_session_preview_path(session_id: &str, requested: &str) -> Result<PathBuf, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("missing artifact_path".to_string());
    }
    let workspace = paths::session_workspace_dir(session_id);
    let artifacts = paths::session_artifacts_dir(session_id);
    let workspace_root = workspace
        .canonicalize()
        .map_err(|e| format!("resolve session workspace: {e}"))?;
    let artifacts_root = artifacts.canonicalize().ok();
    let mut candidates = Vec::new();
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        candidates.push(requested_path.to_path_buf());
    } else {
        candidates.push(workspace.join(requested));
        candidates.push(artifacts.join(requested));
        if let Some(tail) = path_after_segment(requested, "workspace") {
            candidates.push(workspace.join(tail));
        }
        if let Some(tail) = path_after_segment(requested, "artifacts") {
            candidates.push(artifacts.join(tail));
        }
    }
    for candidate in candidates {
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !canonical.is_file() {
            continue;
        }
        if canonical.starts_with(&workspace_root)
            || artifacts_root
                .as_ref()
                .is_some_and(|root| canonical.starts_with(root))
        {
            return Ok(canonical);
        }
    }
    Err(format!(
        "artifact path not found in current session: {requested}"
    ))
}

fn path_after_segment(path: &str, segment: &str) -> Option<PathBuf> {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized.split('/').filter(|part| !part.is_empty());
    while let Some(part) = parts.next() {
        if part == segment {
            let tail = parts.collect::<Vec<_>>().join("/");
            if !tail.is_empty() {
                return Some(PathBuf::from(tail));
            }
            return None;
        }
    }
    None
}

fn display_kind_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" => "MD".to_string(),
        "html" | "htm" => "HTML".to_string(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" => "IMAGE".to_string(),
        "csv" => "CSV".to_string(),
        "json" => "JSON".to_string(),
        other if !other.is_empty() => other.to_ascii_uppercase(),
        _ => "FILE".to_string(),
    }
}

fn build_artifact_preview(path: &Path) -> Result<Value, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("read artifact metadata: {e}"))?;
    if !metadata.is_file() {
        return Err("artifact is not a file".to_string());
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = std::fs::read(path).map_err(|e| format!("read artifact file: {e}"))?;
    let truncated = bytes.len() > PREVIEW_LIMIT_BYTES;
    let slice = if truncated {
        &bytes[..PREVIEW_LIMIT_BYTES]
    } else {
        &bytes[..]
    };
    if is_text_ext(&ext) {
        let content = String::from_utf8_lossy(slice).to_string();
        let preview_type = if ext == "html" || ext == "htm" {
            "html"
        } else if ext == "md" || ext == "markdown" {
            "markdown"
        } else {
            "text"
        };
        return Ok(json!({
            "type": preview_type,
            "content": content,
            "truncated": truncated,
            "mime": mime_for_ext(&ext),
        }));
    }
    if is_image_ext(&ext) && !truncated {
        let encoded = base64::engine::general_purpose::STANDARD.encode(slice);
        return Ok(json!({
            "type": "image",
            "data_url": format!("data:{};base64,{}", mime_for_ext(&ext), encoded),
            "truncated": false,
            "mime": mime_for_ext(&ext),
        }));
    }
    Ok(json!({
        "type": "unsupported",
        "content": "",
        "truncated": truncated,
        "mime": mime_for_ext(&ext),
        "reason": if is_image_ext(&ext) { "image too large for inline preview" } else { "preview not supported for this file type" },
    }))
}

fn is_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        "md" | "markdown"
            | "txt"
            | "html"
            | "htm"
            | "json"
            | "csv"
            | "log"
            | "xml"
            | "js"
            | "ts"
            | "tsx"
            | "jsx"
            | "css"
            | "py"
            | "rs"
            | "toml"
            | "yaml"
            | "yml"
            | "svg"
    )
}

fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp")
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "js" => "text/javascript",
        "ts" | "tsx" | "jsx" => "text/plain",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "text/plain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;
    use std::ffi::OsString;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct PinvouHomeOverride {
        previous: Option<OsString>,
        root: PathBuf,
    }

    impl PinvouHomeOverride {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "pinvou3-remote-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0)
            ));
            let previous = std::env::var_os("PINVOU3_HOME");
            std::env::set_var("PINVOU3_HOME", &root);
            Self { previous, root }
        }
    }

    impl Drop for PinvouHomeOverride {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => std::env::set_var("PINVOU3_HOME", previous),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn write_file(path: &Path, contents: &[u8]) {
        std::fs::create_dir_all(path.parent().expect("test file parent")).expect("create parent");
        std::fs::write(path, contents).expect("write test file");
    }

    #[test]
    fn dedup_rejects_duplicates_and_evicts_the_oldest_id() {
        let mut dedup = Dedup::default();
        assert!(dedup.remember("same-message"));
        assert!(!dedup.remember("same-message"));

        let mut capacity = Dedup::default();
        for index in 0..=200 {
            assert!(capacity.remember(&format!("message-{index}")));
        }
        assert_eq!(capacity.ids.len(), 200);
        assert!(
            capacity.remember("message-0"),
            "oldest id should be evicted"
        );
        assert!(!capacity.remember("message-200"), "newest id must remain");
    }

    #[test]
    fn dedup_accepts_an_id_again_after_expiry() {
        let mut dedup = Dedup::default();
        dedup.ids.insert("expired".to_string());
        dedup.order.push_back((
            "expired".to_string(),
            Instant::now() - Duration::from_secs(601),
        ));

        assert!(dedup.remember("expired"));
        assert!(!dedup.remember("expired"));
    }

    #[test]
    fn session_preview_path_allows_current_session_and_rejects_escape() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = PinvouHomeOverride::new("preview-path");
        let current_session = "session-current";
        let other_session = "session-other";
        let workspace_file = paths::session_workspace_dir(current_session).join("reports/daily.md");
        let artifact_file = paths::session_artifacts_dir(current_session).join("chart.json");
        let other_file = paths::session_workspace_dir(other_session).join("secret.md");
        let outside_file = home.root.join("outside.txt");
        write_file(&workspace_file, b"workspace report");
        write_file(&artifact_file, br#"{"ok":true}"#);
        write_file(&other_file, b"other session secret");
        write_file(&outside_file, b"outside home");

        assert_eq!(
            resolve_session_preview_path(current_session, "reports/daily.md").expect("workspace"),
            workspace_file
                .canonicalize()
                .expect("canonical workspace file")
        );
        assert_eq!(
            resolve_session_preview_path(current_session, "artifacts/chart.json")
                .expect("artifacts path tail"),
            artifact_file
                .canonicalize()
                .expect("canonical artifact file")
        );
        assert!(resolve_session_preview_path(
            current_session,
            other_file.to_str().expect("utf-8 test path")
        )
        .is_err());
        assert!(resolve_session_preview_path(
            current_session,
            "../session-other/workspace/secret.md"
        )
        .is_err());
        assert!(resolve_session_preview_path(
            current_session,
            outside_file.to_str().expect("utf-8 test path")
        )
        .is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let link = paths::session_workspace_dir(current_session).join("outside-link.txt");
            symlink(&outside_file, &link).expect("create escape symlink");
            assert!(resolve_session_preview_path(current_session, "outside-link.txt").is_err());
        }
    }

    #[test]
    fn artifact_preview_reports_kind_and_truncation() {
        let root = std::env::temp_dir().join(format!(
            "pinvou3-remote-preview-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let markdown = root.join("result.md");
        let oversized = root.join("large.txt");
        let binary = root.join("archive.bin");
        write_file(&markdown, b"# Remote result\n");
        write_file(&oversized, &vec![b'x'; PREVIEW_LIMIT_BYTES + 17]);
        write_file(&binary, &[0, 1, 2, 3]);

        let markdown_preview = build_artifact_preview(&markdown).expect("markdown preview");
        assert_eq!(markdown_preview["type"], "markdown");
        assert_eq!(markdown_preview["mime"], "text/markdown");
        assert_eq!(markdown_preview["truncated"], false);

        let large_preview = build_artifact_preview(&oversized).expect("large text preview");
        assert_eq!(large_preview["type"], "text");
        assert_eq!(large_preview["truncated"], true);
        assert_eq!(
            large_preview["content"]
                .as_str()
                .expect("preview content")
                .len(),
            PREVIEW_LIMIT_BYTES
        );

        let binary_preview = build_artifact_preview(&binary).expect("binary preview");
        assert_eq!(binary_preview["type"], "unsupported");
        assert_eq!(binary_preview["truncated"], false);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_segment_tail_accepts_slashes_from_both_platforms() {
        assert_eq!(
            path_after_segment("sessions/s1/workspace/reports/a.md", "workspace"),
            Some(PathBuf::from("reports/a.md"))
        );
        assert_eq!(
            path_after_segment(r"sessions\s1\artifacts\chart.json", "artifacts"),
            Some(PathBuf::from("chart.json"))
        );
        assert_eq!(path_after_segment("workspace", "workspace"), None);
    }
}

fn tail_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts = normalized.split('/').rev().take(3).collect::<Vec<_>>();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}
