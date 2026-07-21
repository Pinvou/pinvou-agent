use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use super::protocol::{
    envelope, MobileAction, RemoteControlStatus, RemoteControlStatusKind, RemotePairingInfo,
};
use super::relay_client::{self, DownloadSender, RelayInbound, RelayOutbound, RelaySender};
use super::snapshot;
use crate::bridge::mode_state::SerializableMode;
use crate::bridge::prefs::{SavedModel, UserPrefs};
use crate::bridge::{paths, sessions::SessionStore};
use crate::connector_cli;
use crate::engine_pool::EnginePool;

const PREVIEW_LIMIT_BYTES: usize = 256 * 1024;
// 远程下载单文件上限，避免把超大文件一次性读进内存并经 relay 转发。
const DOWNLOAD_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
// 每块原始字节数；base64 后约 1MiB，低于 relay 默认 4MiB 的 WS payload 上限。
const DOWNLOAD_CHUNK_BYTES: usize = 768 * 1024;
// 远程上传单文件上限,与下载对称,防止一次性把超大文件读进内存。
const UPLOAD_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
// 上传分块原始字节数;与下载保持一致(base64 后约 1MiB)。
const UPLOAD_CHUNK_BYTES: usize = 768 * 1024;
// 上传 ACK 等待超时:超过即视为中继或对端异常,主动收尾。
const UPLOAD_ACK_TIMEOUT_SECS: u64 = 60;
// 上传分块通道容量:dispatch 投递 → streaming task 消费写盘的有界通道,提供背压。
const UPLOAD_CHANNEL_CAPACITY: usize = 4;
const DEFAULT_PUBLIC_BASE_URL: &str = "https://pinvou.com/pinvou3/remote";
const DEFAULT_RELAY_WS_URL: &str = "wss://pinvou.com/pinvou3/remote/ws";

#[derive(Clone)]
pub struct RemoteControlManager {
    // 仅测试构造(new_headless)时为 None:headless 场景没有 Tauri runtime,emit 全部降级为 no-op。
    app: Option<AppHandle>,
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    room: Option<ActiveRoom>,
    seen: Dedup,
    /// 同一房间同时只允许一个下载任务(值为 download_id),防止重复点击并行堆叠大文件传输。
    active_download: Option<String>,
    download_ack_sender: Option<tokio::sync::mpsc::UnboundedSender<DownloadRelayAck>>,
    /// 同一房间同时只允许一个上传任务(值为 upload_id),防止并发堆叠。
    active_upload: Option<String>,
    /// 上传分块通道:dispatch 把 chunk 投到这里,streaming task 消费写盘。有界,背压来源之一。
    upload_chunk_sender: Option<tokio::sync::mpsc::Sender<UploadChunkMsg>>,
    /// 已完成上传但尚未随消息发走的附件,等待 user_message 取用。key = upload_id。
    pending_attachments: HashMap<String, PendingAttachment>,
    /// 上传落盘根目录。每个 upload 一个子目录 `<uploads_base>/<upload_id>/data.bin`。
    uploads_base: PathBuf,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            room: None,
            seen: Dedup::default(),
            active_download: None,
            download_ack_sender: None,
            active_upload: None,
            upload_chunk_sender: None,
            pending_attachments: HashMap::new(),
            uploads_base: crate::bridge::paths::pinvou3_home().join("uploads"),
        }
    }
}

#[derive(Debug)]
struct DownloadRelayAck {
    index: usize,
    ok: bool,
    message: Option<String>,
}

#[derive(Debug)]
struct UploadChunkMsg {
    upload_id: String,
    index: usize,
    data: Vec<u8>,
    last: bool,
}

#[derive(Debug, Clone)]
struct PendingAttachment {
    session_id: String,
    filename: String,
    byte_size: u64,
    mime: String,
    /// streaming task 写盘+ingest 成功后填。user_message arm 取用时若 None 则报错。
    ingest_result: Option<crate::file_ingest::IngestResult>,
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
    download_sender: DownloadSender,
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
            app: Some(app),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// 无 Tauri runtime 的测试构造:事件 emit 降级为 no-op,relay/消息分发逻辑保持真实。
    #[cfg(test)]
    fn new_headless() -> Self {
        Self {
            app: None,
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
        let (sender, download_sender, mut receiver) = relay_client::spawn(
            relay_ws_url.clone(),
            room_id.clone(),
            session_id.clone(),
            pairing_token,
            desktop_secret,
        );

        {
            let mut inner = self.inner.lock();
            inner.seen = Dedup::default();
            inner.active_download = None;
            inner.download_ack_sender = None;
            inner.room = Some(ActiveRoom {
                room_id: room_id.clone(),
                session_id: session_id.clone(),
                url: url.clone(),
                relay_ws_url: relay_ws_url.clone(),
                status: RemoteControlStatusKind::ConnectingRelay,
                last_error: None,
                sender: sender.clone(),
                download_sender,
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
                    RelayInbound::DownloadAck {
                        download_id,
                        index,
                        ok,
                        message,
                    } => manager.handle_download_relay_ack(
                        &download_id,
                        DownloadRelayAck { index, ok, message },
                    ),
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
        // 锁内:统一清空 download / upload 全部槽位。
        // **不删源文件**(spec §5:防 LLM read_file 竞争):仅清 HashMap 项,
        // streaming task 收 None 后自中止;盘上数据由 mobile_disconnected /
        // session 切换 / streaming task timeout 后续路径清理。
        let old = {
            let mut inner = self.inner.lock();
            inner.active_download = None;
            inner.download_ack_sender = None;
            inner.active_upload = None;
            inner.upload_chunk_sender = None;
            inner.pending_attachments.clear();
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

    /// 清理当前进行中的上传:丢 sender 让 streaming task 自然终止,清 Inner 字段,
    /// best-effort 删除盘上未完成的 upload 目录。失败仅 log,不阻塞调用方。
    ///
    /// 调用方需保证**不在持锁状态**下调用 —— 本方法自己 lock,且 lock 之间无嵌套。
    fn cleanup_active_upload(&self, reason: &str) {
        let (upload_id, dir_to_remove) = {
            let mut inner = self.inner.lock();
            let Some(upload_id) = inner.active_upload.take() else {
                return;
            };
            inner.upload_chunk_sender = None;
            inner.pending_attachments.remove(&upload_id);
            let dir_to_remove = inner.uploads_base.join(&upload_id);
            (upload_id, dir_to_remove)
        };
        // 锁外做 fs 删除(best-effort):不阻塞 dispatcher,失败只 log。
        if let Err(e) = std::fs::remove_dir_all(&dir_to_remove) {
            // 不报错给上层,只在日志留痕 —— 与 attach_file_abort 的删除风格一致。
            eprintln!(
                "[remote-control] cleanup_active_upload: remove_dir_all failed for upload_id={upload_id} reason={reason} (likely already gone): {e:#}"
            );
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
                // 分块流式上传按 upload_id 关联,不走 cmid 去重通道(每块都自带 cmid 反而难复用)。
                | "attach_file_chunk"
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
                    // 工具开关是全局态,刚扫码未选会话的移动端也能查目录,不必强制选中会话。
                    | "list_tools"
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
                // 去重 + 保序:mobile 若重复传同一 upload_id,只取一次,避免附件被双发。
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let attachment_upload_ids: Vec<String> = action
                    .payload
                    .get("attachment_upload_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .filter(|id| seen.insert(id.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                if content.is_empty() && attachment_upload_ids.is_empty() {
                    Err("empty message".to_string())
                } else {
                    // 收集 attachments:每个 upload_id 必须有 ingest_result(上传已完成)。
                    // session_id 校验防 cross-session 取用别人的 pending attachment。
                    let mut attachments: Vec<crate::file_ingest::IngestResult> = Vec::new();
                    let mut missing: Vec<String> = Vec::new();
                    {
                        let mut inner = self.inner.lock();
                        for upload_id in &attachment_upload_ids {
                            match inner.pending_attachments.get_mut(upload_id) {
                                Some(p) if p.session_id == active_session_id => {
                                    match p.ingest_result.clone() {
                                        Some(ir) => attachments.push(ir),
                                        None => missing.push(upload_id.clone()),
                                    }
                                }
                                _ => missing.push(upload_id.clone()),
                            }
                        }
                        // 消费成功才删;若有 missing 不删,留给 mobile 重试或 abort。
                        if missing.is_empty() {
                            for upload_id in &attachment_upload_ids {
                                inner.pending_attachments.remove(upload_id);
                            }
                        }
                    }
                    if !missing.is_empty() {
                        Err(format!("attachments not ready: {}", missing.join(",")))
                    } else if let Some(app) = &self.app {
                        app.emit(
                            "remote_control:mobile_user_message",
                            json!({
                                "session_id": active_session_id,
                                "content": content,
                                "client_message_id": action.client_message_id,
                                "attachments": attachments,
                            }),
                        )
                        .map_err(|e| format!("emit mobile_user_message: {e}"))
                    } else {
                        Ok(())
                    }
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
                    self.send_artifact_preview_by_path(store, &active_session_id, path)
                } else {
                    Err("missing artifact_id".to_string())
                }
            }
            "request_chips" => self.send_chips_snapshot(store, &active_session_id),
            "request_artifact_download" => {
                let artifact_id = action.payload.get("artifact_id").and_then(|v| v.as_str());
                let artifact_path = action
                    .payload
                    .get("artifact_path")
                    .and_then(|v| v.as_str());
                if artifact_id.is_none() && artifact_path.is_none() {
                    Err("missing artifact_id".to_string())
                } else {
                    self.send_artifact_download(
                        store,
                        &active_session_id,
                        artifact_id,
                        artifact_path,
                    )
                }
            }
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
            // --- 知识库 (KB) 分发:list / mount / unmount ---
            "list_kb_collections" => {
                let Some(app) = &self.app else {
                    self.send_error("headless_unsupported", "knowledge service needs Tauri runtime");
                    return;
                };
                let collections = match app.try_state::<crate::knowledge::KnowledgeService>() {
                    Some(svc) => match svc.l1().list_collections() {
                        Ok(rows) => rows,
                        Err(e) => {
                            self.send_error("kb_list_failed", &format!("{e:#}"));
                            return;
                        }
                    },
                    None => Vec::new(),
                };
                let mounted = app
                    .try_state::<SessionStore>()
                    .and_then(|s| s.mounted_collection(&active_session_id));
                self.send_event(
                    &active_session_id,
                    "kb_collections_snapshot",
                    json!({
                        "collections": collections,
                        "mounted_collection_id": mounted,
                    }),
                );
                return;
            }
            "mount_kb_collection" => {
                let Some(app) = &self.app else {
                    self.send_error("headless_unsupported", "knowledge service needs Tauri runtime");
                    return;
                };
                let collection_id = action
                    .payload
                    .get("collection_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if collection_id <= 0 {
                    self.send_error("invalid_collection_id", "collection_id required");
                    return;
                }
                let (knowledge, store) = (
                    app.try_state::<crate::knowledge::KnowledgeService>(),
                    app.try_state::<SessionStore>(),
                );
                let Some(knowledge) = knowledge else {
                    self.send_error("headless_unsupported", "knowledge service not managed");
                    return;
                };
                let Some(store) = store else {
                    self.send_error("headless_unsupported", "session store not managed");
                    return;
                };
                // 完全门控:embedding 模型未就绪 → 知识库整体不可用,拒绝挂载
                // (与 commands::session_mount_collection 同源兜底,防远程绕过)。
                if !knowledge.semantic_ready() {
                    self.send_error(
                        "kb_not_ready",
                        "embedding 模型未就绪,知识库暂不可用",
                    );
                    return;
                }
                // 拒绝挂载空集合(规范 §3 决策:doc_count==0 视为空)。
                let non_empty = knowledge
                    .l1()
                    .list_collections()
                    .unwrap_or_default()
                    .iter()
                    .any(|c| c.id == collection_id && c.doc_count > 0);
                if !non_empty {
                    self.send_error("collection_empty", "cannot mount empty collection");
                    return;
                }
                store.set_mounted_collection(&active_session_id, Some(collection_id));
                let _ = app.emit(
                    "remote_control:kb_mount_changed",
                    json!({
                        "session_id": active_session_id,
                        "collection_id": collection_id,
                    }),
                );
                self.send_event(
                    &active_session_id,
                    "kb_mount_changed",
                    json!({
                        "session_id": active_session_id,
                        "collection_id": collection_id,
                    }),
                );
                return;
            }
            "unmount_kb_collection" => {
                let Some(app) = &self.app else {
                    self.send_error("headless_unsupported", "knowledge service needs Tauri runtime");
                    return;
                };
                let Some(store) = app.try_state::<SessionStore>() else {
                    self.send_error("headless_unsupported", "session store not managed");
                    return;
                };
                store.set_mounted_collection(&active_session_id, None);
                let _ = app.emit(
                    "remote_control:kb_mount_changed",
                    json!({
                        "session_id": active_session_id,
                        "collection_id": null,
                    }),
                );
                self.send_event(
                    &active_session_id,
                    "kb_mount_changed",
                    json!({
                        "session_id": active_session_id,
                        "collection_id": Value::Null,
                    }),
                );
                return;
            }
            // --- 工具开关 (tools) 分发:list / set ---
            "list_tools" => {
                let Some(_app) = &self.app else {
                    self.send_error("headless_unsupported", "marketplace needs Tauri runtime");
                    return;
                };
                // list_marketplace_tools 是 sync I/O(扫 manifest 目录),走 spawn_blocking
                // 避免阻塞 dispatcher 协程。
                let all = match tokio::task::spawn_blocking(|| {
                    crate::commands::list_marketplace_tools()
                })
                .await
                {
                    Ok(Ok(tools)) => tools,
                    Ok(Err(e)) => {
                        self.send_error("tools_list_failed", &format!("{e:#}"));
                        return;
                    }
                    Err(e) => {
                        self.send_error("tools_list_failed", &format!("join: {e:#}"));
                        return;
                    }
                };
                let disabled_ids = crate::bridge::marketplace::load_disabled_connectors();
                self.send_event(
                    &active_session_id,
                    "tools_snapshot",
                    json!({
                        "all": all,
                        "disabled_ids": disabled_ids,
                    }),
                );
                return;
            }
            "set_disabled_connectors" => {
                let Some(app) = &self.app else {
                    self.send_error("headless_unsupported", "marketplace needs Tauri runtime");
                    return;
                };
                let ids: Vec<String> = action
                    .payload
                    .get("connector_ids")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                match crate::commands::apply_disabled_connectors(Some(app), pool, ids).await {
                    Ok(()) => {
                        // apply_disabled_connectors 自身不 emit,这里补一次本地广播。
                        let _ = app.emit("remote_control:tools_changed", ());
                        self.send_event(
                            &active_session_id,
                            "tools_changed",
                            json!({}),
                        );
                    }
                    Err(e) => self.send_error("set_tools_failed", &format!("{e:#}")),
                }
                return;
            }
            // --- 附件上传 (attach) 分发:start / chunk / abort ---
            // 三个动作都不依赖 EnginePool / SessionStore / KB / 工具市场,只读 manager 内部状态,
            // 因此抽到独立私有方法里(便于单测直接调,不必起 EnginePool)。
            "attach_file_start" => {
                self.handle_attach_file_start(&active_session_id, &action.payload);
                return;
            }
            "attach_file_chunk" => {
                self.handle_attach_file_chunk(&action.payload).await;
                return;
            }
            "attach_file_abort" => {
                self.handle_attach_file_abort(&active_session_id, &action.payload);
                return;
            }
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
        if let Some(app) = &self.app {
            let _ = app.emit(
                "remote_control:snapshot_requested",
                json!({ "session_id": session_id }),
            );
        }
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
        // 切 session 时,旧 session 的进行中上传失效:清槽位 + 删盘上目录。
        // 新 session 的快照里不会出现旧 upload,mobile UI 据此自然收尾,
        // 故不需要向 mobile 发 attach_file_aborted 事件。
        self.cleanup_active_upload("switch_session");
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
        if let Some(app) = &self.app {
            let _ = app.emit(
                "remote_control:session_created",
                json!({ "session": session_payload.clone() }),
            );
        }
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
        store: &SessionStore,
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
        let path = resolve_session_preview_path(store, session_id, artifact_path)?;
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

    pub fn send_artifact_download(
        &self,
        store: &SessionStore,
        session_id: &str,
        artifact_id: Option<&str>,
        artifact_path: Option<&str>,
    ) -> Result<(), String> {
        let (path, artifact_id) = if let Some(id) = artifact_id {
            let saved = store
                .load(session_id)
                .map_err(|e| format!("load artifact download({session_id}): {e:?}"))?;
            let artifact = saved
                .artifacts
                .iter()
                .find(|a| a.id == id)
                .ok_or_else(|| format!("artifact not found: {id}"))?;
            (artifact.storage_path.clone(), Value::String(artifact.id.clone()))
        } else if let Some(requested) = artifact_path {
            (
                resolve_session_preview_path(store, session_id, requested)?,
                Value::Null,
            )
        } else {
            return Err("missing artifact_id".to_string());
        };
        let metadata =
            std::fs::metadata(&path).map_err(|e| format!("read artifact metadata: {e}"))?;
        if !metadata.is_file() {
            return Err("artifact is not a file".to_string());
        }
        let byte_size = metadata.len();
        if byte_size > DOWNLOAD_LIMIT_BYTES {
            return Err(format!(
                "artifact too large to download ({byte_size} bytes, limit {DOWNLOAD_LIMIT_BYTES} bytes)"
            ));
        }
        // 同步打开一次,把权限/占用类错误留在同步路径,由 handle_mobile_action 回错给 mobile。
        std::fs::File::open(&path).map_err(|e| format!("open artifact file: {e}"))?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let download_id = format!(
            "dl_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        );
        let (ack_sender, ack_receiver) = tokio::sync::mpsc::unbounded_channel();
        let room = {
            let mut inner = self.inner.lock();
            let room = inner
                .room
                .as_ref()
                .filter(|room| room.session_id == session_id)
                .cloned()
                .ok_or_else(|| "remote control not active".to_string())?;
            if inner.active_download.is_some() {
                return Err("已有下载进行中,请等待完成后再试".to_string());
            }
            inner.active_download = Some(download_id.clone());
            inner.download_ack_sender = Some(ack_sender);
            room
        };
        // 流式传输在独立任务里进行:不阻塞消息循环,分块经有界通道发送形成背压。
        let manager = self.clone();
        let task_download_id = download_id;
        tauri::async_runtime::spawn(async move {
            let result = stream_artifact_download(
                &room,
                &path,
                artifact_id,
                &task_download_id,
                &ext,
                byte_size,
                ack_receiver,
            )
            .await;
            if let Err(err) = result {
                manager.send_error("artifact_download_failed", &err);
            }
            let mut inner = manager.inner.lock();
            if inner.active_download.as_deref() == Some(task_download_id.as_str()) {
                inner.active_download = None;
                inner.download_ack_sender = None;
            }
        });
        Ok(())
    }

    fn handle_download_relay_ack(&self, download_id: &str, ack: DownloadRelayAck) {
        let sender = {
            let inner = self.inner.lock();
            if inner.active_download.as_deref() != Some(download_id) {
                return;
            }
            inner.download_ack_sender.clone()
        };
        if let Some(sender) = sender {
            let _ = sender.send(ack);
        }
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
        // mobile 断开:旧 upload 必然无法收到后续 chunk,清掉槽位 + 删盘上数据。
        // 锁外调 cleanup(它自己再 lock,且 lock 之间无嵌套)。
        if status == "mobile_disconnected" {
            self.cleanup_active_upload("mobile_disconnected");
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

    /// 推一帧事件给当前会话绑定的房间。room 不存在或 session_id 不匹配时静默丢弃
    /// (与 forward_local_event / send_artifact_list 等既有路径一致)。
    fn send_event(&self, session_id: &str, kind: &str, payload: Value) {
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
                kind,
                payload,
            )));
        }
    }

    // ─── 附件上传分发(attach_file_*)───
    // 不读 EnginePool / SessionStore / KB / 工具市场,只操作 manager 内部上传状态,
    // 因此独立成方法,dispatch arm 仅做薄转发;单测可直接调,不必起 EnginePool。

    /// 处理 `attach_file_start`:校验 → 占用 active_upload 槽位 → 起 streaming task →
    /// 回 `attach_file_start_ack`。streaming task(Group E)负责真正的写盘 + ingest。
    fn handle_attach_file_start(&self, active_session_id: &str, payload: &Value) {
        let filename = payload
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let byte_size = payload.get("byte_size").and_then(|v| v.as_u64()).unwrap_or(0);
        let mime = payload
            .get("mime")
            .and_then(|v| v.as_str())
            .unwrap_or("application/octet-stream")
            .to_string();
        if filename.is_empty() || byte_size == 0 {
            self.send_error("invalid_attach_params", "filename and byte_size required");
            return;
        }
        if byte_size > UPLOAD_LIMIT_BYTES {
            self.send_error("upload_too_large", &format!("max {UPLOAD_LIMIT_BYTES} bytes"));
            return;
        }
        // 关键:锁内分支只决定"是否要拒绝",不在锁内调 send_error / send_event
        // (二者都会再 `inner.lock()` → re-entrant 死锁)。锁外再回错。
        // 拒绝信息用 (code, message) 元组传出锁块,避免字符串拼接 + split_once 的脆弱解析。
        let prepared: Result<(String, tokio::sync::mpsc::Receiver<UploadChunkMsg>), (&str, String)> =
            {
                let mut inner = self.inner.lock();
                if inner.active_upload.is_some() {
                    Err(("upload_in_progress", "another upload is running".to_string()))
                } else {
                    // 与 download_id 同源格式:pid + nanos,保持单调,便于日志排查。
                    let upload_id = format!(
                        "up_{}_{}",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos())
                            .unwrap_or(0)
                    );
                    let upload_dir = inner.uploads_base.join(&upload_id);
                    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
                        // 不在锁内 send_error —— collect 错误,锁外回。
                        Err(("upload_dir_failed", format!("{e:#}")))
                    } else {
                        let (tx, rx) =
                            tokio::sync::mpsc::channel::<UploadChunkMsg>(UPLOAD_CHANNEL_CAPACITY);
                        inner.active_upload = Some(upload_id.clone());
                        inner.upload_chunk_sender = Some(tx);
                        inner.pending_attachments.insert(
                            upload_id.clone(),
                            PendingAttachment {
                                session_id: active_session_id.to_string(),
                                filename: filename.clone(),
                                byte_size,
                                mime: mime.clone(),
                                ingest_result: None,
                            },
                        );
                        Ok((upload_id, rx))
                    }
                }
            };
        let (upload_id, rx) = match prepared {
            Ok(pair) => pair,
            Err((code, message)) => {
                self.send_error(code, &message);
                return;
            }
        };
        // 启动流式任务消费分块(Group E 落地真正的写盘 + ingest 逻辑)。
        let manager_clone = self.clone();
        tauri::async_runtime::spawn(manager_clone.stream_file_upload(upload_id.clone(), rx));
        self.send_event(
            active_session_id,
            "attach_file_start_ack",
            json!({
                "upload_id": upload_id,
                "chunk_bytes": UPLOAD_CHUNK_BYTES,
            }),
        );
    }

    /// 处理 `attach_file_chunk`:base64 解码 → 投到 streaming task 通道。ACK 由
    /// streaming task(Group E)写盘成功后 emit `attach_file_relay_ack`,不在本方法里。
    async fn handle_attach_file_chunk(&self, payload: &Value) {
        let upload_id = payload
            .get("upload_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let index = payload
            .get("index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let last = payload.get("last").and_then(|v| v.as_bool()).unwrap_or(false);
        let data_b64 = payload.get("data_base64").and_then(|v| v.as_str()).unwrap_or("");
        let data = match base64::engine::general_purpose::STANDARD.decode(data_b64) {
            Ok(d) => d,
            Err(e) => {
                self.send_error("invalid_base64", &format!("{e:#}"));
                return;
            }
        };
        // 关键:在锁外 await,避免持有 parking_lot::Mutex 跨 await 点死锁。
        // send_error 自身会 lock inner,mismatch 分支必须先 drop 守卫再 send_error,
        // 否则 parking_lot 不可重入 → 永久死锁。
        let sender = {
            let inner = self.inner.lock();
            if inner.active_upload.as_deref() != Some(&upload_id) {
                drop(inner);
                self.send_error("unknown_upload", "upload_id not active");
                return;
            }
            inner.upload_chunk_sender.clone()
        };
        let Some(sender) = sender else {
            self.send_error("upload_closed", "sender dropped");
            return;
        };
        if let Err(e) = sender
            .send(UploadChunkMsg {
                upload_id,
                index,
                data,
                last,
            })
            .await
        {
            self.send_error("upload_send_failed", &format!("{e:#}"));
        }
    }

    /// 处理 `attach_file_abort`:丢 sender 让 streaming task 自然收尾 → 删盘上数据 → 回 ACK。
    fn handle_attach_file_abort(&self, active_session_id: &str, payload: &Value) {
        let upload_id = payload
            .get("upload_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let dir_to_remove = {
            let mut inner = self.inner.lock();
            if inner.active_upload.as_deref() != Some(&upload_id) {
                drop(inner);
                self.send_error("unknown_upload", "upload_id not active");
                return;
            }
            // 丢掉 sender 让 streaming task 自然结束(rx 返回 None)。
            inner.upload_chunk_sender = None;
            inner.active_upload = None;
            inner.pending_attachments.remove(&upload_id);
            Some(inner.uploads_base.join(&upload_id))
        };
        if let Some(dir) = dir_to_remove {
            let _ = std::fs::remove_dir_all(&dir);
        }
        self.send_event(
            active_session_id,
            "attach_file_aborted",
            json!({
                "upload_id": upload_id,
                "reason": "client_abort",
            }),
        );
    }

    fn emit_status(&self) {
        if let Some(app) = &self.app {
            let _ = app.emit("remote_control:status", self.status());
        }
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

fn resolve_session_preview_path(
    store: &SessionStore,
    session_id: &str,
    requested: &str,
) -> Result<PathBuf, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("missing artifact_path".to_string());
    }
    let workspace = store
        .execution_workspace(session_id)
        .map_err(|e| format!("resolve session workspace: {e:#}"))?;
    let scheduled = store.scheduled_profile(session_id).is_some();
    let artifacts = paths::session_artifacts_dir(session_id);
    let workspace_root = workspace
        .canonicalize()
        .map_err(|e| format!("resolve session workspace: {e}"))?;
    let artifacts_root = artifacts.canonicalize().ok();
    // A scheduled engine executes in the shared user workspace. Treating that
    // entire tree as remotely previewable would let a path-shaped mobile
    // request read unrelated user files. Only paths already recorded as this
    // conversation's artifacts (plus its private artifacts directory) inherit
    // preview authority.
    let scheduled_artifacts = if scheduled {
        store
            .load(session_id)
            .map_err(|e| format!("load scheduled artifacts({session_id}): {e:#}"))?
            .artifacts
            .into_iter()
            .filter_map(|artifact| artifact.storage_path.canonicalize().ok())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
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
        let in_private_artifacts = artifacts_root
            .as_ref()
            .is_some_and(|root| canonical.starts_with(root));
        let authorized = if scheduled {
            in_private_artifacts || scheduled_artifacts.iter().any(|path| path == &canonical)
        } else {
            canonical.starts_with(&workspace_root) || in_private_artifacts
        };
        if authorized {
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

fn download_chunk_count(byte_len: usize) -> usize {
    if byte_len == 0 {
        0
    } else {
        byte_len.div_ceil(DOWNLOAD_CHUNK_BYTES)
    }
}

/// 流式发送一个产物下载:start → 逐块 chunk → end,全部经有界下载通道。
/// 文件按 768KB 逐块读取,同一时刻内存里只有一块原始字节 + 其 base64;
/// 通道满时 `.await` 挂起形成背压,大文件不会在无界队列里堆积。
async fn stream_artifact_download(
    room: &ActiveRoom,
    path: &Path,
    artifact_id: Value,
    download_id: &str,
    ext: &str,
    byte_size: u64,
    mut ack_receiver: tokio::sync::mpsc::UnboundedReceiver<DownloadRelayAck>,
) -> Result<(), String> {
    use tokio::io::AsyncReadExt;

    let total_chunks = download_chunk_count(byte_size as usize);
    room.download_sender
        .send(RelayOutbound::Envelope(envelope(
            &room.room_id,
            &room.session_id,
            "artifact_download_start",
            json!({
                "session_id": room.session_id,
                "artifact_id": artifact_id,
                "download_id": download_id,
                "basename": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                "path_tail": tail_path(&path.to_string_lossy()),
                "mime": download_mime_for_ext(ext),
                "byte_size": byte_size,
                "total_chunks": total_chunks,
            }),
        )))
        .await
        .map_err(|e| format!("send artifact download start: {e}"))?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("open artifact file: {e}"))?;
    let mut buffer = vec![0u8; DOWNLOAD_CHUNK_BYTES];
    let mut index = 0usize;
    let mut remaining = byte_size as usize;
    while remaining > 0 {
        // read 不保证一次填满,循环读满一块或到 EOF。
        let mut filled = 0usize;
        let target = buffer.len().min(remaining);
        while filled < target {
            let read = file
                .read(&mut buffer[filled..target])
                .await
                .map_err(|e| format!("read artifact file: {e}"))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled != target {
            return Err(format!(
                "artifact changed during download (expected {byte_size} bytes, reached EOF after {} bytes)",
                byte_size as usize - remaining + filled
            ));
        }
        room.download_sender
            .send(RelayOutbound::Envelope(envelope(
                &room.room_id,
                &room.session_id,
                "artifact_download_chunk",
                json!({
                    "download_id": download_id,
                    "index": index,
                    "data": base64::engine::general_purpose::STANDARD.encode(&buffer[..filled]),
                }),
            )))
            .await
            .map_err(|e| format!("send artifact download chunk {index}: {e}"))?;
        let ack = tokio::time::timeout(Duration::from_secs(60), ack_receiver.recv())
            .await
            .map_err(|_| format!("relay/mobile is too slow while sending chunk {index}"))?
            .ok_or_else(|| "download acknowledgement channel closed".to_string())?;
        if ack.index != index {
            return Err(format!(
                "unexpected download acknowledgement: expected chunk {index}, got {}",
                ack.index
            ));
        }
        if !ack.ok {
            return Err(format!(
                "relay could not deliver chunk {index}: {}",
                ack.message.unwrap_or_else(|| "mobile disconnected".to_string())
            ));
        }
        remaining -= filled;
        index += 1;
    }
    let mut extra = [0u8; 1];
    if file
        .read(&mut extra)
        .await
        .map_err(|e| format!("read artifact file: {e}"))?
        != 0
    {
        return Err(format!(
            "artifact changed during download (grew beyond declared {byte_size} bytes)"
        ));
    }
    room.download_sender
        .send(RelayOutbound::Envelope(envelope(
            &room.room_id,
            &room.session_id,
            "artifact_download_end",
            json!({
                "download_id": download_id,
                "total_chunks": total_chunks,
                "byte_size": byte_size,
            }),
        )))
        .await
        .map_err(|e| format!("send artifact download end: {e}"))?;
    Ok(())
}

fn download_mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "zip" => "application/zip",
        "pdf" => "application/pdf",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        other if is_text_ext(other) || is_image_ext(other) => mime_for_ext(other),
        _ => "application/octet-stream",
    }
}

impl RemoteControlManager {
    /// 上传分块流式任务(Group E 落地真正的写盘 + ingest + 完成事件)。
    ///
    /// 消费 dispatch 端投递的 `UploadChunkMsg`:逐块 base64 解码后已变 Vec<u8>,
    /// 直接 `write_all` 写 `<uploads_base>/<upload_id>/data.bin`,每块写完发
    /// `attach_file_relay_ack { index, ok: true }` 让 mobile 推下一块(背压源头)。
    /// 最后一块(`last == true`)触发 flush + close → `file_ingest::ingest`(sync,
    /// 放 `spawn_blocking` 避免阻塞 dispatcher)→ 写 `ingest_result` → emit
    /// `attach_file_result { ok: true, ingest: <IngestResult> }`。
    ///
    /// 错误路径:
    /// - 任何 IO 失败 → `attach_file_result { ok: false, error }` + cleanup;
    /// - `rx.recv()` 超过 60s(ack 超时) → `attach_file_aborted { reason: "ack_timeout" }`;
    /// - `rx.recv()` 返回 `None`(sender dropped,即 mobile abort/disconnect)
    ///   → `attach_file_aborted { reason: "client_disconnected" }`。
    ///
    /// 上传成功完成后:active_upload 槽位让出(允许下一个 upload),但 pending_attachments
    /// 保留到 user_message 取用(消费后才删)。
    async fn stream_file_upload(
        self,
        upload_id: String,
        mut rx: tokio::sync::mpsc::Receiver<UploadChunkMsg>,
    ) {
        use tokio::io::AsyncWriteExt;

        // 锁内只取写盘路径 + session_id,锁外做所有 await / fs / send_event。
        // pending_attachments 已经被 cleanup 清掉时 session_id 空 → 直接退出。
        let (data_path, session_id) = {
            let inner = self.inner.lock();
            let base = inner.uploads_base.join(&upload_id);
            let session_id = inner
                .pending_attachments
                .get(&upload_id)
                .map(|p| p.session_id.clone())
                .unwrap_or_default();
            (base.join("data.bin"), session_id)
        };
        if session_id.is_empty() {
            // 不在 pending_attachments 里 → 已经被 cleanup 清掉了,直接退出。
            return;
        }

        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&data_path)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                self.send_event(
                    &session_id,
                    "attach_file_result",
                    json!({
                        "upload_id": upload_id,
                        "ok": false,
                        "error": format!("open data.bin: {e:#}"),
                    }),
                );
                self.cleanup_active_upload("stream_open_failed");
                return;
            }
        };

        let mut index_expected = 0usize;
        loop {
            let recv = tokio::time::timeout(
                Duration::from_secs(UPLOAD_ACK_TIMEOUT_SECS),
                rx.recv(),
            )
            .await;
            let msg = match recv {
                Err(_) => {
                    // 60s 没收到下一块 → 视为对端 / relay 异常,主动收尾。
                    self.send_event(
                        &session_id,
                        "attach_file_aborted",
                        json!({
                            "upload_id": upload_id,
                            "reason": "ack_timeout",
                        }),
                    );
                    self.cleanup_active_upload("stream_ack_timeout");
                    return;
                }
                Ok(None) => {
                    // sender dropped(mobile abort / disconnect / cleanup 触发)。
                    self.send_event(
                        &session_id,
                        "attach_file_aborted",
                        json!({
                            "upload_id": upload_id,
                            "reason": "client_disconnected",
                        }),
                    );
                    self.cleanup_active_upload("stream_sender_dropped");
                    return;
                }
                Ok(Some(msg)) => msg,
            };
            // 索引连续性校验(防乱序 / 重发)。
            if msg.index != index_expected {
                self.send_event(
                    &session_id,
                    "attach_file_result",
                    json!({
                        "upload_id": upload_id,
                        "ok": false,
                        "error": format!(
                            "chunk index out of order: expected {index_expected}, got {}",
                            msg.index
                        ),
                    }),
                );
                self.cleanup_active_upload("stream_index_oob");
                return;
            }
            if let Err(e) = file.write_all(&msg.data).await {
                self.send_event(
                    &session_id,
                    "attach_file_result",
                    json!({
                        "upload_id": upload_id,
                        "ok": false,
                        "error": format!("write data.bin: {e:#}"),
                    }),
                );
                self.cleanup_active_upload("stream_write_failed");
                return;
            }
            // 回 chunk ack,让 mobile 发下一块(对应 mobile 端的 acknowledgeUploadChunk)。
            self.send_event(
                &session_id,
                "attach_file_relay_ack",
                json!({
                    "upload_id": upload_id,
                    "index": msg.index,
                    "ok": true,
                }),
            );
            index_expected += 1;
            if msg.last {
                break;
            }
        }

        // flush + 关文件后再 ingest;ingest 是 sync + 可能重 I/O(图片/pdf/office
        // 转换),放 spawn_blocking 避免阻塞 async dispatcher。
        if let Err(e) = file.flush().await {
            self.send_event(
                &session_id,
                "attach_file_result",
                json!({
                    "upload_id": upload_id,
                    "ok": false,
                    "error": format!("flush data.bin: {e:#}"),
                }),
            );
            self.cleanup_active_upload("stream_flush_failed");
            return;
        }
        drop(file);

        let data_path_for_ingest = data_path.clone();
        let ingest_result = match tokio::task::spawn_blocking(move || {
            crate::file_ingest::ingest(&data_path_for_ingest)
        })
        .await
        {
            Ok(result) => result,
            Err(join_err) => {
                self.send_event(
                    &session_id,
                    "attach_file_result",
                    json!({
                        "upload_id": upload_id,
                        "ok": false,
                        "error": format!("ingest join: {join_err:#}"),
                    }),
                );
                self.cleanup_active_upload("stream_ingest_join_failed");
                return;
            }
        };

        // 把 ingest_result 存进 pending_attachments;清 active_upload 槽位
        // (上传已完成,槽位让出,但 pending_attachments 保留到 user_message 取用)。
        {
            let mut inner = self.inner.lock();
            if inner.active_upload.as_deref() == Some(&upload_id) {
                inner.active_upload = None;
                inner.upload_chunk_sender = None;
            }
            if let Some(p) = inner.pending_attachments.get_mut(&upload_id) {
                p.ingest_result = Some(ingest_result.clone());
            }
        }

        // spec §4.2:回给 mobile 的只是 ingest_preview(filename + kind + byte_size +
        // token_estimate + warning),**不含 markdown / path**(markdown 走 WS 太重,
        // path 是桌面绝对路径,不该泄漏)。完整 IngestResult 留在 pending_attachments
        // 里,user_message 时随 mobile_user_message 一起透传给前端走桌面 chat。
        let preview_filename = {
            let inner = self.inner.lock();
            inner
                .pending_attachments
                .get(&upload_id)
                .map(|p| p.filename.clone())
                .unwrap_or_else(|| ingest_result.basename.clone())
        };
        self.send_event(
            &session_id,
            "attach_file_result",
            json!({
                "upload_id": upload_id,
                "ok": true,
                "ingest_preview": {
                    "filename": preview_filename,
                    "kind": ingest_result.kind,
                    "byte_size": ingest_result.byte_size,
                    "token_estimate": ingest_result.token_estimate,
                    "warning": ingest_result.warning,
                },
            }),
        );
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
    fn inner_default_uploads_base_is_under_pinvou3_home() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let override_env = PinvouHomeOverride::new("inner-default-uploads");
        let inner = Inner::default();
        assert!(
            inner.uploads_base.ends_with("uploads"),
            "uploads_base should end with `uploads`, got {:?}",
            inner.uploads_base
        );
        assert!(inner.active_upload.is_none());
        assert!(inner.upload_chunk_sender.is_none());
        assert!(inner.pending_attachments.is_empty());
        drop(override_env);
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
        let store = SessionStore::boot().expect("boot session store");
        let current = store
            .create_new(
                "test-model".to_string(),
                None,
                home.root.join("ignored-current"),
            )
            .expect("create current session");
        let other = store
            .create_new(
                "test-model".to_string(),
                None,
                home.root.join("ignored-other"),
            )
            .expect("create other session");
        let current_session = current.metadata.id.as_str();
        let other_session = other.metadata.id.as_str();
        let workspace_file = paths::session_workspace_dir(current_session).join("reports/daily.md");
        let artifact_file = paths::session_artifacts_dir(current_session).join("chart.json");
        let other_file = paths::session_workspace_dir(other_session).join("secret.md");
        let outside_file = home.root.join("outside.txt");
        write_file(&workspace_file, b"workspace report");
        write_file(&artifact_file, br#"{"ok":true}"#);
        write_file(&other_file, b"other session secret");
        write_file(&outside_file, b"outside home");

        assert_eq!(
            resolve_session_preview_path(&store, current_session, "reports/daily.md")
                .expect("workspace"),
            workspace_file
                .canonicalize()
                .expect("canonical workspace file")
        );
        assert_eq!(
            resolve_session_preview_path(&store, current_session, "artifacts/chart.json")
                .expect("artifacts path tail"),
            artifact_file
                .canonicalize()
                .expect("canonical artifact file")
        );
        assert!(resolve_session_preview_path(
            &store,
            current_session,
            other_file.to_str().expect("utf-8 test path")
        )
        .is_err());
        assert!(resolve_session_preview_path(
            &store,
            current_session,
            "../session-other/workspace/secret.md"
        )
        .is_err());
        assert!(resolve_session_preview_path(
            &store,
            current_session,
            outside_file.to_str().expect("utf-8 test path")
        )
        .is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let link = paths::session_workspace_dir(current_session).join("outside-link.txt");
            symlink(&outside_file, &link).expect("create escape symlink");
            assert!(
                resolve_session_preview_path(&store, current_session, "outside-link.txt").is_err()
            );
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

    #[test]
    fn concurrent_download_is_rejected_until_active_download_finishes() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home = PinvouHomeOverride::new("download-lock");
        let store = SessionStore::boot().expect("boot session store");
        let session = store
            .create_new("test-model".to_string(), None, home.root.join("ignored"))
            .expect("create session");
        let session_id = session.metadata.id.clone();
        let workspace = paths::session_workspace_dir(&session_id);
        // 8 块 > 下载通道容量(4):消费端不排空时流式任务堵在背压上,锁必然保持。
        let big = vec![b'x'; DOWNLOAD_CHUNK_BYTES * 8];
        write_file(&workspace.join("reports/lock.bin"), &big);

        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, mut download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_lock_test".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
        }

        manager
            .send_artifact_download(&store, &session_id, None, Some("reports/lock.bin"))
            .expect("first download should be accepted");
        let second =
            manager.send_artifact_download(&store, &session_id, None, Some("reports/lock.bin"));
        assert!(
            second.is_err(),
            "a second download must be rejected while one is active"
        );
        assert!(second.unwrap_err().contains("进行中"));

        // 排空有界通道，并模拟 relay 每写出一块后的 ACK；任务走完后锁必须归还。
        tauri::async_runtime::block_on(async {
            let mut drained = 0usize;
            while drained < 10 {
                match tokio::time::timeout(Duration::from_secs(5), download_rx.recv()).await {
                    Ok(Some(RelayOutbound::Envelope(env))) => {
                        if env.kind == "artifact_download_chunk" {
                            let index = env.payload["index"].as_u64().expect("chunk index") as usize;
                            let download_id = env.payload["download_id"]
                                .as_str()
                                .expect("download id");
                            manager.handle_download_relay_ack(
                                download_id,
                                DownloadRelayAck {
                                    index,
                                    ok: true,
                                    message: None,
                                },
                            );
                        }
                        drained += 1;
                    }
                    Ok(Some(RelayOutbound::Close { .. })) => {
                        panic!("download channel must not carry Close")
                    }
                    other => panic!("download stream ended early: {other:?}"),
                }
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while manager.inner.lock().active_download.is_some() {
                assert!(Instant::now() < deadline, "download lock was not released");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    }

    /// 直接走 handle_attach_file_start,断言 attach_file_start 在超限时拒绝并保留无 active_upload
    /// (而非 panic / 静默)。headless 模式不需要 AppHandle,attach 不读 KB / 工具市场。
    /// 用 `#[test]` + `block_on` 而非 `#[tokio::test]`,以与 concurrent_download 测试对齐:
    /// `tauri::async_runtime::spawn` 派发的 streaming task 必须挂在 tauri runtime 上才能被调度。
    #[test]
    fn attach_file_start_rejects_oversize() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("attach-oversize");
        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, _download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let session_id = "rc-attach-oversize-session".to_string();
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_attach_oversize".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
        }

        // 比 UPLOAD_LIMIT_BYTES 多 1 字节,触发 upload_too_large。
        let payload = serde_json::json!({
            "filename": "huge.bin",
            "byte_size": UPLOAD_LIMIT_BYTES + 1,
            "mime": "application/octet-stream",
        });
        manager.handle_attach_file_start(&session_id, &payload);

        // 锁不应被占用:send_error 必须在 spawn 前短路。
        assert!(
            manager.inner.lock().active_upload.is_none(),
            "oversize upload must not occupy the upload slot"
        );
        assert!(
            manager.inner.lock().pending_attachments.is_empty(),
            "oversize upload must not register a pending attachment"
        );
    }

    /// 一个上传进行中时,第二个 start 必须被拒(upload_in_progress),active_upload 不被替换。
    /// 第三步 abort 验证槽位能放出来。
    ///
    /// 不通过 `handle_attach_file_start` 起第一个 upload(那会触发 streaming task spawn,
    /// 而本测试是 sync `#[test]`,无法驱动 tauri::async_runtime 让 task 前进 —— 会让进程
    /// 在退出时 hang 在 task join 上)。直接手工写入 `active_upload`,只覆盖并发拒绝 + abort
    /// 释放两条纯同步路径。streaming task 的端到端正确性留给 Group E / Group L 的集成测试。
    #[test]
    fn attach_file_start_blocks_concurrent() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("attach-concurrent");
        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, _download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let session_id = "rc-attach-concurrent-session".to_string();
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_attach_concurrent".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
        }

        // 手工占用 upload 槽位(模拟"已有一个 upload 进行中"):
        // 写入 active_upload + pending_attachments,但**不**起 streaming task。
        // 这样可以纯同步地覆盖并发拒绝 + abort 释放两条路径,无需驱动 tauri runtime
        // 让 streaming task 前进 —— 否则 sync `#[test]` 进程退出时会 hang 在 task join 上。
        // streaming task 的端到端正确性留给 Group E / Group L 的集成测试。
        let existing_upload_id = "up_existing_test".to_string();
        {
            let mut inner = manager.inner.lock();
            inner.active_upload = Some(existing_upload_id.clone());
            inner.pending_attachments.insert(
                existing_upload_id.clone(),
                PendingAttachment {
                    session_id: session_id.clone(),
                    filename: "preexisting.bin".to_string(),
                    byte_size: 1024,
                    mime: "application/octet-stream".to_string(),
                    ingest_result: None,
                },
            );
        }

        // 第二次 start 必须被拒:槽位被占 → send_error(upload_in_progress),active 不被替换。
        let second_payload = serde_json::json!({
            "filename": "b.bin",
            "byte_size": 1024u64,
            "mime": "application/octet-stream",
        });
        manager.handle_attach_file_start(&session_id, &second_payload);

        let active_after_second = manager.inner.lock().active_upload.clone();
        assert_eq!(
            active_after_second.as_deref(),
            Some(existing_upload_id.as_str()),
            "second attach_file_start must not evict the active upload"
        );

        // abort 当前 upload,验证槽位真的能放出来(覆盖 abort 路径)。
        let abort_payload = serde_json::json!({ "upload_id": existing_upload_id });
        manager.handle_attach_file_abort(&session_id, &abort_payload);

        assert!(
            manager.inner.lock().active_upload.is_none(),
            "attach_file_abort must release the upload slot"
        );
        assert!(
            manager.inner.lock().pending_attachments.is_empty(),
            "attach_file_abort must drop the pending attachment"
        );
    }

    /// 回归:attach_file_chunk 在 upload_id 不匹配时,必须在 drop 锁守卫之后再 send_error,
    /// 否则 parking_lot::Mutex 不可重入 → 永久死锁。本测试用 1s 超时驱动 async fn,
    /// 若路径未修复则会 hang 到超时失败。
    #[test]
    fn attach_file_chunk_unknown_upload_id_does_not_deadlock() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("attach-chunk-unknown");
        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, _download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let session_id = "rc-attach-chunk-unknown-session".to_string();
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_attach_chunk_unknown".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
            // 槽位被另一个 upload 占用,使下面的 chunk upload_id 必然不匹配。
            inner.active_upload = Some("up_other_active".to_string());
        }

        let payload = serde_json::json!({
            "upload_id": "up_does_not_match",
            "index": 0u64,
            "data_base64": "AA==",
            "last": false,
        });

        let manager_clone = manager.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build one-off runtime");
        rt.block_on(async move {
            // 修复前:这里会永久 hang(send_error 重入死锁)。修复后:mismatch 分支 drop
            // 守卫后正常 send_error 并 return。
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                manager_clone.handle_attach_file_chunk(&payload),
            )
            .await
            .expect("handle_attach_file_chunk must not deadlock on unknown upload_id");
        });

        // 锁仍然可用(未被毒化/未被泄漏持有)。
        assert!(
            manager.inner.lock().active_upload.is_some(),
            "unknown upload_id chunk must not evict the active upload slot"
        );
    }

    /// 回归:attach_file_abort 在 upload_id 不匹配时同样要 drop 锁守卫后再 send_error。
    /// 同一类 re-entrant 死锁防御。
    #[test]
    fn attach_file_abort_unknown_upload_id_does_not_deadlock() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("attach-abort-unknown");
        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, _download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let session_id = "rc-attach-abort-unknown-session".to_string();
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_attach_abort_unknown".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
            inner.active_upload = Some("up_other_active".to_string());
        }

        let payload = serde_json::json!({ "upload_id": "up_does_not_match" });

        // abort 是 sync fn,但内含 send_error → 修复前会直接死锁本线程。
        // 用 channel + recv_timeout 真正检测:死锁回归会让 recv_timeout 超时失败,
        // 而不是让 cargo 整体 test 超时才被动发现。
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let manager_clone = manager.clone();
        let session_id_for_thread = session_id.clone();
        std::thread::spawn(move || {
            manager_clone.handle_attach_file_abort(&session_id_for_thread, &payload);
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("handle_attach_file_abort must not deadlock on unknown upload_id");

        assert!(
            manager.inner.lock().active_upload.is_some(),
            "unknown upload_id abort must not evict the active upload slot"
        );
    }

    /// 回归:handle_attach_file_start 在 uploads_base 创建失败时(磁盘满 / 权限丢失)
    /// 不能在锁内 send_error —— 否则 parking_lot 重入死锁。本测试通过把 upload_id
    /// 目录预置为文件让 create_dir_all 失败,断言 start 在 1s 内返回而非 hang。
    #[test]
    fn attach_file_start_dir_create_failure_does_not_deadlock() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("attach-dir-fail");
        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, _download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let session_id = "rc-attach-dir-fail-session".to_string();
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_attach_dir_fail".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
            // 把 uploads_base 设成一个已存在的**文件**路径 → join(upload_id) 后
            // create_dir_all 必然失败(EEXIST / ENOTDIR)。
            let uploads_base = inner.uploads_base.clone();
            drop(inner);
            std::fs::create_dir_all(uploads_base.parent().unwrap_or_else(|| std::path::Path::new("/")))
                .ok();
            std::fs::write(&uploads_base, b"blocker").expect("seed uploads_base as file");
        }

        let payload = serde_json::json!({
            "filename": "any.bin",
            "byte_size": 16u64,
            "mime": "application/octet-stream",
        });

        // 修复前:create_dir_all 失败 → 锁内 send_error → 死锁,handle 永不返回。
        // 修复后:错误以元组传出锁块,锁外 send_error。
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let manager_clone = manager.clone();
        let session_id_for_thread = session_id.clone();
        std::thread::spawn(move || {
            manager_clone.handle_attach_file_start(&session_id_for_thread, &payload);
            let _ = done_tx.send(());
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("handle_attach_file_start must not deadlock when uploads_base is not a dir");

        assert!(
            manager.inner.lock().active_upload.is_none(),
            "dir-failed start must not occupy the upload slot"
        );
    }

    /// 回归:close_current / stop_current 必须清空 upload 相关 Inner 字段,并 best-effort
    /// 删除盘上未完成的 upload 目录。防止 manager 拆除后 active_upload 槽位残留,导致下次
    /// attach_file_start 被永久拒服务,以及 <uploads_base>/<upload_id>/ 半成品文件堆积。
    #[test]
    fn close_current_clears_active_upload_slot() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("close-clears-upload");
        let (sender, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (download_sender, _download_rx) =
            tokio::sync::mpsc::channel(relay_client::DOWNLOAD_CHANNEL_CAPACITY);
        let session_id = "rc-close-clears-upload-session".to_string();
        let manager = RemoteControlManager::new_headless();
        let upload_dir;
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: "rc_close_clears_upload".to_string(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url: String::new(),
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
            inner.active_upload = Some("up_close_test".to_string());
            inner.pending_attachments.insert(
                "up_close_test".to_string(),
                PendingAttachment {
                    session_id: session_id.clone(),
                    filename: "x.bin".to_string(),
                    byte_size: 8,
                    mime: "application/octet-stream".to_string(),
                    ingest_result: None,
                },
            );
            upload_dir = inner.uploads_base.join("up_close_test");
            std::fs::create_dir_all(&upload_dir).expect("seed upload dir");
            std::fs::write(upload_dir.join("data.bin"), b"partial").expect("seed partial file");
        }

        manager.close_current("test");

        let inner = manager.inner.lock();
        assert!(
            inner.active_upload.is_none(),
            "close_current must clear active_upload"
        );
        assert!(
            inner.upload_chunk_sender.is_none(),
            "close_current must clear sender"
        );
        assert!(
            inner.pending_attachments.is_empty(),
            "close_current must clear pending_attachments"
        );
        // spec §5:close_current / stop_current / disconnect **不删源文件**
        // (防 LLM read_file 竞争);源文件由 mobile_disconnected / session 切换 /
        // streaming task timeout 后续清理。这里断言目录**仍在**,守住这条不变量。
        assert!(
            upload_dir.exists(),
            "close_current must NOT remove upload dir (spec: source files survive until mobile_disconnected / switch / timeout)"
        );
        // 清理本测试自己造的目录,避免污染其他用例。
        let _ = std::fs::remove_dir_all(&upload_dir);
    }

    /// 回归:cleanup_active_upload 在没有进行中 upload 时必须 no-op,既不 panic 也不修改状态。
    /// 这覆盖 mobile_disconnected / switch_session 在"恰好无上传"时的快路径。
    #[test]
    fn cleanup_active_upload_is_noop_when_no_upload_active() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("cleanup-noop");
        let manager = RemoteControlManager::new_headless();
        manager.cleanup_active_upload("noop_test"); // 不应 panic
        assert!(
            manager.inner.lock().active_upload.is_none(),
            "cleanup with no active upload must not populate the slot"
        );
        assert!(
            manager.inner.lock().pending_attachments.is_empty(),
            "cleanup with no active upload must not populate pending_attachments"
        );
    }

    /// 端到端 happy path:dispatch 推两块("hello" + "world",最后一块 last=true)→
    /// stream_file_upload 必须 (1) 拼接写盘成 "helloworld";(2) 跑 file_ingest::ingest
    /// 填 ingest_result;(3) 发 attach_file_result(ok=true);(4) 释放 active_upload 槽位。
    ///
    /// ingest 对 10 字节 .txt(由 data.bin 扩展名走 binary 兜底 —— 不需要任何外部工具)
    /// 必返回有 basename/path/byte_size 的 IngestResult,无需 pandoc/pdftotext 等。
    #[tokio::test]
    async fn stream_file_upload_writes_chunks_and_ingests() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("stream-upload-happy");
        let session_id = "rc-stream-happy-session".to_string();
        let manager = RemoteControlManager::new_headless();
        let upload_id = "up_stream_happy".to_string();
        {
            let mut inner = manager.inner.lock();
            std::fs::create_dir_all(inner.uploads_base.join(&upload_id)).unwrap();
            inner.active_upload = Some(upload_id.clone());
            inner.pending_attachments.insert(
                upload_id.clone(),
                PendingAttachment {
                    session_id: session_id.clone(),
                    filename: "hello.txt".to_string(),
                    byte_size: 10,
                    mime: "text/plain".to_string(),
                    ingest_result: None,
                },
            );
        }
        // 推两块:第一块 "hello",最后一块 "world"。
        let (tx, rx) = tokio::sync::mpsc::channel::<UploadChunkMsg>(UPLOAD_CHANNEL_CAPACITY);
        tx.send(UploadChunkMsg {
            upload_id: upload_id.clone(),
            index: 0,
            data: b"hello".to_vec(),
            last: false,
        })
        .await
        .unwrap();
        tx.send(UploadChunkMsg {
            upload_id: upload_id.clone(),
            index: 1,
            data: b"world".to_vec(),
            last: true,
        })
        .await
        .unwrap();
        drop(tx); // 关 sender,避免后续 rx.recv() 永久挂起(streaming task 在 last 后已 break,不会等)

        // stream_file_upload 消费 self(manager 是 Clone,Arc<Mutex<Inner>> 共享)。
        // clone 一份调用,原 manager 留给后续断言用。
        manager.clone().stream_file_upload(upload_id.clone(), rx).await;

        let inner = manager.inner.lock();
        let data_path = inner.uploads_base.join(&upload_id).join("data.bin");
        let bytes = std::fs::read(&data_path).unwrap();
        assert_eq!(
            &bytes, b"helloworld",
            "chunks must concatenate correctly into data.bin"
        );
        let pending = inner
            .pending_attachments
            .get(&upload_id)
            .expect("pending_attachment preserved after successful upload");
        let ir = pending
            .ingest_result
            .as_ref()
            .expect("ingest_result must be filled after last chunk");
        // ingest 用的是 data.bin 路径,basename 即 "data.bin";原 filename 保留在
        // PendingAttachment.filename 里(供前端 UI 展示)。这是 spec §5 的已知取舍。
        assert_eq!(ir.basename, "data.bin");
        assert_eq!(ir.byte_size, 10);
        assert!(
            inner.active_upload.is_none(),
            "active_upload slot must be released after successful upload"
        );
        assert!(
            inner.upload_chunk_sender.is_none(),
            "upload_chunk_sender must be cleared after successful upload"
        );
    }

    /// Abort 路径:dispatch 还没推任何 chunk,channel sender 就被 drop(mobile abort /
    /// disconnect)→ stream_file_upload 的 rx.recv() 立刻返回 None → 必须发
    /// attach_file_aborted(reason=client_disconnected) + cleanup_active_upload(删盘 + 清状态)。
    #[tokio::test]
    async fn stream_file_upload_emits_aborted_on_sender_drop() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new("stream-abort");
        let session_id = "rc-stream-abort-session".to_string();
        let manager = RemoteControlManager::new_headless();
        let upload_id = "up_stream_abort".to_string();
        {
            let mut inner = manager.inner.lock();
            std::fs::create_dir_all(inner.uploads_base.join(&upload_id)).unwrap();
            inner.active_upload = Some(upload_id.clone());
            inner.pending_attachments.insert(
                upload_id.clone(),
                PendingAttachment {
                    session_id: session_id.clone(),
                    filename: "x.bin".to_string(),
                    byte_size: 100,
                    mime: "application/octet-stream".to_string(),
                    ingest_result: None,
                },
            );
        }
        let (_tx, rx) = tokio::sync::mpsc::channel::<UploadChunkMsg>(UPLOAD_CHANNEL_CAPACITY);
        drop(_tx); // 立即关 sender 模拟 mobile abort / disconnect

        manager.clone().stream_file_upload(upload_id.clone(), rx).await;

        let inner = manager.inner.lock();
        assert!(
            inner.active_upload.is_none(),
            "cleanup must release active_upload slot on sender drop"
        );
        assert!(
            inner.pending_attachments.is_empty(),
            "cleanup must drop pending_attachment on sender drop"
        );
        let data_path = inner.uploads_base.join(&upload_id).join("data.bin");
        assert!(
            !data_path.exists(),
            "cleanup must remove upload dir on sender drop"
        );
    }
}

fn tail_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts = normalized.split('/').rev().take(3).collect::<Vec<_>>();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod scheduled_tests {
    use super::*;
    use crate::bridge::sessions::{ScheduledRunMode, ScheduledRunProfile};

    #[test]
    fn preview_paths_follow_execution_workspace_without_exposing_shared_home() {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou3-remote-preview-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let state_home = root.join("state");
        let previous_pinvou3_home = std::env::var_os("PINVOU3_HOME");
        let previous_user_profile = std::env::var_os("USERPROFILE");
        let previous_home = std::env::var_os("HOME");
        std::fs::create_dir_all(&root).expect("create test home");
        std::env::set_var("PINVOU3_HOME", &state_home);
        std::env::set_var("USERPROFILE", &root);
        std::env::set_var("HOME", &root);

        let store = SessionStore::boot().expect("boot session store");
        let scheduled = store
            .create_scheduled_run(ScheduledRunProfile {
                task_id: "remote-preview-task".to_string(),
                model: "test-model".to_string(),
                model_id: None,
                workspace: root.join("ignored-profile-workspace"),
                mode: ScheduledRunMode::Yolo,
                allow_shell: true,
                trust_mode: true,
                auto_approve: true,
            })
            .expect("create scheduled conversation");
        let recorded = root.join("scheduled-report.md");
        let unrelated = root.join("unrelated-secret.txt");
        std::fs::write(&recorded, "report").expect("write recorded artifact");
        std::fs::write(&unrelated, "secret").expect("write unrelated file");
        store
            .append_scheduled_artifact_path(&scheduled.metadata.id, recorded.clone())
            .expect("record scheduled artifact");

        assert_eq!(
            resolve_session_preview_path(
                &store,
                &scheduled.metadata.id,
                &recorded.to_string_lossy(),
            )
            .expect("resolve recorded scheduled artifact"),
            recorded
                .canonicalize()
                .expect("canonical recorded artifact")
        );
        assert!(
            resolve_session_preview_path(
                &store,
                &scheduled.metadata.id,
                &unrelated.to_string_lossy()
            )
            .is_err(),
            "the shared scheduled workspace must not grant remote preview authority over arbitrary home files"
        );

        let chat = store
            .create_new("test-model".to_string(), None, root.join("ignored"))
            .expect("create ordinary conversation");
        let chat_workspace = crate::bridge::paths::session_workspace_dir(&chat.metadata.id);
        std::fs::create_dir_all(&chat_workspace).expect("create chat workspace");
        let chat_artifact = chat_workspace.join("chat-report.md");
        std::fs::write(&chat_artifact, "chat report").expect("write chat artifact");
        assert_eq!(
            resolve_session_preview_path(&store, &chat.metadata.id, "chat-report.md")
                .expect("resolve ordinary workspace artifact"),
            chat_artifact
                .canonicalize()
                .expect("canonical ordinary artifact")
        );

        drop(store);
        match previous_pinvou3_home {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        match previous_user_profile {
            Some(value) => std::env::set_var("USERPROFILE", value),
            None => std::env::remove_var("USERPROFILE"),
        }
        match previous_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn download_chunk_count_matches_chunk_boundaries() {
        assert_eq!(download_chunk_count(0), 0);
        assert_eq!(download_chunk_count(1), 1);
        assert_eq!(download_chunk_count(DOWNLOAD_CHUNK_BYTES), 1);
        assert_eq!(download_chunk_count(DOWNLOAD_CHUNK_BYTES + 1), 2);
        assert_eq!(download_chunk_count(DOWNLOAD_CHUNK_BYTES * 3), 3);
    }

    #[test]
    fn download_mime_falls_back_to_octet_stream_for_binary() {
        assert_eq!(download_mime_for_ext("md"), "text/markdown");
        assert_eq!(download_mime_for_ext("png"), "image/png");
        assert_eq!(download_mime_for_ext("zip"), "application/zip");
        assert_eq!(download_mime_for_ext("bin"), "application/octet-stream");
        assert_eq!(download_mime_for_ext(""), "application/octet-stream");
    }
}

/// 端到端测试:真实 node relay + 真实 relay_client(WS) + 真实 manager 下载逻辑,
/// mobile 侧用 tokio-tungstenite 扮演,验证 request_artifact_download 全链路字节一致。
/// 依赖 remote-control-relay/node_modules(npm ci)与 node 可执行文件,缺失时跳过。
#[cfg(test)]
mod e2e_tests {
    use super::relay_client::RelayReceiver;
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;
    use futures_util::{SinkExt, StreamExt};
    use std::ffi::OsString;
    use std::net::TcpListener;
    use std::process::{Child, Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    struct E2eEnv {
        home: PathBuf,
        previous_home: Option<OsString>,
        child: Option<Child>,
    }

    impl Drop for E2eEnv {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            match self.previous_home.take() {
                Some(value) => std::env::set_var("PINVOU3_HOME", value),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    fn now_nanos() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    }

    fn report_missing_e2e_dependency(reason: &str) {
        if std::env::var("PINVOU_REQUIRE_REMOTE_E2E").as_deref() == Ok("1") {
            panic!("required remote e2e dependency missing: {reason}");
        }
        eprintln!("skip remote e2e: {reason}");
    }

    #[test]
    fn artifact_download_round_trips_through_real_relay() {
        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let relay_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../remote-control-relay")
            .canonicalize()
            .expect("remote-control-relay dir");
        let home = std::env::temp_dir().join(format!(
            "pinvou3-remote-e2e-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        let previous_home = std::env::var_os("PINVOU3_HOME");
        std::env::set_var("PINVOU3_HOME", &home);
        if !relay_dir.join("node_modules/ws/package.json").exists() {
            report_missing_e2e_dependency(
                "relay node_modules missing (npm ci in remote-control-relay)",
            );
            return;
        }
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("bind ephemeral port")
            .local_addr()
            .expect("local addr")
            .port();
        let child = match Command::new("node")
            .arg("server.js")
            .current_dir(&relay_dir)
            .env("PORT", port.to_string())
            .env("PINVOU_REMOTE_PUBLIC_BASE_PATH", "")
            .env("HEARTBEAT_INTERVAL_MS", "60000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                report_missing_e2e_dependency(&format!("cannot spawn node: {err}"));
                std::env::remove_var("PINVOU3_HOME");
                return;
            }
        };
        let mut guard = E2eEnv {
            home: home.clone(),
            previous_home,
            child: Some(child),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "relay did not start on port {port}");
            std::thread::sleep(Duration::from_millis(100));
        }

        let store = SessionStore::boot().expect("boot session store");
        let session = store
            .create_new("test-model".to_string(), None, home.join("ignored"))
            .expect("create session");
        let session_id = session.metadata.id.clone();
        let workspace = paths::session_workspace_dir(&session_id);
        let big_bytes: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();
        let big_path = workspace.join("reports/e2e-big.bin");
        std::fs::create_dir_all(big_path.parent().expect("big parent")).expect("big dir");
        std::fs::write(&big_path, &big_bytes).expect("write big file");
        let text_bytes = "你好,远程下载".as_bytes().to_vec();
        let text_path = workspace.join("notes/hello.txt");
        std::fs::create_dir_all(text_path.parent().expect("text parent")).expect("text dir");
        std::fs::write(&text_path, &text_bytes).expect("write text file");

        let room_id = format!("rc_{}", crate::remote_control::short_token(18));
        let pairing_token = crate::remote_control::short_token(32);
        let desktop_secret = crate::remote_control::short_token(32);
        let relay_ws_url = format!("ws://127.0.0.1:{port}/ws");
        let (sender, download_sender, mut receiver) = relay_client::spawn(
            relay_ws_url.clone(),
            room_id.clone(),
            session_id.clone(),
            pairing_token.clone(),
            desktop_secret,
        );
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: room_id.clone(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url,
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
        }

        let big_expect = big_bytes.clone();
        let text_expect = text_bytes.clone();
        tauri::async_runtime::block_on(async move {
            let result = tokio::time::timeout(
                Duration::from_secs(60),
                run_mobile_e2e(
                    port,
                    room_id,
                    pairing_token,
                    session_id.clone(),
                    manager,
                    store,
                    &mut receiver,
                    big_expect,
                    text_expect,
                ),
            )
            .await;
            assert!(result.is_ok(), "remote download e2e timed out");
        });
        let _ = &mut guard;
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_mobile_e2e(
        port: u16,
        room_id: String,
        pairing_token: String,
        session_id: String,
        manager: RemoteControlManager,
        store: SessionStore,
        receiver: &mut RelayReceiver,
        big_expect: Vec<u8>,
        text_expect: Vec<u8>,
    ) {
        let url = format!("ws://127.0.0.1:{port}/ws");
        // desktop_register 由 relay_client 后台任务异步完成,mobile_join 需要等 room 就绪。
        let (mut write, mut read) = 'join: {
            for attempt in 0..40 {
                let (ws, _) = connect_async(&url).await.expect("mobile ws connect");
                let (mut write, mut read) = ws.split();
                write
                    .send(Message::Text(
                        json!({
                            "type": "mobile_join",
                            "room_id": room_id,
                            "token": pairing_token,
                        })
                        .to_string(),
                    ))
                    .await
                    .expect("send mobile_join");
                let reply = tokio::time::timeout(Duration::from_secs(3), read.next())
                    .await
                    .expect("join reply timeout")
                    .expect("join reply stream")
                    .expect("join reply ws");
                let value: Value =
                    serde_json::from_str(reply.to_text().unwrap_or("")).expect("join reply json");
                if value.get("type").and_then(|v| v.as_str()) == Some("mobile_joined") {
                    break 'join (write, read);
                }
                assert!(
                    attempt < 39,
                    "mobile_join never succeeded: {}",
                    value
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            unreachable!("join loop must break or assert")
        };

        // relay 在 mobile_join 后自动向 desktop 推 request_snapshot,确认 desktop 侧真实收到。
        let first_action = recv_mobile_action(receiver).await;
        assert_eq!(
            first_action.get("type").and_then(|v| v.as_str()),
            Some("request_snapshot"),
            "relay should forward the auto request_snapshot to desktop: {first_action}"
        );

        // mobile 请求下载大文件(2MB,应分 3 块),desktop 经真实 WS 收到 action。
        send_mobile_action(&mut write, "dl-big", json!({ "artifact_path": "reports/e2e-big.bin" }))
            .await;
        let download_action = recv_mobile_action(receiver).await;
        assert_eq!(
            download_action.get("type").and_then(|v| v.as_str()),
            Some("request_artifact_download"),
            "desktop should receive the download action: {download_action}"
        );
        assert_eq!(
            download_action
                .get("payload")
                .and_then(|p| p.get("artifact_path"))
                .and_then(|v| v.as_str()),
            Some("reports/e2e-big.bin")
        );

        // desktop 侧分发目标:send_artifact_download(handle_mobile_action 的该 action 命中函数)。
        manager
            .send_artifact_download(&store, &session_id, None, Some("reports/e2e-big.bin"))
            .expect("send big artifact download");
        let big = collect_download(&mut read, receiver, &manager, "e2e-big.bin").await;
        assert_eq!(big.session_id, session_id);
        assert_eq!(big.mime, "application/octet-stream");
        assert_eq!(big.byte_size, big_expect.len() as u64);
        assert_eq!(big.total_chunks, 3);
        assert_eq!(big.bytes, big_expect, "big file bytes must round-trip");
        // 下载锁在任务发送完 end 后才释放,下一次下载前等它归位,避免时序抖动。
        wait_download_idle(&manager).await;

        // 小文本文件:单块 + 正确 MIME。
        send_mobile_action(&mut write, "dl-text", json!({ "artifact_path": "notes/hello.txt" }))
            .await;
        let _ = recv_mobile_action(receiver).await;
        manager
            .send_artifact_download(&store, &session_id, None, Some("notes/hello.txt"))
            .expect("send text artifact download");
        let text = collect_download(&mut read, receiver, &manager, "hello.txt").await;
        assert_eq!(text.mime, "text/plain");
        assert_eq!(text.total_chunks, 1);
        assert_eq!(text.bytes, text_expect, "text file bytes must round-trip");
        wait_download_idle(&manager).await;

        // 越界路径必须被拒绝(不会向 mobile 发任何 download 事件)。
        let escape = manager.send_artifact_download(&store, &session_id, None, Some("/etc/hostname"));
        assert!(escape.is_err(), "absolute path outside session must be rejected");
        let missing = manager.send_artifact_download(&store, &session_id, None, Some("nope.bin"));
        assert!(missing.is_err(), "missing artifact must be rejected");
    }

    async fn send_mobile_action(
        write: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        client_message_id: &str,
        payload: Value,
    ) {
        write
            .send(Message::Text(
                json!({
                    "type": "mobile_action",
                    "payload": {
                        "type": "request_artifact_download",
                        "client_message_id": client_message_id,
                        "payload": payload,
                    },
                })
                .to_string(),
            ))
            .await
            .expect("send mobile_action");
    }

    async fn recv_mobile_action(receiver: &mut RelayReceiver) -> Value {
        loop {
            let inbound = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
                .await
                .expect("desktop inbound timeout")
                .expect("desktop inbound closed");
            if let RelayInbound::MobileAction { payload, .. } = inbound {
                return payload;
            }
        }
    }

    async fn wait_download_idle(manager: &RemoteControlManager) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while manager.inner.lock().active_download.is_some() {
            assert!(Instant::now() < deadline, "download lock was not released");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    struct CollectedDownload {
        session_id: String,
        mime: String,
        byte_size: u64,
        total_chunks: usize,
        bytes: Vec<u8>,
    }

    async fn collect_download(
        read: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        receiver: &mut RelayReceiver,
        manager: &RemoteControlManager,
        expect_basename: &str,
    ) -> CollectedDownload {
        let mut session_id = String::new();
        let mut mime = String::new();
        let mut byte_size = 0_u64;
        let mut total_chunks = 0_usize;
        let mut download_id = String::new();
        let mut chunks: Vec<Option<String>> = Vec::new();
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(15), read.next())
                .await
                .expect("download event timeout")
                .expect("download stream closed")
                .expect("download ws error");
            let value: Value =
                serde_json::from_str(msg.to_text().unwrap_or("")).expect("download event json");
            let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let payload = value.get("payload").cloned().unwrap_or(Value::Null);
            match kind {
                "artifact_download_start" => {
                    assert_eq!(
                        payload.get("basename").and_then(|v| v.as_str()),
                        Some(expect_basename)
                    );
                    download_id = payload
                        .get("download_id")
                        .and_then(|v| v.as_str())
                        .expect("download_id")
                        .to_string();
                    assert!(!download_id.is_empty());
                    session_id = payload
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    mime = payload
                        .get("mime")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    byte_size = payload.get("byte_size").and_then(|v| v.as_u64()).unwrap_or(0);
                    total_chunks = payload
                        .get("total_chunks")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as usize;
                    chunks = vec![None; total_chunks];
                }
                "artifact_download_chunk" => {
                    assert!(!download_id.is_empty(), "chunk before start");
                    assert_eq!(
                        payload.get("download_id").and_then(|v| v.as_str()),
                        Some(download_id.as_str())
                    );
                    let index = payload.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    assert!(index < total_chunks, "chunk index {index} out of range");
                    let data = payload
                        .get("data")
                        .and_then(|v| v.as_str())
                        .expect("chunk data")
                        .to_string();
                    chunks[index] = Some(data);
                    let ack = loop {
                        let inbound = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
                            .await
                            .expect("download ack timeout")
                            .expect("desktop inbound closed");
                        if let RelayInbound::DownloadAck {
                            download_id,
                            index,
                            ok,
                            message,
                        } = inbound
                        {
                            break (download_id, DownloadRelayAck { index, ok, message });
                        }
                    };
                    manager.handle_download_relay_ack(&ack.0, ack.1);
                }
                "artifact_download_end" => {
                    assert_eq!(
                        payload.get("download_id").and_then(|v| v.as_str()),
                        Some(download_id.as_str())
                    );
                    assert_eq!(
                        payload.get("total_chunks").and_then(|v| v.as_u64()),
                        Some(total_chunks as u64),
                        "end total_chunks must match start"
                    );
                    assert_eq!(
                        payload.get("byte_size").and_then(|v| v.as_u64()),
                        Some(byte_size),
                        "end byte_size must match start"
                    );
                    let mut bytes = Vec::new();
                    for (index, chunk) in chunks.iter().enumerate() {
                        let data = chunk
                            .as_ref()
                            .unwrap_or_else(|| panic!("missing chunk {index}"));
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .expect("chunk base64");
                        bytes.extend_from_slice(&decoded);
                    }
                    return CollectedDownload {
                        session_id,
                        mime,
                        byte_size,
                        total_chunks,
                        bytes,
                    };
                }
                other => panic!("unexpected mobile event during download: {other}"),
            }
        }
    }

    fn bin_runnable(bin: &str, args: &[&str]) -> bool {
        Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Chrome 解析顺序:CHROME 环境变量 → 仓库本地 .cache/puppeteer(手动 zip 或
    /// npx @puppeteer/browsers 布局) → 常见系统路径。只返回能跑 --version 的。
    fn resolve_chrome_binary(app_dir: &Path) -> Option<String> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(from_env) = std::env::var("CHROME") {
            if !from_env.is_empty() {
                candidates.push(PathBuf::from(from_env));
            }
        }
        if let Some(repo_root) = app_dir.parent() {
            candidates.push(repo_root.join(".cache/puppeteer/chrome-linux64/chrome"));
            if let Ok(entries) = std::fs::read_dir(repo_root.join(".cache/puppeteer/chrome")) {
                for entry in entries.flatten() {
                    candidates.push(entry.path().join("chrome-linux64/chrome"));
                }
            }
        }
        for system in [
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ] {
            candidates.push(PathBuf::from(system));
        }
        candidates
            .into_iter()
            .find(|path| path.exists() && bin_runnable(&path.to_string_lossy(), &["--version"]))
            .map(|path| path.to_string_lossy().to_string())
    }

    /// 真实浏览器全链路:真实 node relay + 真实 manager(流式下载/单下载锁/背压)
    /// + 真实 Chrome/Chromium(puppeteer-core 驱动 relay 服务的真实手机端页面)。
    /// 覆盖:64MiB 近上限下载字节一致、64MiB+1 超限拒绝、下载中重复点击拦截、
    /// 中途杀 relay 断连中断。依赖 node 与 Chrome 二进制,缺失时跳过。
    /// Chrome 解析顺序:CHROME 环境变量 → 仓库本地 .cache/puppeteer → 常见系统路径。
    #[test]
    fn real_browser_download_full_stack() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let _env_lock = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let relay_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../remote-control-relay")
            .canonicalize()
            .expect("remote-control-relay dir");
        let app_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .canonicalize()
            .expect("pinvou3-app dir");
        if !relay_dir.join("node_modules/ws/package.json").exists() {
            report_missing_e2e_dependency(
                "relay node_modules missing (npm ci in remote-control-relay)",
            );
            return;
        }
        if !app_dir
            .join("node_modules/puppeteer-core/package.json")
            .exists()
        {
            report_missing_e2e_dependency(
                "pinvou3-app node_modules missing (npm ci in pinvou3-app)",
            );
            return;
        }
        if !bin_runnable("node", &["--version"]) {
            report_missing_e2e_dependency("node not available");
            return;
        }
        let chrome_bin = resolve_chrome_binary(&app_dir);
        let Some(chrome_bin) = chrome_bin else {
            report_missing_e2e_dependency(
                "no Chrome/Chromium binary (set CHROME, or install Chrome for Testing into .cache/puppeteer)",
            );
            return;
        };

        let home = std::env::temp_dir().join(format!(
            "pinvou3-remote-browser-e2e-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        let previous_home = std::env::var_os("PINVOU3_HOME");
        std::env::set_var("PINVOU3_HOME", &home);
        let relay_child: Arc<parking_lot::Mutex<Option<Child>>> = Arc::new(parking_lot::Mutex::new(None));
        struct Guard {
            home: PathBuf,
            previous_home: Option<OsString>,
            relay_child: Arc<parking_lot::Mutex<Option<Child>>>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                if let Some(mut child) = self.relay_child.lock().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                match self.previous_home.take() {
                    Some(value) => std::env::set_var("PINVOU3_HOME", value),
                    None => std::env::remove_var("PINVOU3_HOME"),
                }
                let _ = std::fs::remove_dir_all(&self.home);
            }
        }
        let guard = Guard {
            home: home.clone(),
            previous_home,
            relay_child: relay_child.clone(),
        };

        // 近上限文件:恰好 64MiB(边界值,>64MiB 才拒绝);内容为确定性伪随机。
        let store = SessionStore::boot().expect("boot session store");
        let session = store
            .create_new("test-model".to_string(), None, home.join("ignored"))
            .expect("create session");
        let session_id = session.metadata.id.clone();
        let workspace = paths::session_workspace_dir(&session_id);
        let pattern: Vec<u8> = (0..1024 * 1024u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let near_bytes = pattern.repeat((DOWNLOAD_LIMIT_BYTES as usize) / pattern.len());
        assert_eq!(near_bytes.len() as u64, DOWNLOAD_LIMIT_BYTES);
        let near_path = workspace.join("reports/near-limit.bin");
        std::fs::create_dir_all(near_path.parent().expect("near parent")).expect("near dir");
        std::fs::write(&near_path, &near_bytes).expect("write near-limit file");
        // 超限文件:稀疏到 64MiB+1,仅元数据参与判定。
        let oversize_path = workspace.join("reports/oversize.bin");
        let oversize_file = std::fs::File::create(&oversize_path).expect("create oversize file");
        oversize_file
            .set_len(DOWNLOAD_LIMIT_BYTES + 1)
            .expect("size oversize file");
        drop(oversize_file);
        store
            .update_artifacts(
                &session_id,
                vec![
                    near_path.to_string_lossy().to_string(),
                    oversize_path.to_string_lossy().to_string(),
                ],
            )
            .expect("register artifacts");

        let port = TcpListener::bind("127.0.0.1:0")
            .expect("bind ephemeral port")
            .local_addr()
            .expect("local addr")
            .port();
        let child = Command::new("node")
            .arg("server.js")
            .current_dir(&relay_dir)
            .env("PORT", port.to_string())
            .env("PINVOU_REMOTE_PUBLIC_BASE_PATH", "")
            .env("HEARTBEAT_INTERVAL_MS", "60000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn relay");
        *relay_child.lock() = Some(child);
        let deadline = Instant::now() + Duration::from_secs(15);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "relay did not start on port {port}");
            std::thread::sleep(Duration::from_millis(100));
        }

        let room_id = format!("rc_{}", crate::remote_control::short_token(18));
        let pairing_token = crate::remote_control::short_token(32);
        let desktop_secret = crate::remote_control::short_token(32);
        let relay_ws_url = format!("ws://127.0.0.1:{port}/ws");
        let (sender, download_sender, mut receiver) = relay_client::spawn(
            relay_ws_url.clone(),
            room_id.clone(),
            session_id.clone(),
            pairing_token.clone(),
            desktop_secret,
        );
        let manager = RemoteControlManager::new_headless();
        {
            let mut inner = manager.inner.lock();
            inner.room = Some(ActiveRoom {
                room_id: room_id.clone(),
                session_id: session_id.clone(),
                url: String::new(),
                relay_ws_url,
                status: RemoteControlStatusKind::WaitingMobile,
                last_error: None,
                sender,
                download_sender,
            });
        }

        // 分发任务:扮演 handle_mobile_action 中本场景用到的 action 子集
        // (EnginePool 需要 AppHandle,headless 无法构造;下载/预览/列表路径全部真实)。
        let download_actions = Arc::new(AtomicUsize::new(0));
        let stop_dispatch = Arc::new(AtomicBool::new(false));
        let control_file = home.join("control.txt");
        {
            let manager = manager.clone();
            let store = store.clone();
            let download_actions = download_actions.clone();
            let stop_dispatch = stop_dispatch.clone();
            let control_file = control_file.clone();
            let relay_child = relay_child.clone();
            tauri::async_runtime::spawn(async move {
                while !stop_dispatch.load(Ordering::SeqCst) {
                    // 控制文件:驱动端要求杀 relay,模拟真实网络中断。
                    if let Ok(cmd) = std::fs::read_to_string(&control_file) {
                        if cmd.trim() == "kill_relay" {
                            if let Some(mut child) = relay_child.lock().take() {
                                let _ = child.kill();
                                let _ = child.wait();
                            }
                            let _ = std::fs::remove_file(&control_file);
                        }
                    }
                    let inbound =
                        match tokio::time::timeout(Duration::from_millis(100), receiver.recv())
                            .await
                        {
                            Ok(Some(inbound)) => inbound,
                            Ok(None) => break,
                            Err(_) => continue,
                        };
                    let payload = match inbound {
                        RelayInbound::DownloadAck {
                            download_id,
                            index,
                            ok,
                            message,
                        } => {
                            manager.handle_download_relay_ack(
                                &download_id,
                                DownloadRelayAck { index, ok, message },
                            );
                            continue;
                        }
                        RelayInbound::MobileAction { payload, .. } => payload,
                        _ => continue,
                    };
                    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let action_payload = payload.get("payload").cloned().unwrap_or(Value::Null);
                    let sid = manager
                        .inner
                        .lock()
                        .room
                        .as_ref()
                        .map(|room| room.session_id.clone())
                        .unwrap_or_default();
                    if sid.is_empty() {
                        continue;
                    }
                    let result = match kind {
                        "request_snapshot" | "ping" => {
                            manager.send_snapshot_with_live_request(&store, &sid)
                        }
                        "request_session_list" => manager.send_session_list(&store, &sid),
                        "request_chips" => manager.send_chips_snapshot(&store, &sid),
                        "request_artifacts" => manager.send_artifact_list(&store, &sid),
                        "switch_remote_session" => match action_payload
                            .get("session_id")
                            .and_then(|v| v.as_str())
                        {
                            Some(id) => manager.switch_remote_session(&store, id),
                            None => Ok(()),
                        },
                        "request_artifact_preview" => {
                            if let Some(id) =
                                action_payload.get("artifact_id").and_then(|v| v.as_str())
                            {
                                manager.send_artifact_preview(&store, &sid, id)
                            } else if let Some(path) =
                                action_payload.get("artifact_path").and_then(|v| v.as_str())
                            {
                                manager.send_artifact_preview_by_path(&store, &sid, path)
                            } else {
                                Err("missing artifact_id".to_string())
                            }
                        }
                        "request_artifact_download" => {
                            download_actions.fetch_add(1, Ordering::SeqCst);
                            let id = action_payload.get("artifact_id").and_then(|v| v.as_str());
                            let path =
                                action_payload.get("artifact_path").and_then(|v| v.as_str());
                            manager.send_artifact_download(&store, &sid, id, path)
                        }
                        _ => Ok(()),
                    };
                    if let Err(err) = result {
                        manager.send_error("mobile_action_failed", &err);
                    }
                }
            });
        }

        let download_dir = home.join("firefox-downloads");
        let params_path = home.join("driver-params.json");
        let params = json!({
            "pageUrl": format!("http://127.0.0.1:{port}/r/{room_id}#token={pairing_token}"),
            "sessionTitle": session.metadata.title,
            "sessionIdShort": &session_id[..session_id.len().min(6)],
            "artifactName": "near-limit.bin",
            "oversizeName": "oversize.bin",
            "downloadDir": download_dir,
            "controlFile": control_file,
            "sourceFile": near_path,
            "expectedSize": DOWNLOAD_LIMIT_BYTES,
            "chromeBin": chrome_bin,
        });
        std::fs::write(&params_path, serde_json::to_string_pretty(&params).expect("params json"))
            .expect("write driver params");

        let status = Command::new("node")
            .arg(relay_dir.join("test/real-browser-download.driver.mjs"))
            .arg(&params_path)
            .current_dir(&relay_dir)
            .stdin(Stdio::null())
            .status()
            .expect("spawn real browser driver");
        stop_dispatch.store(true, Ordering::SeqCst);
        assert!(status.success(), "real browser driver failed");
        assert_eq!(
            download_actions.load(Ordering::SeqCst),
            3,
            "desktop must see exactly 3 download actions: 64MiB 完成 + 超限拒绝 + 第二次 64MiB(重复点击被浏览器拦截)"
        );
        let _ = &guard;
    }
}
