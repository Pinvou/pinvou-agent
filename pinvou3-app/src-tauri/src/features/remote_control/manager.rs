use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fs2::FileExt as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::event_stream::{
    snapshot_message, stream_reset_message, try_enqueue_message_batch, EventStreamState,
    ReplayMessageContext, StreamRecordError, LEASE_ID_WIRE_PLACEHOLDER,
};
use super::platform;
use super::protocol::{
    WebAccessConfig, WebAccessInfo, WebAccessStatus, WebAccessStatusKind, PROTOCOL_VERSION,
};
use super::relay_client::{self, RelayInbound, RelayOutbound, RelaySender};
use crate::features::sessions::SessionStore;
use crate::platform::paths;

// 正式安装包默认连接生产 Relay；本地联调由 run-dev.sh 显式覆盖到隔离的
// remote-test 端点。用户保存的自定义 Relay 设置仍具有最高优先级。
const DEFAULT_PUBLIC_BASE_URL: &str = "http://127.0.0.1:8787/pinvou3/remote";
const DEFAULT_RELAY_WS_URL: &str = "ws://127.0.0.1:8787/pinvou3/remote/ws";
const MAX_WEB_ACCESS_CONFIG_BYTES: usize = 16 * 1024;
const RPC_CACHE_CAPACITY: usize = 512;
const RPC_CACHE_BYTES_CAPACITY: usize = 32 * 1024 * 1024;
const MAX_RPC_IN_FLIGHT: usize = 32;
/// 浏览器端 `invoke` 180s 即超时放弃；桌面命令卡死时按此上限惰性释放
/// in-flight 槽位，避免 `too_many_in_flight_requests` 永久拒绝后续请求。
const RPC_IN_FLIGHT_TTL: Duration = Duration::from_secs(300);
const MAX_RPC_COMMAND_BYTES: usize = 128;
const MAX_RPC_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const RPC_LEDGER_VERSION: u8 = 1;
/// 会话分块单独放大：首开会话在高延迟链路上按 256KB 串行拉取过慢。
/// 上限受 `MAX_RPC_RESPONSE_BYTES`（2MiB）约束，base64 膨胀后仍需留出 JSON 外壳余量。
pub const MAX_SESSION_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_WEB_ATTACHMENTS: usize = 16;
const MAX_WEB_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_WEB_ATTACHMENTS_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_WEB_ATTACHMENT_UPLOADS: usize = 4;
const MAX_WEB_ATTACHMENT_UPLOAD_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// 浏览器本机上传在桌面端的短生命周期暂存目录前缀。仅带此前缀的目录会被
/// 启动清扫与附件消费清理删除，避免波及旧版 E2E 使用的其他 uploads 子目录。
pub(crate) const WEB_ATTACHMENT_UPLOAD_DIR_PREFIX: &str = "webup_";
const MAX_WEB_SESSION_UPLOADS: usize = 4;
const MAX_WEB_SESSION_UPLOAD_BYTES: usize = 256 * 1024 * 1024;
const MAX_WEB_SESSION_DOWNLOADS: usize = 2;
const PENDING_REVOCATIONS_VERSION: u8 = 1;
const MAX_PENDING_REVOCATIONS: usize = 32;
const MAX_PENDING_REVOCATIONS_FILE_BYTES: usize = 256 * 1024;
const MAX_WEB_SESSION_UPLOAD_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const WEB_SESSION_TRANSFER_TTL: Duration = Duration::from_secs(10 * 60);

const RUST_FORWARDED_EVENTS: &[&str] = &[
    "artifact:disk",
    "chat:compaction",
    "chat:delta",
    "chat:reasoning_start",
    "chat:reasoning_delta",
    "chat:reasoning_done",
    "chat:done",
    "chat:plan_ready",
    "chat:plan_resolved",
    "chat:plan_snapshot",
    "chat:transcript_committed",
    "chat:turn_started",
    "chat:tool_end",
    "chat:tool_start",
    "chat:transient_error",
    "chat:usage",
    "chat:user_message",
    "chat:user_input_required",
    "scheduled_task:run_updated",
    "session:deleted",
    "session:list_changed",
    "session:model_changed",
    "session:persona_changed",
];

/// 原生代码会话判定（与 Engine bridge 共用 AcpPool 那份 SessionAgentStore 注入）。
/// 远程端正式支持代码会话列表/授权/UI 之前，带 session_id 的原生代码会话事件
/// 一律不转发。
pub type CodeSessionPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct RemoteControlManager {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
    lifecycle: Arc<Mutex<()>>,
    policy: Arc<AccessPolicy>,
    policy_error: Option<String>,
}

struct Inner {
    /// Held for the remainder of this desktop process after Web access is
    /// first touched. This makes the persistent endpoint and RPC ledger a
    /// single-process authority even if two app instances are launched.
    process_lock: Option<File>,
    endpoint: Option<ActiveEndpoint>,
    idle_status: WebAccessStatusKind,
    idle_error: Option<String>,
    event_stream: EventStreamState,
    bridge_generation: Option<String>,
    rpc_ledger: RpcLedger,
    rpc_cache: HashMap<String, RpcCacheEntry>,
    rpc_order: VecDeque<String>,
    web_attachments: HashMap<String, CachedWebAttachment>,
    web_attachment_order: VecDeque<String>,
    web_attachment_bytes: usize,
    web_attachment_uploads: HashMap<String, WebAttachmentUpload>,
    web_attachment_upload_order: VecDeque<String>,
    web_session_uploads: HashMap<String, WebSessionUpload>,
    web_session_upload_order: VecDeque<String>,
    web_session_downloads: HashMap<String, WebSessionDownload>,
    web_session_download_order: VecDeque<String>,
    pending_revocations_in_flight: HashSet<String>,
    /// 原生代码会话判定；None = 未注入（不过滤，启动早期行为不变）。
    code_session_predicate: Option<CodeSessionPredicate>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            process_lock: None,
            endpoint: None,
            idle_status: WebAccessStatusKind::Idle,
            idle_error: None,
            event_stream: EventStreamState::new(),
            bridge_generation: None,
            rpc_ledger: RpcLedger::default(),
            rpc_cache: HashMap::new(),
            rpc_order: VecDeque::new(),
            web_attachments: HashMap::new(),
            web_attachment_order: VecDeque::new(),
            web_attachment_bytes: 0,
            web_attachment_uploads: HashMap::new(),
            web_attachment_upload_order: VecDeque::new(),
            web_session_uploads: HashMap::new(),
            web_session_upload_order: VecDeque::new(),
            web_session_downloads: HashMap::new(),
            web_session_download_order: VecDeque::new(),
            pending_revocations_in_flight: HashSet::new(),
            code_session_predicate: None,
        }
    }
}

#[derive(Debug)]
struct WebSessionUpload {
    session_id: String,
    expected_revision: String,
    total: usize,
    data: Vec<u8>,
    last_touched: Instant,
}

/// 浏览器本机文件分块上传的内存累积缓冲。落盘与 ingest 只发生在最后一块提交时。
#[derive(Debug)]
struct WebAttachmentUpload {
    file_name: String,
    total: usize,
    data: Vec<u8>,
    last_touched: Instant,
}

#[derive(Debug)]
struct WebSessionDownload {
    session_id: String,
    path: PathBuf,
    reserved_bytes: usize,
    total: usize,
    ready: bool,
    last_touched: Instant,
}

pub struct WebSessionDownloadReservation {
    manager: RemoteControlManager,
    download_id: String,
    session_id: String,
    path: PathBuf,
    committed: bool,
}

impl WebSessionDownloadReservation {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn commit(mut self) -> Result<String, String> {
        let result = self.manager.commit_web_session_download(
            &self.download_id,
            &self.session_id,
            &self.path,
        );
        if result.is_ok() {
            self.committed = true;
        }
        result.map(|()| self.download_id.clone())
    }
}

impl Drop for WebSessionDownloadReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.manager
                .remove_web_session_download(&self.download_id, Some(&self.path));
        }
    }
}

#[derive(Debug)]
pub struct WebSessionDownloadChunk {
    pub total: usize,
    pub data: Vec<u8>,
    pub eof: bool,
}

#[derive(Debug, Clone)]
struct CachedWebAttachment {
    result: crate::features::files::file_ingest::IngestResult,
    bytes: usize,
    reservation_id: Option<String>,
    /// The browser removed the chip while a chat submission still held the
    /// handle. Defer deletion until that reservation finishes so the engine
    /// never races a disappearing upload source.
    discard_requested: bool,
    /// 浏览器本机上传的桌面暂存目录。附件被消费、淘汰或清空时随之删除，
    /// 保证上传文件不留存；桌面主机文件附件恒为 None。
    upload_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebAttachmentSummary {
    pub handle: String,
    pub kind: String,
    pub basename: String,
    pub token_estimate: u32,
    pub byte_size: u64,
    pub warning: Option<String>,
}

#[derive(Clone)]
struct ActiveEndpoint {
    config: WebAccessConfig,
    url: String,
    status: WebAccessStatusKind,
    web_client_connected: bool,
    client_ready: bool,
    lease_id: Option<String>,
    last_lease_id: Option<String>,
    /// Stream head captured when a browser without a cursor joins. Readiness
    /// is asynchronous, so fresh-client replay must start here rather than at
    /// the later `client_ready` instant.
    fresh_baseline_seq: Option<u64>,
    last_error: Option<String>,
    sender: RelaySender,
}

#[derive(Debug, Clone)]
enum RpcCacheEntry {
    Pending(PendingRpc),
    Complete {
        fingerprint: String,
        completion: RpcCompletion,
    },
}

#[derive(Debug, Clone)]
struct PendingRpc {
    lease_id: String,
    command: String,
    args: Value,
    fingerprint: String,
    dispatched_generation: Option<String>,
    acknowledged_generation: Option<String>,
    dispatched_at: Instant,
}

#[derive(Debug, Clone)]
struct RpcDispatch {
    request_id: String,
    command: String,
    args: Value,
    bridge_generation: String,
}

#[derive(Debug, Clone)]
struct RpcCompletion {
    ok: bool,
    result: Value,
    error: Option<String>,
    error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcLedger {
    #[serde(default = "rpc_ledger_version")]
    version: u8,
    #[serde(default)]
    endpoint_id: String,
    #[serde(default)]
    entries: VecDeque<RpcTombstone>,
}

impl Default for RpcLedger {
    fn default() -> Self {
        Self {
            version: RPC_LEDGER_VERSION,
            endpoint_id: String::new(),
            entries: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RpcTombstone {
    request_id: String,
    fingerprint: String,
    #[serde(default)]
    acknowledged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRevocation {
    relay_url: String,
    endpoint_id: String,
    desktop_secret: String,
}

impl From<&WebAccessConfig> for PendingRevocation {
    fn from(config: &WebAccessConfig) -> Self {
        Self {
            relay_url: config.relay_url.clone(),
            endpoint_id: config.endpoint_id.clone(),
            desktop_secret: config.desktop_secret.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRevocationLedger {
    version: u8,
    #[serde(default)]
    entries: VecDeque<PendingRevocation>,
}

impl Default for PendingRevocationLedger {
    fn default() -> Self {
        Self {
            version: PENDING_REVOCATIONS_VERSION,
            entries: VecDeque::new(),
        }
    }
}

const fn rpc_ledger_version() -> u8 {
    RPC_LEDGER_VERSION
}

impl RpcLedger {
    fn for_endpoint(endpoint_id: &str) -> Self {
        Self {
            version: RPC_LEDGER_VERSION,
            endpoint_id: endpoint_id.to_string(),
            entries: VecDeque::new(),
        }
    }

    fn get(&self, request_id: &str) -> Option<&RpcTombstone> {
        self.entries
            .iter()
            .find(|entry| entry.request_id == request_id)
    }

    fn remember(&mut self, request_id: &str, fingerprint: &str) -> Result<bool, String> {
        if let Some(existing) = self.get(request_id) {
            if existing.fingerprint != fingerprint {
                return Err("client_request_id was reused with different command or args".into());
            }
            return Ok(false);
        }
        if self.entries.len() == RPC_CACHE_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(RpcTombstone {
            request_id: request_id.to_string(),
            fingerprint: fingerprint.to_string(),
            acknowledged: false,
        });
        Ok(true)
    }

    fn acknowledge(&mut self, request_id: &str, fingerprint: &str) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.request_id == request_id)
            .ok_or_else(|| format!("RPC tombstone is missing: {request_id}"))?;
        if entry.fingerprint != fingerprint {
            return Err("RPC tombstone fingerprint conflict".to_string());
        }
        entry.acknowledged = true;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct EmbeddedPolicy {
    protocol_version: u8,
    #[serde(default)]
    allowed_commands: Vec<String>,
    #[serde(default)]
    allowed_events: Vec<String>,
}

#[derive(Debug, Default)]
struct AccessPolicy {
    commands: HashSet<String>,
    events: HashSet<String>,
}

impl AccessPolicy {
    fn load() -> Result<Self, String> {
        let parsed: EmbeddedPolicy = serde_json::from_str(include_str!(
            "../../../../src/platform/web/access-policy.json"
        ))
        .map_err(|error| format!("parse embedded Web access policy: {error}"))?;
        if parsed.protocol_version != PROTOCOL_VERSION {
            return Err(format!(
                "Web access policy protocol {} does not match backend protocol {}",
                parsed.protocol_version, PROTOCOL_VERSION
            ));
        }
        let policy = Self {
            commands: parsed.allowed_commands.into_iter().collect(),
            events: parsed.allowed_events.into_iter().collect(),
        };
        policy.validate_capability_snapshot()?;
        Ok(policy)
    }

    fn capability_snapshot_wire_bytes(&self) -> Result<usize, String> {
        let mut commands = self.commands.iter().cloned().collect::<Vec<_>>();
        commands.sort();
        let mut events = self.events.iter().cloned().collect::<Vec<_>>();
        events.sort();
        let endpoint_id = "e".repeat(128);
        let stream_epoch = format!("epoch_{}", "0".repeat(24));
        serde_json::to_vec(&snapshot_message(
            &endpoint_id,
            LEASE_ID_WIRE_PLACEHOLDER,
            &stream_epoch,
            u64::MAX,
            &commands,
            &events,
        ))
        .map(|encoded| encoded.len())
        .map_err(|error| format!("serialize Web access capability snapshot: {error}"))
    }

    fn validate_capability_snapshot(&self) -> Result<(), String> {
        let wire_bytes = self.capability_snapshot_wire_bytes()?;
        if wire_bytes > relay_client::MAX_CONTROL_FRAME_BYTES {
            return Err(format!(
                "Web access capability snapshot is too large ({wire_bytes} bytes; limit {})",
                relay_client::MAX_CONTROL_FRAME_BYTES
            ));
        }
        Ok(())
    }
}

impl RemoteControlManager {
    pub fn new(app: AppHandle) -> Self {
        let (policy, policy_error) = match AccessPolicy::load() {
            Ok(policy) => (policy, None),
            Err(error) => {
                eprintln!("[web-access] {error}");
                (AccessPolicy::default(), Some(error))
            }
        };
        Self {
            app,
            inner: Arc::new(Mutex::new(Inner::default())),
            lifecycle: Arc::new(Mutex::new(())),
            policy: Arc::new(policy),
            policy_error,
        }
    }

    /// 注入原生代码会话判定；由 app 组合根在 AcpPool 就绪后调用一次，与 Engine
    /// bridge 共用同一份 SessionAgentStore。存放在共享 `Inner` 里，所有 clone
    /// （含已 manage 进 Tauri 的那份）都能读到。
    pub fn set_code_session_predicate(&self, predicate: CodeSessionPredicate) {
        self.inner.lock().code_session_predicate = Some(predicate);
    }

    /// Resume the persistent endpoint after all authoritative application
    /// state (notably `EnginePool`) has been managed by Tauri.
    pub fn resume(&self) -> Result<bool, String> {
        std::thread::spawn(sweep_stale_web_attachment_uploads);
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        if self.inner.lock().endpoint.is_some() {
            return Ok(true);
        }
        let Some(config) = load_config()? else {
            return Ok(false);
        };
        let config = apply_runtime_relay_override(config);
        validate_config(&config)?;
        self.activate(config)?;
        Ok(true)
    }

    /// Enable WebUI access. Repeated calls return the same persistent endpoint
    /// instead of creating session-scoped rooms.
    pub fn start(&self) -> Result<WebAccessInfo, String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        if let Some(endpoint) = self.inner.lock().endpoint.clone() {
            return Ok(pairing_info(&endpoint));
        }

        let config = match load_config()? {
            Some(config) => {
                let config = apply_runtime_relay_override(config);
                validate_config(&config)?;
                config
            }
            None => {
                let config = fresh_config();
                persist_config(&config)?;
                config
            }
        };
        self.activate(apply_runtime_relay_override(config))?;
        let inner = self.inner.lock();
        let endpoint = inner
            .endpoint
            .as_ref()
            .ok_or_else(|| "Web access endpoint failed to start".to_string())?;
        Ok(pairing_info(endpoint))
    }

    /// Rotate both browser and desktop credentials. The previous endpoint is
    /// revoked at the relay and can no longer reconnect.
    pub fn refresh(&self) -> Result<WebAccessInfo, String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_policy()?;
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        let persisted_previous = load_config()?;
        let previous_config = self
            .inner
            .lock()
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.clone())
            .or_else(|| persisted_previous.clone());
        if let Some(previous) = previous_config.as_ref() {
            queue_pending_revocation(previous)?;
        }
        let config = fresh_config();
        // The old credentials are already durably queued above. If this write
        // fails, the queue entry still matches the current config and replay
        // deliberately leaves the still-active endpoint alone.
        persist_config(&config)?;
        let previous = {
            let mut inner = self.inner.lock();
            inner.event_stream.clear_subscriptions();
            inner.rpc_ledger = RpcLedger::default();
            inner.rpc_cache.clear();
            inner.rpc_order.clear();
            clear_web_attachments(&mut inner);
            inner.event_stream.reset();
            inner.endpoint.take()
        };
        if let Some(previous) = previous {
            let _ = previous.sender.try_send(RelayOutbound::Shutdown);
        }
        self.replay_pending_revocations()?;
        self.activate(apply_runtime_relay_override(config))?;
        let inner = self.inner.lock();
        Ok(pairing_info(inner.endpoint.as_ref().ok_or_else(|| {
            "Web access endpoint failed to rotate".to_string()
        })?))
    }

    pub fn relay_settings(&self) -> RelaySettingsInfo {
        RelaySettingsInfo {
            relay_url: remote_relay_ws_url(),
            public_base_url: remote_public_base_url(),
            custom: load_relay_settings().is_some(),
            default_relay_url: DEFAULT_RELAY_WS_URL.to_string(),
            default_public_base_url: DEFAULT_PUBLIC_BASE_URL.to_string(),
        }
    }

    /// 设置自定义 Relay 地址（域名/IP，见 `normalize_relay_address`）。
    /// 已有 endpoint 时走完整 refresh：旧 endpoint 的撤销意图先落盘（指向旧
    /// relay），新凭据注册到新 relay，分享链接与二维码随之更新。
    pub fn set_relay_address(&self, input: &str) -> Result<RelaySettingsInfo, String> {
        let settings = normalize_relay_address(input)?;
        atomic_write_private_json(&relay_settings_path(), &settings, "Web relay settings")?;
        let has_endpoint = self.inner.lock().endpoint.is_some();
        if has_endpoint || load_config()?.is_some() {
            self.refresh()?;
        }
        Ok(self.relay_settings())
    }

    /// 恢复内置默认 Relay。语义与 `set_relay_address` 对称。
    pub fn reset_relay_address(&self) -> Result<RelaySettingsInfo, String> {
        remove_private_file(&relay_settings_path())?;
        let has_endpoint = self.inner.lock().endpoint.is_some();
        if has_endpoint || load_config()?.is_some() {
            self.refresh()?;
        }
        Ok(self.relay_settings())
    }

    pub fn stop_current(&self) -> Result<(), String> {
        let _lifecycle = self.lifecycle.lock();
        self.ensure_process_ownership()?;
        self.replay_pending_revocations()?;
        let persisted = load_config()?;
        let current_config = self
            .inner
            .lock()
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.config.clone())
            .or_else(|| persisted.clone());
        if let Some(config) = current_config.as_ref() {
            queue_pending_revocation(config)?;
        }
        // A failed removal leaves the old config authoritative. Its queued
        // entry therefore remains dormant instead of revoking a live link.
        remove_config()?;
        let endpoint = {
            let mut inner = self.inner.lock();
            inner.event_stream.clear_subscriptions();
            inner.rpc_cache.clear();
            inner.rpc_order.clear();
            clear_web_attachments(&mut inner);
            inner.rpc_ledger = RpcLedger::default();
            inner.event_stream.reset();
            inner.idle_status = WebAccessStatusKind::Stopped;
            inner.idle_error = None;
            inner.endpoint.take()
        };
        if let Some(endpoint) = endpoint {
            let _ = endpoint.sender.try_send(RelayOutbound::Shutdown);
        }
        self.replay_pending_revocations()?;
        remove_rpc_ledger()?;
        self.emit_status();
        Ok(())
    }

    fn ensure_process_ownership(&self) -> Result<(), String> {
        let mut inner = self.inner.lock();
        if inner.process_lock.is_some() {
            return Ok(());
        }
        let lock = acquire_process_lock(&process_lock_path())?;
        inner.process_lock = Some(lock);
        Ok(())
    }

    /// Start independent retry workers for every retired endpoint. A queue
    /// entry matching the current config is a prepared-but-not-yet-committed
    /// rotation/stop and must remain dormant. This makes crashes between
    /// queueing and replacing/removing `web-access.json` safe.
    fn replay_pending_revocations(&self) -> Result<(), String> {
        let ledger = load_pending_revocations()?;
        let current = load_config()?;
        for pending in eligible_pending_revocations(&ledger, current.as_ref()) {
            let key = pending_revocation_key(&pending);
            {
                let mut inner = self.inner.lock();
                if !inner.pending_revocations_in_flight.insert(key.clone()) {
                    continue;
                }
            }

            let mut receiver = relay_client::spawn_revocation(
                pending.relay_url.clone(),
                pending.endpoint_id.clone(),
                pending.desktop_secret.clone(),
            );
            let manager = self.clone();
            tauri::async_runtime::spawn(async move {
                let mut acknowledged = false;
                while let Some(inbound) = receiver.recv().await {
                    if let RelayInbound::Message(text) = inbound {
                        match serde_json::from_str::<Value>(&text) {
                            Ok(value) if pending_revocation_ack(&value, &pending.endpoint_id) => {
                                acknowledged = true;
                                break;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!("[web-access] queued revocation JSON ignored: {error}")
                            }
                        }
                    }
                }
                manager.finish_pending_revocation(pending, key, acknowledged);
            });
        }
        Ok(())
    }

    fn finish_pending_revocation(
        &self,
        pending: PendingRevocation,
        key: String,
        acknowledged: bool,
    ) {
        let _lifecycle = self.lifecycle.lock();
        if acknowledged {
            if let Err(error) = acknowledge_pending_revocation(&pending) {
                // Leave the durable entry in place. The next lifecycle replay
                // will prove the endpoint absent and retry cleanup.
                eprintln!(
                    "[web-access] persist revocation acknowledgement for {} failed: {error}",
                    pending.endpoint_id
                );
            }
        }
        self.inner.lock().pending_revocations_in_flight.remove(&key);
    }

    /// Keep parsed attachment contents on the desktop. The browser receives
    /// only an opaque handle and compact display metadata, so neither the
    /// ingest response nor the subsequent chat request can overflow Relay.
    pub fn cache_web_attachment(
        &self,
        result: crate::features::files::file_ingest::IngestResult,
    ) -> Result<WebAttachmentSummary, String> {
        self.cache_web_attachment_entry(result, None)
    }

    /// 与 `cache_web_attachment` 同语义，但附件源自浏览器本机上传：登记其
    /// 桌面暂存目录，缓存条目消亡（消费/淘汰/清空）时一并删除目录。
    pub fn cache_web_attachment_from_upload(
        &self,
        result: crate::features::files::file_ingest::IngestResult,
        upload_dir: PathBuf,
    ) -> Result<WebAttachmentSummary, String> {
        self.cache_web_attachment_entry(result, Some(upload_dir))
    }

    fn cache_web_attachment_entry(
        &self,
        result: crate::features::files::file_ingest::IngestResult,
        upload_dir: Option<PathBuf>,
    ) -> Result<WebAttachmentSummary, String> {
        let bytes = serde_json::to_vec(&result)
            .map_err(|error| format!("serialize Web attachment: {error}"))?
            .len();
        if bytes > MAX_WEB_ATTACHMENT_BYTES {
            return Err(format!(
                "解析后的附件超过远程控制桌面缓存上限 {} MiB",
                MAX_WEB_ATTACHMENT_BYTES / (1024 * 1024)
            ));
        }
        let handle = format!(
            "attachment_{}",
            crate::features::remote_control::short_token(32)
        );
        let summary = WebAttachmentSummary {
            handle: handle.clone(),
            kind: result.kind.clone(),
            basename: result.basename.clone(),
            token_estimate: result.token_estimate,
            byte_size: result.byte_size,
            warning: result.warning.clone(),
        };
        let mut inner = self.inner.lock();
        let mut eviction_attempts = inner.web_attachment_order.len();
        while (inner.web_attachments.len() >= MAX_WEB_ATTACHMENTS
            || inner.web_attachment_bytes.saturating_add(bytes) > MAX_WEB_ATTACHMENTS_TOTAL_BYTES)
            && eviction_attempts > 0
        {
            eviction_attempts -= 1;
            let Some(oldest) = inner.web_attachment_order.pop_front() else {
                break;
            };
            if inner
                .web_attachments
                .get(&oldest)
                .is_some_and(|cached| cached.reservation_id.is_some())
            {
                inner.web_attachment_order.push_back(oldest);
                continue;
            }
            if let Some(removed) = inner.web_attachments.remove(&oldest) {
                inner.web_attachment_bytes =
                    inner.web_attachment_bytes.saturating_sub(removed.bytes);
                remove_web_attachment_upload_dir(&removed);
            }
        }
        if inner.web_attachment_bytes.saturating_add(bytes) > MAX_WEB_ATTACHMENTS_TOTAL_BYTES {
            return Err("远程控制附件缓存已满，请移除一个附件后重试".into());
        }
        inner.web_attachment_bytes = inner.web_attachment_bytes.saturating_add(bytes);
        inner.web_attachment_order.push_back(handle.clone());
        inner.web_attachments.insert(
            handle,
            CachedWebAttachment {
                result,
                bytes,
                reservation_id: None,
                discard_requested: false,
                upload_dir,
            },
        );
        Ok(summary)
    }

    /// Atomically reserve opaque handles while a chat submission crosses the
    /// async engine admission boundary. A rejected turn releases them for the
    /// same browser to retry; only an accepted turn consumes them.
    pub fn reserve_web_attachments(
        &self,
        handles: &[String],
    ) -> Result<
        (
            String,
            Vec<crate::features::files::file_ingest::IngestResult>,
        ),
        String,
    > {
        if handles.len() > MAX_WEB_ATTACHMENTS {
            return Err("单条消息中的远程控制附件过多".into());
        }
        let mut inner = self.inner.lock();
        let mut unique = HashSet::new();
        for handle in handles {
            if !unique.insert(handle.as_str()) {
                return Err("远程控制附件句柄重复".into());
            }
            if !inner.web_attachments.contains_key(handle) {
                return Err(format!("远程控制附件句柄不存在或已过期：{handle}"));
            }
            if inner
                .web_attachments
                .get(handle)
                .and_then(|cached| cached.reservation_id.as_ref())
                .is_some()
            {
                return Err(format!("远程控制附件句柄已在使用：{handle}"));
            }
        }
        let reservation_id = format!(
            "attachment_use_{}",
            crate::features::remote_control::short_token(24)
        );
        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let cached = inner
                .web_attachments
                .get_mut(handle)
                .expect("attachment presence checked above");
            cached.reservation_id = Some(reservation_id.clone());
            results.push(cached.result.clone());
        }
        Ok((reservation_id, results))
    }

    /// Discard an opaque attachment handle after its browser chip is removed.
    /// Unknown handles are accepted for idempotency. A handle currently owned
    /// by a chat turn is marked for deletion and cleaned when that reservation
    /// finishes, regardless of whether the turn succeeds or fails.
    pub fn discard_web_attachment(&self, handle: &str) -> Result<(), String> {
        if handle.len() < 12
            || handle.len() > 128
            || !handle.starts_with("attachment_")
            || !handle
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("远程控制附件句柄无效".into());
        }
        request_web_attachment_discard(&mut self.inner.lock(), handle);
        Ok(())
    }

    pub fn finish_web_attachment_reservation(
        &self,
        reservation_id: &str,
        handles: &[String],
        consume: bool,
    ) -> Result<(), String> {
        finish_web_attachment_reservation_inner(
            &mut self.inner.lock(),
            reservation_id,
            handles,
            consume,
        )
    }

    /// 累积浏览器本机文件的一个有界分块；最后一块 `commit` 时返回完整文件名
    /// 与字节。校验、容量与 TTL 语义对齐 `append_web_session_upload`，单文件
    /// 上限收紧为 `file_ingest::MAX_FILE_BYTES`，避免传完 20 MiB 才拿到一个
    /// oversize 降级附件。
    pub fn append_web_attachment_upload(
        &self,
        upload_id: &str,
        file_name: &str,
        offset: usize,
        total: usize,
        data: &[u8],
        commit: bool,
    ) -> Result<Option<(String, Vec<u8>)>, String> {
        append_web_attachment_upload_chunk(
            &mut self.inner.lock(),
            upload_id,
            file_name,
            offset,
            total,
            data,
            commit,
        )
    }

    /// 取消一个进行中的浏览器本机上传并释放其内存缓冲。对未知或已完成的
    /// ID 幂等——迟到的取消不应产生错误。
    pub fn abort_web_attachment_upload(&self, upload_id: &str) {
        discard_web_attachment_upload(&mut self.inner.lock(), upload_id);
    }

    pub fn append_web_session_upload(
        &self,
        upload_id: &str,
        session_id: &str,
        expected_revision: &str,
        offset: usize,
        total: usize,
        data: &[u8],
        commit: bool,
    ) -> Result<Option<Vec<u8>>, String> {
        if upload_id.len() < 8
            || upload_id.len() > 128
            || !upload_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("远程控制会话上传 ID 无效".into());
        }
        if total > MAX_WEB_SESSION_UPLOAD_BYTES {
            return Err(format!(
                "会话数据超过远程控制 {} MiB 上限",
                MAX_WEB_SESSION_UPLOAD_BYTES / (1024 * 1024)
            ));
        }
        if expected_revision.trim().is_empty() || expected_revision.len() > 128 {
            return Err("远程控制会话上传需要有效的预期版本".into());
        }
        let mut inner = self.inner.lock();
        prune_expired_web_session_transfers(&mut inner);
        if offset == 0 {
            if let Some(previous) = inner.web_session_uploads.remove(upload_id) {
                drop(previous);
            }
            while inner.web_session_uploads.len() >= MAX_WEB_SESSION_UPLOADS {
                let Some(oldest) = inner.web_session_upload_order.pop_front() else {
                    break;
                };
                inner.web_session_uploads.remove(&oldest);
            }
            inner.web_session_upload_order.retain(|id| id != upload_id);
            inner
                .web_session_upload_order
                .push_back(upload_id.to_string());
            inner.web_session_uploads.insert(
                upload_id.to_string(),
                WebSessionUpload {
                    session_id: session_id.to_string(),
                    expected_revision: expected_revision.to_string(),
                    total,
                    data: Vec::with_capacity(total.min(4 * 1024 * 1024)),
                    last_touched: Instant::now(),
                },
            );
        }
        let retained_upload_bytes: usize = inner
            .web_session_uploads
            .iter()
            .filter(|(id, _)| id.as_str() != upload_id)
            .map(|(_, upload)| upload.data.len())
            .sum();
        if retained_upload_bytes
            .saturating_add(offset)
            .saturating_add(data.len())
            > MAX_WEB_SESSION_UPLOAD_TOTAL_BYTES
        {
            return Err("远程控制会话上传缓存超过总容量上限".into());
        }
        let upload = inner
            .web_session_uploads
            .get_mut(upload_id)
            .ok_or_else(|| "远程控制会话上传不存在或已过期".to_string())?;
        if upload.session_id != session_id
            || upload.expected_revision != expected_revision
            || upload.total != total
        {
            return Err("远程控制会话上传元数据已变化".into());
        }
        if upload.data.len() != offset {
            return Err(format!(
                "远程控制会话上传预期偏移量为 {}，实际为 {offset}",
                upload.data.len()
            ));
        }
        if upload.data.len().saturating_add(data.len()) > total {
            return Err("远程控制会话上传超过声明大小".into());
        }
        upload.data.extend_from_slice(data);
        upload.last_touched = Instant::now();
        if !commit {
            return Ok(None);
        }
        if upload.data.len() != total {
            return Err(format!(
                "远程控制会话上传不完整：已上传 {} / {total} 字节",
                upload.data.len()
            ));
        }
        let completed = inner
            .web_session_uploads
            .remove(upload_id)
            .expect("upload exists above")
            .data;
        inner.web_session_upload_order.retain(|id| id != upload_id);
        Ok(Some(completed))
    }

    pub fn begin_web_session_download(
        &self,
        session_id: &str,
        reserved_bytes: usize,
    ) -> Result<WebSessionDownloadReservation, String> {
        if reserved_bytes > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
            return Err(format!(
                "会话数据超过远程控制 {} MiB 上限",
                MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
            ));
        }
        let download_id = format!(
            "download_{}",
            crate::features::remote_control::short_token(32)
        );
        let download_dir = paths::pinvou3_home().join("web-session-downloads");
        std::fs::create_dir_all(&download_dir)
            .map_err(|error| format!("创建远程控制会话下载目录失败：{error}"))?;
        let path = download_dir.join(format!("{download_id}.json"));
        let mut inner = self.inner.lock();
        prune_expired_web_session_transfers(&mut inner);
        ensure_web_session_download_capacity(&inner, reserved_bytes)?;
        inner
            .web_session_download_order
            .push_back(download_id.clone());
        inner.web_session_downloads.insert(
            download_id.clone(),
            WebSessionDownload {
                session_id: session_id.to_string(),
                path: path.clone(),
                reserved_bytes,
                total: 0,
                ready: false,
                last_touched: Instant::now(),
            },
        );
        Ok(WebSessionDownloadReservation {
            manager: self.clone(),
            download_id,
            session_id: session_id.to_string(),
            path,
            committed: false,
        })
    }

    fn commit_web_session_download(
        &self,
        download_id: &str,
        session_id: &str,
        path: &Path,
    ) -> Result<(), String> {
        let total = std::fs::metadata(path)
            .map_err(|error| format!("检查远程控制会话下载数据失败：{error}"))?
            .len();
        let total = usize::try_from(total).map_err(|_| "远程控制会话下载数据过大".to_string())?;
        if total > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
            return Err(format!(
                "会话数据超过远程控制 {} MiB 上限",
                MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
            ));
        }

        let mut inner = self.inner.lock();
        prune_expired_web_session_transfers(&mut inner);
        let reserved = inner
            .web_session_downloads
            .get(download_id)
            .ok_or_else(|| "远程控制会话下载预留已过期".to_string())?;
        if reserved.session_id != session_id || reserved.path != path {
            return Err("远程控制会话下载预留已变化".into());
        }
        let other_bytes = inner
            .web_session_downloads
            .iter()
            .filter(|(id, _)| id.as_str() != download_id)
            .map(|(_, download)| download.reserved_bytes)
            .sum::<usize>();
        if other_bytes.saturating_add(total) > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
            return Err(format!(
                "进行中的远程控制会话下载将超过 {} MiB 总上限",
                MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
            ));
        }
        let download = inner
            .web_session_downloads
            .get_mut(download_id)
            .expect("reservation checked above");
        download.reserved_bytes = total;
        download.total = total;
        download.ready = true;
        download.last_touched = Instant::now();
        Ok(())
    }

    fn remove_web_session_download(&self, download_id: &str, expected_path: Option<&Path>) {
        let removed = {
            let mut inner = self.inner.lock();
            let matches = inner
                .web_session_downloads
                .get(download_id)
                .is_some_and(|download| {
                    expected_path.is_none_or(|path| download.path.as_path() == path)
                });
            if matches {
                inner
                    .web_session_download_order
                    .retain(|id| id != download_id);
                inner.web_session_downloads.remove(download_id)
            } else {
                None
            }
        };
        if let Some(path) = removed
            .map(|download| download.path)
            .or_else(|| expected_path.map(Path::to_path_buf))
        {
            // The reservation path is unique. Removing it even when the map
            // entry was concurrently cleared closes the cancellation/crash
            // race where Windows could not unlink an open writer earlier.
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn read_web_session_download(
        &self,
        download_id: &str,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<WebSessionDownloadChunk, String> {
        let mut inner = self.inner.lock();
        prune_expired_web_session_transfers(&mut inner);
        let download = inner
            .web_session_downloads
            .get_mut(download_id)
            .ok_or_else(|| "远程控制会话下载不存在或已过期".to_string())?;
        if download.session_id != session_id {
            return Err("远程控制会话下载属于另一个会话".into());
        }
        if !download.ready {
            return Err("远程控制会话下载仍在准备中".into());
        }
        if offset > download.total {
            return Err(format!(
                "Session offset {offset} exceeds payload size {}",
                download.total
            ));
        }
        let total = download.total;
        let end = offset.saturating_add(limit).min(total);
        let path = download.path.clone();
        let mut file =
            File::open(&path).map_err(|error| format!("打开远程控制会话下载失败：{error}"))?;
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|error| format!("定位远程控制会话下载失败：{error}"))?;
        let mut data = vec![0_u8; end.saturating_sub(offset)];
        file.read_exact(&mut data)
            .map_err(|error| format!("读取远程控制会话下载失败：{error}"))?;
        drop(file);
        download.last_touched = Instant::now();
        let eof = end == total;
        if eof {
            inner.web_session_downloads.remove(download_id);
            inner
                .web_session_download_order
                .retain(|id| id != download_id);
            let _ = std::fs::remove_file(path);
        }
        Ok(WebSessionDownloadChunk { total, data, eof })
    }

    pub fn status(&self) -> WebAccessStatus {
        let inner = self.inner.lock();
        if let Some(endpoint) = &inner.endpoint {
            WebAccessStatus {
                active: true,
                endpoint_id: Some(endpoint.config.endpoint_id.clone()),
                url: Some(endpoint.url.clone()),
                qr_data_url: crate::features::connectors::connector_cli::make_qr(&endpoint.url),
                status: endpoint.status,
                relay_url: endpoint.config.relay_url.clone(),
                web_client_connected: endpoint.web_client_connected,
                last_error: endpoint.last_error.clone(),
            }
        } else {
            WebAccessStatus {
                active: false,
                endpoint_id: None,
                url: None,
                qr_data_url: None,
                status: inner.idle_status,
                relay_url: remote_relay_ws_url(),
                web_client_connected: false,
                last_error: inner
                    .idle_error
                    .clone()
                    .or_else(|| self.policy_error.clone()),
            }
        }
    }

    pub fn forward_local_event(&self, event: &str, payload: Value) {
        if let Err(error) = self.publish_event_inner(event, payload, EventSource::Rust) {
            eprintln!("[web-access] forward {event} failed: {error}");
        }
    }

    pub fn publish_frontend_event(&self, event: &str, payload: Value) -> Result<(), String> {
        self.publish_event_inner(event, payload, EventSource::Frontend)
    }

    /// Register a concrete desktop WebView generation. Unacknowledged work may
    /// be dispatched to a new generation; work whose begin ACK was persisted
    /// is never re-run and is resolved as `outcome_unknown` instead.
    pub fn mark_frontend_ready(&self, generation: String) -> Result<(), String> {
        validate_bridge_generation(&generation)?;
        let (actions, subscriptions) = {
            let mut inner = self.inner.lock();
            let actions = prepare_bridge_generation(&mut inner, &generation);
            let subscriptions = inner.event_stream.subscription_names();
            (actions, subscriptions)
        };
        let mut first_error = None;
        for action in actions {
            match action {
                RpcReadyAction::Dispatch(dispatch) => {
                    if let Err(error) = self.emit_rpc_dispatch(dispatch) {
                        first_error.get_or_insert(error);
                    }
                }
                RpcReadyAction::Respond(sender, response) => {
                    if sender.try_send(RelayOutbound::Message(response)).is_err() {
                        first_error
                            .get_or_insert_with(|| "Web access relay task has stopped".to_string());
                    }
                }
            }
        }
        // The browser may reconnect and subscribe before a restarted desktop
        // WebView has installed its forwarding listeners. Replaying the set
        // after the main bridge readiness barrier closes that lost-event race.
        for event in subscriptions {
            if let Err(error) = self.app.emit_to(
                "main",
                "web_access:event_subscribe",
                json!({ "event": event }),
            ) {
                first_error.get_or_insert_with(|| {
                    format!("restore Web access frontend subscription: {error}")
                });
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    /// Persist the side-effect barrier before allowing the desktop WebView to
    /// invoke the requested command. Returning `false` means this dispatch was
    /// already acknowledged and must not be executed again.
    pub fn begin_rpc(&self, request_id: &str, generation: &str) -> Result<bool, String> {
        validate_bridge_generation(generation)?;
        let mut inner = self.inner.lock();
        if inner.bridge_generation.as_deref() != Some(generation) {
            return Err("stale WebView bridge generation".to_string());
        }
        let (fingerprint, lease_id, already_acknowledged) = match inner.rpc_cache.get(request_id) {
            Some(RpcCacheEntry::Pending(pending)) => {
                if pending.dispatched_generation.as_deref() != Some(generation) {
                    return Err("RPC was not dispatched to this WebView generation".to_string());
                }
                (
                    pending.fingerprint.clone(),
                    pending.lease_id.clone(),
                    pending.acknowledged_generation.is_some(),
                )
            }
            Some(RpcCacheEntry::Complete { .. }) => return Ok(false),
            None => return Err(format!("unknown Web RPC request: {request_id}")),
        };
        if already_acknowledged {
            return Ok(false);
        }

        let mut next_ledger = inner.rpc_ledger.clone();
        next_ledger.acknowledge(request_id, &fingerprint)?;
        if let Err(error) = persist_rpc_ledger(&next_ledger) {
            // Never leave this generation in a dispatched-but-unacknowledged
            // limbo. Return a deterministic terminal response without running
            // the application command when the durable side-effect barrier
            // cannot be committed.
            let completion = rpc_error_completion(
                "rpc_ledger_unavailable",
                format!("cannot durably begin Web RPC: {error}"),
            );
            inner.rpc_cache.insert(
                request_id.to_string(),
                RpcCacheEntry::Complete {
                    fingerprint,
                    completion: completion.clone(),
                },
            );
            prune_rpc_cache(&mut inner);
            if let Some(endpoint) = inner.endpoint.as_ref() {
                let response = rpc_response(
                    &endpoint.config.endpoint_id,
                    &lease_id,
                    request_id,
                    &completion,
                );
                let _ = endpoint.sender.try_send(RelayOutbound::Message(response));
            }
            return Ok(false);
        }
        inner.rpc_ledger = next_ledger;
        let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get_mut(request_id) else {
            return Err(format!("unknown Web RPC request: {request_id}"));
        };
        pending.acknowledged_generation = Some(generation.to_string());
        Ok(true)
    }

    pub fn complete_rpc(
        &self,
        request_id: &str,
        generation: &str,
        ok: bool,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<(), String> {
        validate_bridge_generation(generation)?;
        let (sender, response) = {
            let mut inner = self.inner.lock();
            if inner.bridge_generation.as_deref() != Some(generation) {
                return Err("stale WebView bridge generation".to_string());
            }
            let (lease_id, fingerprint) = match inner.rpc_cache.get(request_id) {
                Some(RpcCacheEntry::Pending(pending)) => {
                    if pending.acknowledged_generation.as_deref() != Some(generation) {
                        return Err("Web RPC completion arrived before its begin ACK".to_string());
                    }
                    (pending.lease_id.clone(), pending.fingerprint.clone())
                }
                Some(RpcCacheEntry::Complete { .. }) => return Ok(()),
                None => return Err(format!("unknown Web RPC request: {request_id}")),
            };
            let completion = bounded_rpc_completion(ok, result, error);
            inner.rpc_cache.insert(
                request_id.to_string(),
                RpcCacheEntry::Complete {
                    fingerprint,
                    completion: completion.clone(),
                },
            );
            prune_rpc_cache(&mut inner);
            let endpoint = inner
                .endpoint
                .as_ref()
                .ok_or_else(|| "Web access endpoint is not active".to_string())?;
            (
                endpoint.sender.clone(),
                rpc_response(
                    &endpoint.config.endpoint_id,
                    &lease_id,
                    request_id,
                    &completion,
                ),
            )
        };
        sender
            .try_send(RelayOutbound::Message(response))
            .map_err(|_| "Web access relay task has stopped".to_string())
    }

    fn ensure_policy(&self) -> Result<(), String> {
        match &self.policy_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn activate(&self, config: WebAccessConfig) -> Result<(), String> {
        let rpc_ledger = load_or_initialize_rpc_ledger(&config.endpoint_id)?;
        let url = public_url(&config);
        let (sender, mut receiver) = relay_client::spawn(config.clone());
        {
            let mut inner = self.inner.lock();
            inner.idle_error = None;
            inner.idle_status = WebAccessStatusKind::Idle;
            inner.rpc_ledger = rpc_ledger;
            inner.rpc_cache.clear();
            inner.rpc_order.clear();
            inner.endpoint = Some(ActiveEndpoint {
                config: config.clone(),
                url,
                status: WebAccessStatusKind::ConnectingRelay,
                web_client_connected: false,
                client_ready: false,
                lease_id: None,
                last_lease_id: None,
                fresh_baseline_seq: None,
                last_error: None,
                sender,
            });
        }
        self.emit_status();

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(inbound) = receiver.recv().await {
                match inbound {
                    RelayInbound::Message(text) => match serde_json::from_str::<Value>(&text) {
                        Ok(value) => manager.handle_relay_message(value),
                        Err(error) => {
                            eprintln!("[web-access] queued relay JSON ignored: {error}")
                        }
                    },
                    RelayInbound::Connection {
                        endpoint_id,
                        connected,
                        error,
                    } => manager.handle_connection(&endpoint_id, connected, error),
                }
            }
        });
        Ok(())
    }

    fn handle_connection(&self, endpoint_id: &str, connected: bool, error: Option<String>) {
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            if endpoint.config.endpoint_id != endpoint_id {
                return;
            }
            if connected {
                endpoint.status = WebAccessStatusKind::ConnectingRelay;
                endpoint.last_error = None;
            } else {
                endpoint.status = WebAccessStatusKind::ConnectingRelay;
                endpoint.web_client_connected = false;
                endpoint.lease_id = None;
                endpoint.fresh_baseline_seq = None;
                endpoint.last_error = error;
            }
        }
        self.emit_status();
    }

    fn handle_relay_message(&self, value: Value) {
        let endpoint_id = value
            .get("endpoint_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !self.is_current_endpoint(endpoint_id) {
            return;
        }
        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "desktop_endpoint_registered" => self.handle_registered(&value),
            "web_client_connected" => self.handle_web_connected(&value),
            "web_client_disconnected" => self.handle_web_disconnected(&value),
            "rpc_request" => self.handle_rpc_request(&value),
            "event_subscribe" => self.handle_event_subscription(&value, true),
            "event_unsubscribe" => self.handle_event_subscription(&value, false),
            "client_ready" => self.synchronize_client_from_value(&value),
            "desktop_endpoint_revoked" => self.handle_revoked(endpoint_id),
            "desktop_endpoint_replaced" => self.handle_replaced(endpoint_id),
            "error" => {
                let error = value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("relay rejected Web access request")
                    .to_string();
                self.set_endpoint_error(error);
            }
            _ => {}
        }
    }

    fn handle_registered(&self, value: &Value) {
        let connected = value
            .get("web_client_connected")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        {
            let mut inner = self.inner.lock();
            let lease_changed = connected
                && inner
                    .endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.last_lease_id.as_ref())
                    != lease_id.as_ref();
            if lease_changed {
                inner.event_stream.clear_subscriptions();
                clear_web_attachments(&mut inner);
            }
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            endpoint.web_client_connected = connected;
            endpoint.client_ready = false;
            endpoint.fresh_baseline_seq = None;
            if connected {
                endpoint.lease_id = lease_id.clone();
                endpoint.last_lease_id = lease_id;
            } else {
                endpoint.lease_id = None;
            }
            endpoint.status = if connected {
                WebAccessStatusKind::WebClientConnected
            } else {
                WebAccessStatusKind::WaitingWebClient
            };
            endpoint.last_error = None;
        }
        self.emit_status();
    }

    fn handle_web_connected(&self, value: &Value) {
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if lease_id.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.lock();
            let fresh_client = value
                .get("stream_epoch")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty);
            let fresh_baseline_seq = fresh_client.then_some(inner.event_stream.seq());
            let lease_changed = inner
                .endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.last_lease_id.as_deref())
                != Some(lease_id.as_str());
            if lease_changed {
                // Subscriptions belong to a concrete Web lease. A takeover or
                // reload must rebuild them before `client_ready` requests a
                // replay, otherwise old subscriptions can advance the new
                // client's event cursor before it is ready.
                inner.event_stream.clear_subscriptions();
                clear_web_attachments(&mut inner);
            }
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            endpoint.web_client_connected = true;
            endpoint.client_ready = false;
            endpoint.lease_id = Some(lease_id.clone());
            endpoint.last_lease_id = Some(lease_id.clone());
            endpoint.fresh_baseline_seq = fresh_baseline_seq;
            endpoint.status = WebAccessStatusKind::WebClientConnected;
            endpoint.last_error = None;
            // A pending request may finish just after a reconnect. Route that
            // first completion to the new valid lease; a completed request is
            // still replayed from cache if the browser resends it.
            for entry in inner.rpc_cache.values_mut() {
                if let RpcCacheEntry::Pending(pending) = entry {
                    pending.lease_id = lease_id.clone();
                }
            }
        }
        self.emit_status();
    }

    fn handle_web_disconnected(&self, value: &Value) {
        let disconnected_lease = value.get("lease_id").and_then(Value::as_str);
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            if disconnected_lease.is_some() && endpoint.lease_id.as_deref() != disconnected_lease {
                return;
            }
            endpoint.web_client_connected = false;
            endpoint.client_ready = false;
            endpoint.lease_id = None;
            endpoint.fresh_baseline_seq = None;
            endpoint.status = WebAccessStatusKind::WebClientDisconnected;
        }
        self.emit_status();
    }

    fn handle_revoked(&self, endpoint_id: &str) {
        let revoked = {
            let mut inner = self.inner.lock();
            if inner
                .endpoint
                .as_ref()
                .is_none_or(|endpoint| endpoint.config.endpoint_id != endpoint_id)
            {
                return;
            }
            let revoked = inner.endpoint.take();
            inner.event_stream.clear_subscriptions();
            inner.rpc_ledger = RpcLedger::default();
            inner.rpc_cache.clear();
            inner.rpc_order.clear();
            clear_web_attachments(&mut inner);
            inner.event_stream.reset();
            inner.idle_status = WebAccessStatusKind::Revoked;
            inner.idle_error = None;
            revoked
        };
        if let Some(endpoint) = revoked {
            let _ = endpoint.sender.try_send(RelayOutbound::Shutdown);
        }
        if let Err(error) = remove_config() {
            eprintln!("[web-access] remove revoked config failed: {error}");
        }
        if let Err(error) = remove_rpc_ledger() {
            eprintln!("[web-access] remove revoked RPC ledger failed: {error}");
        }
        self.emit_status();
    }

    fn handle_replaced(&self, endpoint_id: &str) {
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_mut() else {
                return;
            };
            if endpoint.config.endpoint_id != endpoint_id {
                return;
            }
            endpoint.status = WebAccessStatusKind::Error;
            endpoint.web_client_connected = false;
            endpoint.client_ready = false;
            endpoint.lease_id = None;
            endpoint.fresh_baseline_seq = None;
            endpoint.last_error = Some(
                "this endpoint is active in another desktop process; rotate Web access to take a new endpoint"
                    .to_string(),
            );
            inner.event_stream.clear_subscriptions();
            inner.rpc_cache.clear();
            inner.rpc_order.clear();
            clear_web_attachments(&mut inner);
        }
        self.emit_status();
    }

    fn handle_event_subscription(&self, value: &Value, subscribe: bool) {
        let Some(event) = value.get("event").and_then(Value::as_str) else {
            return;
        };
        if !self.policy.events.contains(event) {
            return;
        }
        let changed = {
            let mut inner = self.inner.lock();
            inner.event_stream.set_subscription(event, subscribe)
        };
        if changed {
            let bridge_event = if subscribe {
                "web_access:event_subscribe"
            } else {
                "web_access:event_unsubscribe"
            };
            let _ = self
                .app
                .emit_to("main", bridge_event, json!({ "event": event }));
        }
    }

    fn handle_rpc_request(&self, value: &Value) {
        let request_id = value
            .get("client_request_id")
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let command = value
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if request_id.is_empty() || request_id.len() > 256 || lease_id.is_empty() {
            return;
        }
        let args = value.get("args").cloned().unwrap_or_else(|| json!({}));
        let command_error = validate_rpc_command(&command).err();
        let fingerprint_command = if command_error.is_some() {
            "__invalid_web_rpc_command__"
        } else {
            command.as_str()
        };
        let fingerprint = match rpc_fingerprint(fingerprint_command, &args) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                self.set_endpoint_error(error);
                return;
            }
        };

        let allowed = command_error.is_none() && self.policy.commands.contains(&command);
        let preflight_completion = if let Some(error) = command_error {
            Some(rpc_error_completion("invalid_command", error))
        } else if serde_json::to_vec(&args)
            .map(|encoded| encoded.len() > MAX_RPC_REQUEST_BYTES)
            .unwrap_or(true)
        {
            Some(rpc_error_completion(
                "request_too_large",
                format!(
                    "远程控制请求参数超过 {} MiB 上限",
                    MAX_RPC_REQUEST_BYTES / (1024 * 1024)
                ),
            ))
        } else if allowed {
            validate_web_rpc_scope(&self.app, &command, &args)
                .err()
                .map(|error| rpc_error_completion("invalid_session_scope", error))
        } else {
            None
        };
        let action = {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_ref() else {
                return;
            };
            let endpoint_id = endpoint.config.endpoint_id.clone();
            let sender = endpoint.sender.clone();
            let bridge_generation = inner.bridge_generation.clone();
            // 浏览器已在 180 秒超时后放弃的卡死请求，超过 TTL 后转为终态错误，
            // 释放 in-flight 槽位。迟到的 begin/complete 会命中 Complete 并被幂等忽略。
            let now = Instant::now();
            let expired: Vec<String> = inner
                .rpc_cache
                .iter()
                .filter_map(|(id, entry)| match entry {
                    RpcCacheEntry::Pending(pending)
                        if rpc_in_flight_expired(pending.dispatched_at, now) =>
                    {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect();
            for id in expired {
                if let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get(&id) {
                    let fingerprint = pending.fingerprint.clone();
                    inner.rpc_cache.insert(
                        id,
                        RpcCacheEntry::Complete {
                            fingerprint,
                            completion: rpc_error_completion(
                                "rpc_timed_out",
                                "远程控制请求执行超时，已释放该请求",
                            ),
                        },
                    );
                }
            }
            let in_flight_count = inner
                .rpc_cache
                .values()
                .filter(|entry| matches!(entry, RpcCacheEntry::Pending(_)))
                .count();
            match inner.rpc_cache.get_mut(&request_id) {
                Some(RpcCacheEntry::Pending(pending)) => {
                    if pending.fingerprint != fingerprint {
                        RpcRequestAction::Respond(
                            sender,
                            rpc_response(
                                &endpoint_id,
                                &lease_id,
                                &request_id,
                                &request_conflict_completion(),
                            ),
                        )
                    } else {
                        pending.lease_id = lease_id;
                        if pending.acknowledged_generation.is_none()
                            && bridge_generation.is_some()
                            && pending.dispatched_generation != bridge_generation
                        {
                            pending.dispatched_generation = bridge_generation.clone();
                            RpcRequestAction::Dispatch(RpcDispatch {
                                request_id: request_id.clone(),
                                command: pending.command.clone(),
                                args: pending.args.clone(),
                                bridge_generation: bridge_generation.unwrap_or_default(),
                            })
                        } else {
                            RpcRequestAction::None
                        }
                    }
                }
                Some(RpcCacheEntry::Complete {
                    fingerprint: existing_fingerprint,
                    completion,
                }) => {
                    let completion = if existing_fingerprint == &fingerprint {
                        completion.clone()
                    } else {
                        request_conflict_completion()
                    };
                    RpcRequestAction::Respond(
                        sender,
                        rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                    )
                }
                None => {
                    if let Some(tombstone) = inner.rpc_ledger.get(&request_id).cloned() {
                        if tombstone.fingerprint != fingerprint || tombstone.acknowledged {
                            let completion = tombstone_completion(&tombstone, &fingerprint);
                            inner.rpc_cache.insert(
                                request_id.clone(),
                                RpcCacheEntry::Complete {
                                    fingerprint: tombstone.fingerprint,
                                    completion: completion.clone(),
                                },
                            );
                            inner.rpc_order.push_back(request_id.clone());
                            prune_rpc_cache(&mut inner);
                            RpcRequestAction::Respond(
                                sender,
                                rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                            )
                        } else {
                            // `remember` was durable but the desktop never
                            // acknowledged execution. Reconstructing the
                            // pending request is safe after a process/WebView
                            // restart; only acknowledged tombstones become
                            // outcome_unknown.
                            let rejection = rpc_admission_rejection(
                                preflight_completion.as_ref(),
                                allowed,
                                in_flight_count,
                            );
                            if let Some(completion) = rejection {
                                inner.rpc_cache.insert(
                                    request_id.clone(),
                                    RpcCacheEntry::Complete {
                                        fingerprint: fingerprint.clone(),
                                        completion: completion.clone(),
                                    },
                                );
                                inner.rpc_order.push_back(request_id.clone());
                                prune_rpc_cache(&mut inner);
                                RpcRequestAction::Respond(
                                    sender,
                                    rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                                )
                            } else {
                                inner.rpc_cache.insert(
                                    request_id.clone(),
                                    RpcCacheEntry::Pending(PendingRpc {
                                        lease_id,
                                        command: command.clone(),
                                        args: args.clone(),
                                        fingerprint,
                                        dispatched_generation: bridge_generation.clone(),
                                        acknowledged_generation: None,
                                        dispatched_at: Instant::now(),
                                    }),
                                );
                                inner.rpc_order.push_back(request_id.clone());
                                prune_rpc_cache(&mut inner);
                                if let Some(bridge_generation) = bridge_generation {
                                    RpcRequestAction::Dispatch(RpcDispatch {
                                        request_id: request_id.clone(),
                                        command,
                                        args,
                                        bridge_generation,
                                    })
                                } else {
                                    RpcRequestAction::None
                                }
                            }
                        }
                    } else {
                        match prepare_new_rpc_admission(
                            &inner.rpc_ledger,
                            &request_id,
                            &fingerprint,
                            preflight_completion.as_ref(),
                            allowed,
                            in_flight_count,
                        ) {
                            Ok(NewRpcAdmission::Rejected(completion)) => {
                                inner.rpc_cache.insert(
                                    request_id.clone(),
                                    RpcCacheEntry::Complete {
                                        fingerprint: fingerprint.clone(),
                                        completion: completion.clone(),
                                    },
                                );
                                inner.rpc_order.push_back(request_id.clone());
                                prune_rpc_cache(&mut inner);
                                RpcRequestAction::Respond(
                                    sender,
                                    rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                                )
                            }
                            Ok(NewRpcAdmission::Durable(next_ledger)) => {
                                if let Err(error) = persist_rpc_ledger(&next_ledger) {
                                    let completion = rpc_error_completion(
                                        "rpc_ledger_unavailable",
                                        format!("cannot durably accept Web RPC: {error}"),
                                    );
                                    inner.rpc_cache.insert(
                                        request_id.clone(),
                                        RpcCacheEntry::Complete {
                                            fingerprint: fingerprint.clone(),
                                            completion: completion.clone(),
                                        },
                                    );
                                    inner.rpc_order.push_back(request_id.clone());
                                    prune_rpc_cache(&mut inner);
                                    RpcRequestAction::Respond(
                                        sender,
                                        rpc_response(
                                            &endpoint_id,
                                            &lease_id,
                                            &request_id,
                                            &completion,
                                        ),
                                    )
                                } else {
                                    inner.rpc_ledger = next_ledger;
                                    inner.rpc_cache.insert(
                                        request_id.clone(),
                                        RpcCacheEntry::Pending(PendingRpc {
                                            lease_id,
                                            command: command.clone(),
                                            args: args.clone(),
                                            fingerprint,
                                            dispatched_generation: bridge_generation.clone(),
                                            acknowledged_generation: None,
                                            dispatched_at: Instant::now(),
                                        }),
                                    );
                                    inner.rpc_order.push_back(request_id.clone());
                                    prune_rpc_cache(&mut inner);
                                    if let Some(bridge_generation) = bridge_generation {
                                        RpcRequestAction::Dispatch(RpcDispatch {
                                            request_id: request_id.clone(),
                                            command,
                                            args,
                                            bridge_generation,
                                        })
                                    } else {
                                        RpcRequestAction::None
                                    }
                                }
                            }
                            Err(error) => {
                                let completion = rpc_error_completion(
                                    "rpc_ledger_unavailable",
                                    format!("cannot durably accept Web RPC: {error}"),
                                );
                                inner.rpc_cache.insert(
                                    request_id.clone(),
                                    RpcCacheEntry::Complete {
                                        fingerprint: fingerprint.clone(),
                                        completion: completion.clone(),
                                    },
                                );
                                inner.rpc_order.push_back(request_id.clone());
                                prune_rpc_cache(&mut inner);
                                RpcRequestAction::Respond(
                                    sender,
                                    rpc_response(&endpoint_id, &lease_id, &request_id, &completion),
                                )
                            }
                        }
                    }
                }
            }
        };

        match action {
            RpcRequestAction::None => {}
            RpcRequestAction::Respond(sender, response) => {
                let _ = sender.try_send(RelayOutbound::Message(response));
            }
            RpcRequestAction::Dispatch(dispatch) => {
                let _ = self.emit_rpc_dispatch(dispatch);
            }
        }
    }

    fn emit_rpc_dispatch(&self, dispatch: RpcDispatch) -> Result<(), String> {
        let request_id = dispatch.request_id;
        let generation = dispatch.bridge_generation;
        let emitted = self.app.emit_to(
            "main",
            "web_access:rpc_request",
            json!({
                "request_id": &request_id,
                "id": &request_id,
                "command": dispatch.command,
                "args": dispatch.args,
                "bridge_generation": &generation,
            }),
        );
        if let Err(error) = emitted {
            let mut inner = self.inner.lock();
            if let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get_mut(&request_id) {
                if pending.acknowledged_generation.is_none()
                    && pending.dispatched_generation.as_deref() == Some(generation.as_str())
                {
                    pending.dispatched_generation = None;
                }
            }
            return Err(format!("dispatch Web RPC to desktop UI: {error}"));
        }
        Ok(())
    }

    fn synchronize_client_from_value(&self, value: &Value) {
        let lease_id = value
            .get("lease_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if lease_id.is_empty() {
            return;
        }
        let requested_epoch = value
            .get("stream_epoch")
            .and_then(Value::as_str)
            .filter(|epoch| !epoch.is_empty());
        let after_seq = value.get("after_seq").and_then(Value::as_u64).unwrap_or(0);
        let state_ready = value
            .get("state_ready")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let mut capability_commands = self.policy.commands.iter().cloned().collect::<Vec<_>>();
        capability_commands.sort();
        let mut capability_events = self.policy.events.iter().cloned().collect::<Vec<_>>();
        capability_events.sort();
        {
            let mut inner = self.inner.lock();
            let Some(endpoint) = inner.endpoint.as_ref() else {
                return;
            };
            if endpoint.lease_id.as_deref() != Some(lease_id) {
                return;
            }
            let endpoint_id = endpoint.config.endpoint_id.clone();
            let sender = endpoint.sender.clone();
            let fresh_baseline_seq = endpoint.fresh_baseline_seq;
            let epoch_matches = requested_epoch
                .map(|epoch| epoch == inner.event_stream.epoch())
                .unwrap_or(false);
            if !state_ready {
                if requested_epoch.is_some()
                    && (!epoch_matches || after_seq > inner.event_stream.seq())
                {
                    inner.event_stream.reset();
                    let stream_epoch = inner.event_stream.epoch().to_string();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(
                            &endpoint_id,
                            lease_id,
                            &stream_epoch,
                            "epoch_changed",
                        ),
                    );
                    return;
                }
                let baseline = if requested_epoch.is_none() {
                    fresh_baseline_seq.unwrap_or(inner.event_stream.seq())
                } else {
                    after_seq
                };
                let snapshot = snapshot_message(
                    &endpoint_id,
                    lease_id,
                    inner.event_stream.epoch(),
                    baseline,
                    &capability_commands,
                    &capability_events,
                );
                if sender.try_send(RelayOutbound::Message(snapshot)).is_err() {
                    inner.event_stream.reset();
                    let stream_epoch = inner.event_stream.epoch().to_string();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(
                            &endpoint_id,
                            lease_id,
                            &stream_epoch,
                            "snapshot_enqueue_failed",
                        ),
                    );
                }
                return;
            }
            // A client without an epoch is a fresh browser/device, not a
            // reconnect from sequence zero. Give it a baseline snapshot at
            // the current head so historical interactive events are never
            // replayed as if they had just happened.
            let fresh_client = requested_epoch.is_none();
            let stream_epoch = inner.event_stream.epoch().to_string();
            let replay_context = ReplayMessageContext {
                endpoint_id: &endpoint_id,
                lease_id,
                stream_epoch: &stream_epoch,
                capability_commands: &capability_commands,
                capability_events: &capability_events,
            };
            let replay = if fresh_client {
                inner.event_stream.replay_messages_after(
                    fresh_baseline_seq.unwrap_or(inner.event_stream.seq()),
                    replay_context,
                )
            } else if epoch_matches {
                inner
                    .event_stream
                    .replay_messages_after(after_seq, replay_context)
            } else {
                None
            };

            let mut messages = match replay {
                Some(replay_messages) if fresh_client => {
                    let baseline = fresh_baseline_seq.unwrap_or(inner.event_stream.seq());
                    let mut messages = vec![snapshot_message(
                        &endpoint_id,
                        lease_id,
                        inner.event_stream.epoch(),
                        baseline,
                        &capability_commands,
                        &capability_events,
                    )];
                    messages.extend(replay_messages);
                    messages
                }
                Some(messages) => messages,
                None => {
                    // The browser reload behavior deliberately persists the new
                    // epoch with cursor zero. Resetting the journal here makes
                    // that next join satisfiable instead of causing a loop.
                    let reason = if fresh_client || epoch_matches {
                        "journal_gap"
                    } else {
                        "epoch_changed"
                    };
                    inner.event_stream.reset();
                    let stream_epoch = inner.event_stream.epoch().to_string();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(&endpoint_id, lease_id, &stream_epoch, reason),
                    );
                    return;
                }
            };
            if !messages.iter().any(|message| {
                message.get("type").and_then(Value::as_str) == Some("desktop_snapshot")
            }) {
                messages.push(snapshot_message(
                    &endpoint_id,
                    lease_id,
                    inner.event_stream.epoch(),
                    inner.event_stream.seq(),
                    &capability_commands,
                    &capability_events,
                ));
            }
            // Enqueue the complete replay while holding the stream lock. Live
            // events also enqueue under this lock, so seq N+1 cannot overtake
            // the replay suffix ending at N.
            if !try_enqueue_message_batch(messages, |message| {
                sender.try_send(RelayOutbound::Message(message)).is_ok()
            }) {
                // A partially enqueued suffix is always followed by a reset;
                // never advertise readiness after silently losing its tail.
                inner.event_stream.reset();
                let stream_epoch = inner.event_stream.epoch().to_string();
                if let Some(endpoint) = inner.endpoint.as_mut() {
                    endpoint.client_ready = false;
                    endpoint.fresh_baseline_seq = None;
                }
                enqueue_stream_reset(
                    sender,
                    stream_reset_message(
                        &endpoint_id,
                        lease_id,
                        &stream_epoch,
                        "replay_enqueue_failed",
                    ),
                );
                return;
            }
            if let Some(endpoint) = inner.endpoint.as_mut() {
                if endpoint.lease_id.as_deref() == Some(lease_id) {
                    endpoint.client_ready = true;
                    endpoint.fresh_baseline_seq = None;
                }
            }
        }
    }

    fn publish_event_inner(
        &self,
        event: &str,
        payload: Value,
        source: EventSource,
    ) -> Result<(), String> {
        if !self.policy.events.contains(event) {
            return Err(format!("远程控制不允许发送该事件：{event}"));
        }
        // The engine/file watcher already call `forward_app_event` directly.
        // The desktop WebView proxy also sees the corresponding Tauri event;
        // ignore that second source to keep sequence semantics exact.
        if source == EventSource::Frontend && RUST_FORWARDED_EVENTS.contains(&event) {
            return Ok(());
        }
        // 远程端正式支持代码会话列表/授权/UI 之前，先过滤原生代码会话事件：事件
        // payload 携带的会话 id（`session_id` 用于 chat:* / artifact:disk，
        // `id` 用于 session:*，`sessionId` 用于 scheduled_task:run_updated）指向
        // 原生代码会话（ACP/品悟）时不转发。远程 WebUI 不会收到它无法展示/授权
        // 的代码会话消息流；predicate 只对真实代码会话 id 返回 true，普通会话
        // 不受影响。
        if should_filter_code_session_event(
            self.inner.lock().code_session_predicate.as_ref(),
            &payload,
        ) {
            return Ok(());
        }
        {
            let mut inner = self.inner.lock();
            let endpoint = inner
                .endpoint
                .as_ref()
                .ok_or_else(|| "Web access endpoint is not active".to_string())?;
            let endpoint_id = endpoint.config.endpoint_id.clone();
            let lease_id = endpoint.lease_id.clone();
            let client_ready = endpoint.client_ready;
            let sender = endpoint.sender.clone();
            let subscribed = inner.event_stream.is_subscribed(event);
            // Journal every allowlisted event independently of the current
            // lease's subscribe handshake. A reconnect can otherwise lose
            // deltas emitted between `web_client_connected` and the browser's
            // subscribe/client_ready messages without leaving a sequence gap.
            let sizing_lease_id = lease_id.as_deref().unwrap_or(LEASE_ID_WIRE_PLACEHOLDER);
            let message = match inner.event_stream.record(
                &endpoint_id,
                sizing_lease_id,
                event.to_string(),
                payload,
            ) {
                Ok(message) => message,
                Err(error @ StreamRecordError::Oversized { .. }) => {
                    // `record` already rotated the epoch and cleared the
                    // journal atomically. Stop live delivery until the WebUI
                    // reloads and negotiates that new epoch, so no later event
                    // can overtake this recovery barrier.
                    let stream_epoch = inner.event_stream.epoch().to_string();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    if let Some(lease_id) = lease_id.as_deref() {
                        enqueue_stream_reset(
                            sender,
                            stream_reset_message(
                                &endpoint_id,
                                lease_id,
                                &stream_epoch,
                                "event_too_large",
                            ),
                        );
                    }
                    return Err(error.to_string());
                }
                Err(error) => return Err(error.to_string()),
            };
            let Some(lease_id) = lease_id.as_deref() else {
                // Keep the event in the journal for a reconnecting subscribed
                // browser, but there is no valid destination at this instant.
                return Ok(());
            };
            if !client_ready {
                // Subscriptions arrive before client_ready. Keep journaling,
                // but do not let a live high sequence overtake its replay.
                return Ok(());
            }
            if !subscribed {
                // 旧版 WebUI 没有协商的新事件不能直接推送，否则会把桌面端判为
                // 不兼容并断开；事件已写入 journal，后续订阅客户端仍可通过回放恢复。
                return Ok(());
            }
            debug_assert_eq!(
                message.get("lease_id").and_then(Value::as_str),
                Some(lease_id)
            );
            match sender.try_send(RelayOutbound::Message(message)) {
                Ok(()) => Ok(()),
                Err(send_error) => {
                    // The just-recorded event is the journal tail. Rebase it
                    // into a fresh epoch before exposing the reset barrier, so
                    // a full relay queue cannot strand a final `done` or
                    // `user_input_required` with no later gap to trigger a
                    // reconnect.
                    let rebase_result = inner.event_stream.rebase_tail(&endpoint_id, lease_id);
                    let stream_epoch = inner.event_stream.epoch().to_string();
                    if let Some(endpoint) = inner.endpoint.as_mut() {
                        endpoint.client_ready = false;
                        endpoint.fresh_baseline_seq = None;
                    }
                    enqueue_stream_reset(
                        sender,
                        stream_reset_message(
                            &endpoint_id,
                            lease_id,
                            &stream_epoch,
                            "live_enqueue_failed",
                        ),
                    );
                    rebase_result.map_err(|error| {
                        format!("远程控制事件入队失败后重建事件流失败：{error}")
                    })?;
                    Err(format!(
                        "远程控制实时事件入队失败，已安排事件流恢复：{send_error}"
                    ))
                }
            }
        }
    }

    fn is_current_endpoint(&self, endpoint_id: &str) -> bool {
        !endpoint_id.is_empty()
            && self
                .inner
                .lock()
                .endpoint
                .as_ref()
                .is_some_and(|endpoint| endpoint.config.endpoint_id == endpoint_id)
    }

    fn set_endpoint_error(&self, error: String) {
        {
            let mut inner = self.inner.lock();
            if let Some(endpoint) = inner.endpoint.as_mut() {
                endpoint.status = WebAccessStatusKind::Error;
                endpoint.last_error = Some(error);
            }
        }
        self.emit_status();
    }

    fn emit_status(&self) {
        let status = self.status();
        let _ = self.app.emit("web_access:status", status);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventSource {
    Rust,
    Frontend,
}

/// 从事件 payload 提取会话 id（不同事件用不同字段名）。
fn event_session_id(payload: &Value) -> Option<&str> {
    payload
        .get("session_id")
        .or_else(|| payload.get("id"))
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
}

/// 远程端正式支持代码会话之前，过滤原生代码会话事件。predicate 为 None（未注入）
/// 时不过滤，行为与注入前一致。
fn should_filter_code_session_event(
    predicate: Option<&CodeSessionPredicate>,
    payload: &Value,
) -> bool {
    match predicate {
        Some(predicate) => {
            event_session_id(payload).is_some_and(|session_id| predicate(session_id))
        }
        None => false,
    }
}

fn rpc_in_flight_expired(dispatched_at: Instant, now: Instant) -> bool {
    now.duration_since(dispatched_at) > RPC_IN_FLIGHT_TTL
}

enum RpcRequestAction {
    None,
    Respond(RelaySender, Value),
    Dispatch(RpcDispatch),
}

enum NewRpcAdmission {
    Rejected(RpcCompletion),
    Durable(RpcLedger),
}

fn rpc_admission_rejection(
    preflight_completion: Option<&RpcCompletion>,
    allowed: bool,
    in_flight_count: usize,
) -> Option<RpcCompletion> {
    if let Some(completion) = preflight_completion {
        Some(completion.clone())
    } else if !allowed {
        Some(rpc_error_completion(
            "command_not_allowed",
            "远程控制不允许调用该命令",
        ))
    } else if in_flight_count >= MAX_RPC_IN_FLIGHT {
        Some(rpc_error_completion(
            "too_many_in_flight_requests",
            "正在运行的远程控制请求过多",
        ))
    } else {
        None
    }
}

fn prepare_new_rpc_admission(
    ledger: &RpcLedger,
    request_id: &str,
    fingerprint: &str,
    preflight_completion: Option<&RpcCompletion>,
    allowed: bool,
    in_flight_count: usize,
) -> Result<NewRpcAdmission, String> {
    if let Some(completion) =
        rpc_admission_rejection(preflight_completion, allowed, in_flight_count)
    {
        // Rejections are deterministic and side-effect free. Keep them only in
        // the bounded in-memory response cache; they must never consume a
        // durable idempotency slot or evict an acknowledged request tombstone.
        return Ok(NewRpcAdmission::Rejected(completion));
    }
    let mut next_ledger = ledger.clone();
    next_ledger.remember(request_id, fingerprint)?;
    Ok(NewRpcAdmission::Durable(next_ledger))
}

enum RpcReadyAction {
    Respond(RelaySender, Value),
    Dispatch(RpcDispatch),
}

fn prepare_bridge_generation(inner: &mut Inner, generation: &str) -> Vec<RpcReadyAction> {
    let generation_changed = inner.bridge_generation.as_deref() != Some(generation);
    inner.bridge_generation = Some(generation.to_string());

    let response_target = inner
        .endpoint
        .as_ref()
        .map(|endpoint| (endpoint.config.endpoint_id.clone(), endpoint.sender.clone()));
    let request_ids = inner.rpc_order.iter().cloned().collect::<Vec<_>>();
    let mut actions = Vec::new();
    for request_id in request_ids {
        let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get(&request_id) else {
            continue;
        };

        if generation_changed && pending.acknowledged_generation.is_some() {
            let lease_id = pending.lease_id.clone();
            let fingerprint = pending.fingerprint.clone();
            let completion = outcome_unknown_completion();
            inner.rpc_cache.insert(
                request_id.clone(),
                RpcCacheEntry::Complete {
                    fingerprint,
                    completion: completion.clone(),
                },
            );
            if let Some((endpoint_id, sender)) = &response_target {
                actions.push(RpcReadyAction::Respond(
                    sender.clone(),
                    rpc_response(endpoint_id, &lease_id, &request_id, &completion),
                ));
            }
            continue;
        }

        if pending.acknowledged_generation.is_some()
            || pending.dispatched_generation.as_deref() == Some(generation)
        {
            continue;
        }
        let command = pending.command.clone();
        let args = pending.args.clone();
        let Some(RpcCacheEntry::Pending(pending)) = inner.rpc_cache.get_mut(&request_id) else {
            continue;
        };
        pending.dispatched_generation = Some(generation.to_string());
        actions.push(RpcReadyAction::Dispatch(RpcDispatch {
            request_id,
            command,
            args,
            bridge_generation: generation.to_string(),
        }));
    }
    actions
}

fn prune_rpc_cache(inner: &mut Inner) {
    while inner.rpc_cache.len() > RPC_CACHE_CAPACITY
        || rpc_cache_bytes(inner) > RPC_CACHE_BYTES_CAPACITY
    {
        let Some(position) = inner.rpc_order.iter().position(|request_id| {
            matches!(
                inner.rpc_cache.get(request_id),
                Some(RpcCacheEntry::Complete { .. })
            )
        }) else {
            // Do not evict in-flight work. The cache falls back under the cap
            // as those requests complete and later insertions prune it.
            break;
        };
        if let Some(request_id) = inner.rpc_order.remove(position) {
            inner.rpc_cache.remove(&request_id);
        }
    }
}

fn rpc_cache_bytes(inner: &Inner) -> usize {
    inner
        .rpc_cache
        .iter()
        .map(|(request_id, entry)| {
            request_id.len()
                + match entry {
                    RpcCacheEntry::Pending(pending) => {
                        pending.lease_id.len()
                            + pending.command.len()
                            + pending.fingerprint.len()
                            + serde_json::to_vec(&pending.args)
                                .map(|encoded| encoded.len())
                                .unwrap_or(MAX_RPC_REQUEST_BYTES)
                    }
                    RpcCacheEntry::Complete {
                        fingerprint,
                        completion,
                    } => {
                        fingerprint.len()
                            + serde_json::to_vec(&completion.result)
                                .map(|encoded| encoded.len())
                                .unwrap_or(MAX_RPC_RESPONSE_BYTES)
                            + completion.error.as_ref().map_or(0, String::len)
                            + completion.error_code.as_ref().map_or(0, String::len)
                    }
                }
        })
        .sum()
}

fn clear_web_attachments(inner: &mut Inner) {
    for (_, cached) in inner.web_attachments.drain() {
        remove_web_attachment_upload_dir(&cached);
    }
    inner.web_attachment_order.clear();
    inner.web_attachment_bytes = 0;
    inner.web_attachment_uploads.clear();
    inner.web_attachment_upload_order.clear();
    inner.web_session_uploads.clear();
    inner.web_session_upload_order.clear();
    for (_, download) in inner.web_session_downloads.drain() {
        let _ = std::fs::remove_file(download.path);
    }
    inner.web_session_download_order.clear();
}

fn remove_cached_web_attachment(inner: &mut Inner, handle: &str) -> bool {
    let Some(cached) = inner.web_attachments.remove(handle) else {
        return false;
    };
    inner.web_attachment_bytes = inner.web_attachment_bytes.saturating_sub(cached.bytes);
    inner
        .web_attachment_order
        .retain(|candidate| candidate != handle);
    remove_web_attachment_upload_dir(&cached);
    true
}

fn request_web_attachment_discard(inner: &mut Inner, handle: &str) {
    if let Some(cached) = inner.web_attachments.get_mut(handle) {
        if cached.reservation_id.is_some() {
            cached.discard_requested = true;
            return;
        }
    }
    remove_cached_web_attachment(inner, handle);
}

fn finish_web_attachment_reservation_inner(
    inner: &mut Inner,
    reservation_id: &str,
    handles: &[String],
    consume: bool,
) -> Result<(), String> {
    for handle in handles {
        let Some(cached) = inner.web_attachments.get(handle) else {
            // Stop/rotate deliberately clears the whole lease cache.
            continue;
        };
        if cached.reservation_id.as_deref() != Some(reservation_id) {
            return Err(format!("远程控制附件预留已不再持有句柄：{handle}"));
        }
    }
    for handle in handles {
        let should_remove = consume
            || inner
                .web_attachments
                .get(handle)
                .is_some_and(|cached| cached.discard_requested);
        if should_remove {
            remove_cached_web_attachment(inner, handle);
        } else if let Some(cached) = inner.web_attachments.get_mut(handle) {
            cached.reservation_id = None;
        }
    }
    Ok(())
}

fn append_web_attachment_upload_chunk(
    inner: &mut Inner,
    upload_id: &str,
    file_name: &str,
    offset: usize,
    total: usize,
    data: &[u8],
    commit: bool,
) -> Result<Option<(String, Vec<u8>)>, String> {
    if upload_id.len() < 8
        || upload_id.len() > 128
        || !upload_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("远程控制附件上传 ID 无效".into());
    }
    let file_name = file_name.trim();
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err("远程控制附件上传需要有效的文件名".into());
    }
    if total as u64 > crate::features::files::file_ingest::MAX_FILE_BYTES {
        return Err(format!(
            "文件超过附件 {} MB 上限",
            crate::features::files::file_ingest::MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    prune_expired_web_session_transfers(inner);
    if offset == 0 {
        inner.web_attachment_uploads.remove(upload_id);
        while inner.web_attachment_uploads.len() >= MAX_WEB_ATTACHMENT_UPLOADS {
            let Some(oldest) = inner.web_attachment_upload_order.pop_front() else {
                break;
            };
            inner.web_attachment_uploads.remove(&oldest);
        }
        inner
            .web_attachment_upload_order
            .retain(|id| id != upload_id);
        inner
            .web_attachment_upload_order
            .push_back(upload_id.to_string());
        inner.web_attachment_uploads.insert(
            upload_id.to_string(),
            WebAttachmentUpload {
                file_name: file_name.to_string(),
                total,
                data: Vec::with_capacity(total.min(4 * 1024 * 1024)),
                last_touched: Instant::now(),
            },
        );
    }
    let retained_upload_bytes: usize = inner
        .web_attachment_uploads
        .iter()
        .filter(|(id, _)| id.as_str() != upload_id)
        .map(|(_, upload)| upload.data.len())
        .sum();
    if retained_upload_bytes
        .saturating_add(offset)
        .saturating_add(data.len())
        > MAX_WEB_ATTACHMENT_UPLOAD_TOTAL_BYTES
    {
        return Err("远程控制附件上传缓存超过总容量上限".into());
    }
    let upload = inner
        .web_attachment_uploads
        .get_mut(upload_id)
        .ok_or_else(|| "远程控制附件上传不存在或已过期".to_string())?;
    if upload.file_name != file_name || upload.total != total {
        return Err("远程控制附件上传元数据已变化".into());
    }
    if upload.data.len() != offset {
        return Err(format!(
            "远程控制附件上传预期偏移量为 {}，实际为 {offset}",
            upload.data.len()
        ));
    }
    if upload.data.len().saturating_add(data.len()) > total {
        return Err("远程控制附件上传超过声明大小".into());
    }
    upload.data.extend_from_slice(data);
    upload.last_touched = Instant::now();
    if !commit {
        return Ok(None);
    }
    if upload.data.len() != total {
        return Err(format!(
            "远程控制附件上传不完整：已上传 {} / {total} 字节",
            upload.data.len()
        ));
    }
    let completed = inner
        .web_attachment_uploads
        .remove(upload_id)
        .expect("upload exists above");
    inner
        .web_attachment_upload_order
        .retain(|id| id != upload_id);
    Ok(Some((completed.file_name, completed.data)))
}

fn discard_web_attachment_upload(inner: &mut Inner, upload_id: &str) {
    inner.web_attachment_uploads.remove(upload_id);
    inner
        .web_attachment_upload_order
        .retain(|id| id != upload_id);
}

/// 浏览器本机上传的桌面暂存根目录。目录布局 `uploads/webup_<token>/<file>`
/// 与旧版远控上传一致，`verify_upload` E2E 命令可直接校验。
pub(crate) fn web_attachment_uploads_base() -> PathBuf {
    paths::pinvou3_home().join("uploads")
}

fn remove_web_attachment_upload_dir(cached: &CachedWebAttachment) {
    if let Some(dir) = &cached.upload_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 清理上一次进程遗留的上传暂存目录（崩溃或强杀时正常清理不会执行）。
/// 只清 `webup_` 前缀，避免波及 uploads 下的其他历史内容。
pub(crate) fn sweep_stale_web_attachment_uploads() {
    let Ok(entries) = std::fs::read_dir(web_attachment_uploads_base()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(WEB_ATTACHMENT_UPLOAD_DIR_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn prune_expired_web_session_transfers(inner: &mut Inner) {
    let now = Instant::now();
    inner.web_attachment_uploads.retain(|_, upload| {
        now.saturating_duration_since(upload.last_touched) <= WEB_SESSION_TRANSFER_TTL
    });
    let active_attachment_uploads = inner
        .web_attachment_uploads
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    inner
        .web_attachment_upload_order
        .retain(|id| active_attachment_uploads.contains(id));
    inner.web_session_uploads.retain(|_, upload| {
        now.saturating_duration_since(upload.last_touched) <= WEB_SESSION_TRANSFER_TTL
    });
    let active_uploads = inner
        .web_session_uploads
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    inner
        .web_session_upload_order
        .retain(|id| active_uploads.contains(id));
    let expired_downloads = inner
        .web_session_downloads
        .iter()
        .filter(|(_, download)| {
            now.saturating_duration_since(download.last_touched) > WEB_SESSION_TRANSFER_TTL
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in expired_downloads {
        if let Some(download) = inner.web_session_downloads.remove(&id) {
            let _ = std::fs::remove_file(download.path);
        }
    }
    let active_downloads = inner
        .web_session_downloads
        .keys()
        .cloned()
        .collect::<HashSet<_>>();
    inner
        .web_session_download_order
        .retain(|id| active_downloads.contains(id));
}

fn ensure_web_session_download_capacity(
    inner: &Inner,
    reserved_bytes: usize,
) -> Result<(), String> {
    if inner.web_session_downloads.len() >= MAX_WEB_SESSION_DOWNLOADS {
        return Err(format!(
            "远程控制已有 {} 个进行中的会话下载，请等待完成或重试已有下载",
            MAX_WEB_SESSION_DOWNLOADS
        ));
    }
    let active_bytes = inner
        .web_session_downloads
        .values()
        .map(|download| download.reserved_bytes)
        .sum::<usize>();
    if active_bytes.saturating_add(reserved_bytes) > MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES {
        return Err(format!(
            "进行中的远程控制会话下载将超过 {} MiB 总上限",
            MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES / (1024 * 1024)
        ));
    }
    Ok(())
}

fn rpc_response(
    endpoint_id: &str,
    lease_id: &str,
    request_id: &str,
    completion: &RpcCompletion,
) -> Value {
    json!({
        "v": PROTOCOL_VERSION,
        "type": "rpc_response",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "id": request_id,
        "client_request_id": request_id,
        "ok": completion.ok,
        "result": completion.result,
        "error": completion.error,
        "error_code": completion.error_code,
    })
}

fn rpc_error_completion(code: impl Into<String>, error: impl Into<String>) -> RpcCompletion {
    RpcCompletion {
        ok: false,
        result: Value::Null,
        error: Some(error.into().chars().take(16_384).collect()),
        error_code: Some(code.into()),
    }
}

fn bounded_rpc_completion(ok: bool, result: Option<Value>, error: Option<String>) -> RpcCompletion {
    let result = result.unwrap_or(Value::Null);
    let response_bytes = serde_json::to_vec(&result)
        .map(|encoded| encoded.len())
        .unwrap_or(usize::MAX);
    if response_bytes > MAX_RPC_RESPONSE_BYTES {
        return rpc_error_completion(
            "response_too_large",
            format!(
                "远程控制响应超过 {} MiB 上限，请使用分页或分块命令",
                MAX_RPC_RESPONSE_BYTES / (1024 * 1024)
            ),
        );
    }
    RpcCompletion {
        ok,
        result,
        error: error.map(|message| message.chars().take(16_384).collect()),
        error_code: None,
    }
}

fn outcome_unknown_completion() -> RpcCompletion {
    rpc_error_completion(
        "outcome_unknown",
        "the desktop may have started this request before its result was durably observed; it will not be run again",
    )
}

fn request_conflict_completion() -> RpcCompletion {
    rpc_error_completion(
        "request_id_conflict",
        "client_request_id was reused with different command or args",
    )
}

fn tombstone_completion(tombstone: &RpcTombstone, fingerprint: &str) -> RpcCompletion {
    if tombstone.fingerprint != fingerprint {
        request_conflict_completion()
    } else if tombstone.acknowledged {
        outcome_unknown_completion()
    } else {
        rpc_error_completion(
            "request_not_started",
            "the request was durably accepted but not acknowledged for execution",
        )
    }
}

fn rpc_fingerprint(command: &str, args: &Value) -> Result<String, String> {
    let canonical = canonicalize_json(args);
    let encoded = serde_json::to_vec(&(command, canonical))
        .map_err(|error| format!("serialize Web RPC fingerprint: {error}"))?;
    let digest = Sha256::digest(encoded);
    Ok(crate::platform::encoding::hex_lower(&digest))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(left, _)| *left);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                    .collect(),
            )
        }
        _ => value.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSessionScope {
    Required(&'static str),
    Optional(&'static str),
}

fn web_session_scope(command: &str) -> Option<WebSessionScope> {
    use WebSessionScope::{Optional, Required};
    let scope = match command {
        // Commands whose Rust API historically falls back to the desktop
        // process-wide active Session must be explicit over WebUI.
        "add_run_materials"
        | "approve_workflow_gate"
        | "archive_recent_work_memory"
        | "cancel_generation"
        | "cancel_user_input"
        | "compact_now"
        | "confirm_pending_memory"
        | "delete_memory_preference"
        | "delete_timed_memory"
        | "delete_work_context_memory"
        | "edit_last_turn"
        | "get_memory_overview"
        | "ignore_pending_memory"
        | "kick_workflow"
        | "never_pending_memory"
        | "reject_workflow_gate"
        | "retry_workflow_role"
        | "stop_workflow"
        | "submit_user_input"
        | "summon_pinvou"
        | "update_memory_profile"
        | "update_memory_preference"
        | "update_timed_memory"
        | "update_work_context_memory" => Required("sessionId"),

        "accept_plan"
        | "cancel_shell_task"
        | "discard_plan"
        | "equip_persona"
        | "exit_plan_to_yolo"
        | "get_active_persona"
        | "get_mode_state"
        | "get_session_model_id"
        | "get_session_persona_events"
        | "get_session_pinvou_reviews"
        | "get_session_pinvou_scene_events"
        | "get_session_timeline"
        | "get_workflow_state"
        | "list_shell_tasks"
        | "list_workspace_files"
        | "save_session_persona_events"
        | "save_session_pinvou_reviews"
        | "save_session_pinvou_scene_events"
        | "session_mount_collection"
        | "session_add_mounted_collection"
        | "session_mounted_collection"
        | "session_mounted_collections"
        | "session_mounted_collections_snapshot"
        | "session_remove_mounted_collection"
        | "session_set_mounted_collection_enabled"
        | "session_set_mounted_collections"
        | "session_unmount_collection"
        | "set_plan_mode_next"
        | "set_session_model"
        | "unbind_session_skill"
        | "unequip_persona"
        | "web_access_artifact_info"
        | "web_access_chat"
        | "web_access_get_gate_report"
        | "web_access_get_role_logs"
        | "web_access_get_role_outputs"
        | "web_access_get_role_prompt"
        | "web_access_list_deliverables"
        | "web_access_read_artifact_chunk"
        | "web_access_read_artifact_image_b64"
        | "web_access_read_artifact_text"
        | "web_access_read_artifact_thumbnail"
        | "web_access_render_artifact_visual"
        | "web_access_read_conversation_attachment_chunk"
        | "web_access_transcribe_voice_audio"
        | "web_access_write_artifact_text" => Required("sessionId"),

        "delete_session"
        | "rename_session"
        | "save_session_artifacts"
        | "set_session_archived"
        | "set_session_pinned"
        | "web_access_load_session_chunk"
        | "web_access_save_session_messages_chunk" => Required("id"),

        // Omitting this one deliberately means "create a fresh workflow
        // Session"; it never consults the desktop active pointer.
        "start_workflow" => Optional("sessionId"),
        _ => return None,
    };
    Some(scope)
}

fn validate_web_rpc_scope(app: &AppHandle, command: &str, args: &Value) -> Result<(), String> {
    let Some(scope) = web_session_scope(command) else {
        return Ok(());
    };
    let (field, required) = match scope {
        WebSessionScope::Required(field) => (field, true),
        WebSessionScope::Optional(field) => (field, false),
    };
    let session_id = args
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(session_id) = session_id else {
        return if required {
            Err(format!("{command} requires an explicit {field}"))
        } else {
            Ok(())
        };
    };
    crate::features::sessions::validate_session_id(session_id)
        .map_err(|error| format!("远程控制会话 ID 无效：{error:#}"))?;
    if (command == "web_access_load_session_chunk"
        && args
            .get("downloadId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty()))
        || (command == "web_access_save_session_messages_chunk"
            && args.get("offset").and_then(Value::as_u64).unwrap_or(0) > 0)
    {
        // The opaque transfer token is already bound to the validated
        // Session id in RemoteControlManager; avoid re-reading a large Session
        // file for every 256 KiB chunk.
        return Ok(());
    }
    let store = app
        .try_state::<SessionStore>()
        .ok_or_else(|| "Session store is not ready".to_string())?;
    store
        .load(session_id)
        .map_err(|error| format!("远程控制会话 {session_id} 不存在：{error:#}"))?;
    Ok(())
}

fn validate_bridge_generation(generation: &str) -> Result<(), String> {
    let generation = generation.trim();
    if generation.len() < 8 || generation.len() > 256 {
        return Err("invalid WebView bridge generation".to_string());
    }
    if !generation
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("invalid WebView bridge generation".to_string());
    }
    Ok(())
}

fn validate_rpc_command(command: &str) -> Result<(), String> {
    if command.is_empty() || command.len() > MAX_RPC_COMMAND_BYTES {
        return Err("远程控制命令无效".to_string());
    }
    if !command
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-'))
    {
        return Err("远程控制命令无效".to_string());
    }
    Ok(())
}

fn enqueue_stream_reset(sender: RelaySender, message: Value) {
    // RelaySender owns a single bounded waiter and coalesces repeated recovery
    // barriers to the latest lease/epoch while the data channel is saturated.
    if sender.enqueue_stream_reset(message).is_err() {
        eprintln!("[web-access] stream reset could not reach the relay task");
    }
}

fn pairing_info(endpoint: &ActiveEndpoint) -> WebAccessInfo {
    WebAccessInfo {
        endpoint_id: endpoint.config.endpoint_id.clone(),
        url: endpoint.url.clone(),
        qr_data_url: crate::features::connectors::connector_cli::make_qr(&endpoint.url),
        status: endpoint.status,
    }
}

fn fresh_config() -> WebAccessConfig {
    WebAccessConfig {
        relay_url: remote_relay_ws_url(),
        endpoint_id: format!("ep_{}", crate::features::remote_control::short_token(24)),
        access_token: crate::features::remote_control::short_token(48),
        desktop_secret: crate::features::remote_control::short_token(48),
    }
}

fn validate_config(config: &WebAccessConfig) -> Result<(), String> {
    if config.relay_url.len() > 2_048
        || !(config.relay_url.starts_with("ws://") || config.relay_url.starts_with("wss://"))
        || config.relay_url.chars().any(char::is_whitespace)
    {
        return Err("Web access relay_url is invalid".to_string());
    }
    if config.endpoint_id.len() < 8
        || config.endpoint_id.len() > 128
        || !config
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("Web access endpoint_id is invalid".to_string());
    }
    if !(24..=1_024).contains(&config.access_token.len())
        || !(24..=1_024).contains(&config.desktop_secret.len())
    {
        return Err("Web access credentials are invalid".to_string());
    }
    Ok(())
}

fn config_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.json")
}

fn process_lock_path() -> PathBuf {
    paths::pinvou3_home().join("web-access.lock")
}

fn pending_revocations_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-pending-revocations.json")
}

fn acquire_process_lock(path: &Path) -> Result<File, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid Web access lock path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    platform::configure_private_open_options(&mut options);
    let file = options
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    platform::enforce_private_permissions(&file, path)
        .map_err(|error| format!("set private permissions on {}: {error}", path.display()))?;
    file.try_lock_exclusive().map_err(|error| {
        format!(
            "Web access is already owned by another desktop process ({}): {error}",
            path.display()
        )
    })?;
    Ok(file)
}

fn rpc_ledger_path() -> PathBuf {
    paths::pinvou3_home().join("web-access-rpc-ledger.json")
}

fn load_config() -> Result<Option<WebAccessConfig>, String> {
    load_config_from(&config_path())
}

fn load_config_from(path: &Path) -> Result<Option<WebAccessConfig>, String> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut text = String::new();
    file.take((MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if text.len() > MAX_WEB_ACCESS_CONFIG_BYTES {
        return Err(format!(
            "Web access config is larger than {} KiB: {}",
            MAX_WEB_ACCESS_CONFIG_BYTES / 1024,
            path.display()
        ));
    }
    let config: WebAccessConfig = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_config(&config)?;
    Ok(Some(config))
}

fn persist_config(config: &WebAccessConfig) -> Result<(), String> {
    validate_config(config)?;
    atomic_write_private_json(&config_path(), config, "Web access config")
}

fn remove_config() -> Result<(), String> {
    remove_private_file(&config_path())
}

// ── 用户自定义 Relay 地址 ─────────────────────────────────────────────
// 设置页填写的域名/IP 规范化为「WebSocket 地址 + 页面基址」一对后持久化；
// 生效优先级：运行时 env 覆盖 > PINVOU_REMOTE_* env > 本设置 > 内置默认。

const RELAY_DEFAULT_BASE_PATH: &str = "/pinvou3/remote";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelaySettings {
    pub relay_url: String,
    pub public_base_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelaySettingsInfo {
    pub relay_url: String,
    pub public_base_url: String,
    pub custom: bool,
    pub default_relay_url: String,
    pub default_public_base_url: String,
}

fn relay_settings_path() -> PathBuf {
    paths::pinvou3_home().join("web-relay.json")
}

fn validate_relay_settings(settings: &RelaySettings) -> Result<(), String> {
    if settings.relay_url.len() > 2_048
        || !(settings.relay_url.starts_with("ws://") || settings.relay_url.starts_with("wss://"))
        || settings.relay_url.chars().any(char::is_whitespace)
    {
        return Err("relay address does not normalize to a valid ws(s) URL".to_string());
    }
    if settings.public_base_url.len() > 2_048
        || !(settings.public_base_url.starts_with("http://")
            || settings.public_base_url.starts_with("https://"))
        || settings.public_base_url.chars().any(char::is_whitespace)
    {
        return Err("relay address does not normalize to a valid http(s) base".to_string());
    }
    Ok(())
}

/// 把用户填写的 Relay 地址规范化。接受裸域名/IP（默认按 TLS 处理成 wss/https）、
/// `ws(s)://`、`http(s)://` 前缀以及可选自定义路径；无 TLS 证书的环境需要显式
/// 写 `ws://` 或 `http://` 前缀，不做静默降级。
fn normalize_relay_address(input: &str) -> Result<RelaySettings, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("relay address is empty".to_string());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("relay address must not contain whitespace".to_string());
    }
    if trimmed.len() > 1_024 {
        return Err("relay address is too long".to_string());
    }
    let (ws_scheme, http_scheme, rest) = if let Some(rest) = trimmed.strip_prefix("wss://") {
        ("wss", "https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("ws://") {
        ("ws", "http", rest)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        ("wss", "https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("ws", "http", rest)
    } else if trimmed.contains("://") {
        return Err("relay address only supports ws/wss/http/https".to_string());
    } else {
        ("wss", "https", trimmed)
    };
    if rest.contains('?') || rest.contains('#') || rest.contains('@') {
        return Err("relay address must not contain userinfo, query or fragment".to_string());
    }
    let (host, path) = match rest.find('/') {
        Some(index) => (&rest[..index], rest[index..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if host.is_empty() {
        return Err("relay address is missing a host".to_string());
    }
    // 路径规则：不填 → 内置 /pinvou3/remote；填了 → 尊重自定义（容忍粘贴完整
    // ws 地址时结尾多出的 /ws，避免「复制现有 relay_url 再保存」变成 /ws/ws）。
    let base_path = if path.is_empty() {
        RELAY_DEFAULT_BASE_PATH.to_string()
    } else {
        path.strip_suffix("/ws").unwrap_or(path).to_string()
    };
    let settings = RelaySettings {
        relay_url: format!("{ws_scheme}://{host}{base_path}/ws"),
        public_base_url: format!("{http_scheme}://{host}{base_path}"),
    };
    validate_relay_settings(&settings)?;
    Ok(settings)
}

/// 读取失败（不存在/损坏/非法）一律回落默认 relay：设置文件不是凭据，宁可
/// 降级也不把「启用 Web 访问」整个卡死；重新保存会原子覆盖坏文件。
fn load_relay_settings() -> Option<RelaySettings> {
    let file = File::open(relay_settings_path()).ok()?;
    let mut text = String::new();
    file.take((MAX_WEB_ACCESS_CONFIG_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > MAX_WEB_ACCESS_CONFIG_BYTES {
        return None;
    }
    let settings: RelaySettings = serde_json::from_str(&text).ok()?;
    validate_relay_settings(&settings).ok()?;
    Some(settings)
}

fn pending_revocation_key(pending: &PendingRevocation) -> String {
    format!("{}\n{}", pending.relay_url, pending.endpoint_id)
}

fn validate_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    if pending.relay_url.len() > 2_048
        || !(pending.relay_url.starts_with("ws://") || pending.relay_url.starts_with("wss://"))
        || pending.relay_url.chars().any(char::is_whitespace)
    {
        return Err("pending Web revocation has an invalid relay_url".to_string());
    }
    if pending.endpoint_id.len() < 4
        || pending.endpoint_id.len() > 128
        || !pending
            .endpoint_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("pending Web revocation has an invalid endpoint_id".to_string());
    }
    if pending.desktop_secret.len() < 24 || pending.desktop_secret.len() > 1_024 {
        return Err("pending Web revocation has an invalid desktop secret".to_string());
    }
    Ok(())
}

fn validate_pending_revocation_ledger(ledger: &PendingRevocationLedger) -> Result<(), String> {
    if ledger.version != PENDING_REVOCATIONS_VERSION {
        return Err(format!(
            "unsupported pending Web revocation version {}",
            ledger.version
        ));
    }
    if ledger.entries.len() > MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation count exceeds {MAX_PENDING_REVOCATIONS}"
        ));
    }
    let mut keys = HashSet::new();
    for pending in &ledger.entries {
        validate_pending_revocation(pending)?;
        if !keys.insert(pending_revocation_key(pending)) {
            return Err(format!(
                "duplicate pending Web revocation for {}",
                pending.endpoint_id
            ));
        }
    }
    Ok(())
}

fn load_pending_revocations() -> Result<PendingRevocationLedger, String> {
    load_pending_revocations_from(&pending_revocations_path())
}

fn load_pending_revocations_from(path: &Path) -> Result<PendingRevocationLedger, String> {
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PendingRevocationLedger::default());
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    if data.len() > MAX_PENDING_REVOCATIONS_FILE_BYTES {
        return Err(format!(
            "pending Web revocation file is larger than {} KiB: {}",
            MAX_PENDING_REVOCATIONS_FILE_BYTES / 1024,
            path.display()
        ));
    }
    let ledger: PendingRevocationLedger = serde_json::from_slice(&data)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    validate_pending_revocation_ledger(&ledger)?;
    Ok(ledger)
}

fn persist_pending_revocations_to(
    path: &Path,
    ledger: &PendingRevocationLedger,
) -> Result<(), String> {
    validate_pending_revocation_ledger(ledger)?;
    if ledger.entries.is_empty() {
        remove_private_file(path)
    } else {
        atomic_write_private_json(path, ledger, "pending Web revocations")
    }
}

fn queue_pending_revocation(config: &WebAccessConfig) -> Result<(), String> {
    queue_pending_revocation_at(&pending_revocations_path(), PendingRevocation::from(config))
}

fn queue_pending_revocation_at(path: &Path, pending: PendingRevocation) -> Result<(), String> {
    validate_pending_revocation(&pending)?;
    let mut ledger = load_pending_revocations_from(path)?;
    let key = pending_revocation_key(&pending);
    if let Some(existing) = ledger
        .entries
        .iter()
        .find(|entry| pending_revocation_key(entry) == key)
    {
        return if existing == &pending {
            Ok(())
        } else {
            Err(format!(
                "conflicting pending Web revocation credentials for {}",
                pending.endpoint_id
            ))
        };
    }
    if ledger.entries.len() >= MAX_PENDING_REVOCATIONS {
        return Err(format!(
            "pending Web revocation queue is full ({MAX_PENDING_REVOCATIONS})"
        ));
    }
    ledger.entries.push_back(pending);
    persist_pending_revocations_to(path, &ledger)
}

fn acknowledge_pending_revocation(pending: &PendingRevocation) -> Result<(), String> {
    acknowledge_pending_revocation_at(&pending_revocations_path(), pending)
}

fn acknowledge_pending_revocation_at(
    path: &Path,
    pending: &PendingRevocation,
) -> Result<(), String> {
    let mut ledger = load_pending_revocations_from(path)?;
    ledger.entries.retain(|entry| entry != pending);
    persist_pending_revocations_to(path, &ledger)
}

fn eligible_pending_revocations(
    ledger: &PendingRevocationLedger,
    current: Option<&WebAccessConfig>,
) -> Vec<PendingRevocation> {
    ledger
        .entries
        .iter()
        .filter(|pending| {
            current.is_none_or(|config| {
                pending.relay_url != config.relay_url
                    || pending.endpoint_id != config.endpoint_id
                    || pending.desktop_secret != config.desktop_secret
            })
        })
        .cloned()
        .collect()
}

fn pending_revocation_ack(value: &Value, endpoint_id: &str) -> bool {
    let message_endpoint = value.get("endpoint_id").and_then(Value::as_str);
    match value.get("type").and_then(Value::as_str) {
        Some("desktop_endpoint_revoked") | Some("desktop_endpoint_replaced") => {
            message_endpoint == Some(endpoint_id)
        }
        Some("error")
            if value.get("code").and_then(Value::as_str) == Some("endpoint_not_found") =>
        {
            message_endpoint.is_none_or(|value| value == endpoint_id)
        }
        _ => false,
    }
}

fn load_or_initialize_rpc_ledger(endpoint_id: &str) -> Result<RpcLedger, String> {
    let path = rpc_ledger_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let ledger = RpcLedger::for_endpoint(endpoint_id);
            persist_rpc_ledger(&ledger)?;
            return Ok(ledger);
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut ledger: RpcLedger = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    if ledger.endpoint_id != endpoint_id {
        let ledger = RpcLedger::for_endpoint(endpoint_id);
        persist_rpc_ledger(&ledger)?;
        return Ok(ledger);
    }
    if ledger.version != RPC_LEDGER_VERSION {
        return Err(format!(
            "unsupported Web RPC ledger version {} in {}",
            ledger.version,
            path.display()
        ));
    }
    let mut request_ids = HashSet::new();
    for entry in &ledger.entries {
        if entry.request_id.is_empty()
            || entry.request_id.len() > 256
            || entry.fingerprint.len() != 64
            || !entry
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!(
                "invalid Web RPC ledger entry in {}",
                path.display()
            ));
        }
        if !request_ids.insert(entry.request_id.as_str()) {
            return Err(format!(
                "duplicate Web RPC request id in {}: {}",
                path.display(),
                entry.request_id
            ));
        }
    }
    if ledger.entries.len() > RPC_CACHE_CAPACITY {
        let overflow = ledger.entries.len() - RPC_CACHE_CAPACITY;
        ledger.entries.drain(..overflow);
        persist_rpc_ledger(&ledger)?;
    }
    Ok(ledger)
}

fn persist_rpc_ledger(ledger: &RpcLedger) -> Result<(), String> {
    if ledger.version != RPC_LEDGER_VERSION || ledger.endpoint_id.is_empty() {
        return Err("refusing to persist an invalid Web RPC ledger".to_string());
    }
    atomic_write_private_json(&rpc_ledger_path(), ledger, "Web RPC ledger")
}

fn remove_rpc_ledger() -> Result<(), String> {
    remove_private_file(&rpc_ledger_path())
}

fn atomic_write_private_json<T: Serialize>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid {label} path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let data =
        serde_json::to_vec_pretty(value).map_err(|error| format!("serialize {label}: {error}"))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("web-access");
    let mut temporary = None;
    for _ in 0..16 {
        let candidate = parent.join(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        platform::configure_private_open_options(&mut options);
        match options.open(&candidate) {
            Ok(file) => {
                platform::enforce_private_permissions(&file, &candidate).map_err(|error| {
                    format!(
                        "set private permissions on {}: {error}",
                        candidate.display()
                    )
                })?;
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create {}: {error}", candidate.display())),
        }
    }
    let (temporary, mut file) =
        temporary.ok_or_else(|| format!("allocate a temporary file next to {}", path.display()))?;
    let result = (|| {
        file.write_all(&data)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        drop(file);
        platform::atomic_replace(&temporary, path)
            .map_err(|error| format!("commit {}: {error}", path.display()))?;
        platform::sync_parent_directory(parent)
            .map_err(|error| format!("sync {}: {error}", parent.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn remove_private_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                platform::sync_parent_directory(parent)
                    .map_err(|error| format!("sync {}: {error}", parent.display()))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

fn remote_public_base_url() -> String {
    if let Ok(url) = std::env::var("PINVOU_REMOTE_PUBLIC_URL") {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.public_base_url;
    }
    DEFAULT_PUBLIC_BASE_URL.to_string()
}

fn remote_relay_ws_url() -> String {
    if let Ok(url) = std::env::var("PINVOU_REMOTE_RELAY_WS_URL") {
        return url;
    }
    if let Some(settings) = load_relay_settings() {
        return settings.relay_url;
    }
    DEFAULT_RELAY_WS_URL.to_string()
}

/// Development/manual-test override for the active connection only. Unlike
/// `PINVOU_REMOTE_RELAY_WS_URL`, this value is never written to the persistent
/// Web access config, so testing a local relay cannot strand the normal link.
fn apply_runtime_relay_override(mut config: WebAccessConfig) -> WebAccessConfig {
    if let Ok(relay_url) = std::env::var("PINVOU_REMOTE_RUNTIME_RELAY_WS_URL") {
        if !relay_url.trim().is_empty() {
            config.relay_url = relay_url;
        }
    }
    config
}

fn public_url(config: &WebAccessConfig) -> String {
    let mut url = format!(
        "{}/#endpoint={}&token={}",
        remote_public_base_url().trim_end_matches('/'),
        percent_encode(&config.endpoint_id),
        percent_encode(&config.access_token),
    );
    if config.relay_url != DEFAULT_RELAY_WS_URL {
        url.push_str("&relay=");
        url.push_str(&percent_encode(&config.relay_url));
    }
    url
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_in_flight_rpc_expires_after_ttl_only() {
        let dispatched_at = Instant::now();
        assert!(!rpc_in_flight_expired(
            dispatched_at,
            dispatched_at + Duration::from_secs(180)
        ));
        assert!(rpc_in_flight_expired(
            dispatched_at,
            dispatched_at + RPC_IN_FLIGHT_TTL + Duration::from_secs(1)
        ));
    }

    #[test]
    fn packaged_defaults_target_the_production_remote_endpoint() {
        assert_eq!(
            DEFAULT_PUBLIC_BASE_URL,
            "http://127.0.0.1:8787/pinvou3/remote"
        );
        assert_eq!(
            DEFAULT_RELAY_WS_URL,
            "ws://127.0.0.1:8787/pinvou3/remote/ws"
        );
    }

    #[test]
    fn web_attachment_upload_accumulates_chunks_until_commit() {
        let mut inner = Inner::default();

        let first = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "报告.pdf",
            0,
            6,
            b"abc",
            false,
        )
        .expect("first chunk");
        assert!(first.is_none());

        let completed = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "报告.pdf",
            3,
            6,
            b"def",
            true,
        )
        .expect("final chunk")
        .expect("commit returns the payload");
        assert_eq!(completed.0, "报告.pdf");
        assert_eq!(completed.1, b"abcdef");
        assert!(inner.web_attachment_uploads.is_empty());
        assert!(inner.web_attachment_upload_order.is_empty());
    }

    #[test]
    fn web_attachment_upload_rejects_totals_over_the_ingest_limit() {
        let mut inner = Inner::default();
        let oversized = crate::features::files::file_ingest::MAX_FILE_BYTES as usize + 1;

        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "big.bin",
            0,
            oversized,
            b"x",
            false,
        )
        .expect_err("uploads above MAX_FILE_BYTES must fail before transfer");
        assert!(error.contains("上限"), "unexpected error: {error}");
        assert!(inner.web_attachment_uploads.is_empty());
    }

    #[test]
    fn web_attachment_upload_requires_contiguous_offsets() {
        let mut inner = Inner::default();
        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            0,
            8,
            b"abcd",
            false,
        )
        .expect("first chunk");

        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            2,
            8,
            b"cdef",
            false,
        )
        .expect_err("out-of-order chunks must be rejected");
        assert!(error.contains("偏移量"), "unexpected error: {error}");
    }

    #[test]
    fn web_attachment_upload_abort_discards_the_buffer() {
        let mut inner = Inner::default();
        append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            0,
            8,
            b"abcd",
            false,
        )
        .expect("first chunk");

        discard_web_attachment_upload(&mut inner, "upload_device_1");
        assert!(inner.web_attachment_uploads.is_empty());
        assert!(inner.web_attachment_upload_order.is_empty());

        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_1",
            "notes.txt",
            4,
            8,
            b"efgh",
            true,
        )
        .expect_err("continuing an aborted upload must fail");
        assert!(
            error.contains("不存在或已过期"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn web_attachment_upload_capacity_evicts_the_oldest_buffer() {
        let mut inner = Inner::default();
        for index in 0..=MAX_WEB_ATTACHMENT_UPLOADS {
            append_web_attachment_upload_chunk(
                &mut inner,
                &format!("upload_device_{index}"),
                "notes.txt",
                0,
                8,
                b"abcd",
                false,
            )
            .expect("start upload");
        }

        assert_eq!(
            inner.web_attachment_uploads.len(),
            MAX_WEB_ATTACHMENT_UPLOADS
        );
        let error = append_web_attachment_upload_chunk(
            &mut inner,
            "upload_device_0",
            "notes.txt",
            4,
            8,
            b"efgh",
            true,
        )
        .expect_err("the evicted oldest upload must no longer continue");
        assert!(
            error.contains("不存在或已过期"),
            "unexpected error: {error}"
        );
    }

    fn cached_upload_for_test(
        staging: &std::path::Path,
        reservation_id: Option<&str>,
    ) -> CachedWebAttachment {
        CachedWebAttachment {
            result: crate::features::files::file_ingest::IngestResult {
                kind: "text".into(),
                basename: "notes.txt".into(),
                path: staging.join("notes.txt").to_string_lossy().into_owned(),
                markdown: Some("abc".into()),
                token_estimate: 2,
                byte_size: 3,
                warning: None,
            },
            bytes: 64,
            reservation_id: reservation_id.map(str::to_string),
            discard_requested: false,
            upload_dir: Some(staging.to_path_buf()),
        }
    }

    #[test]
    fn clearing_web_attachments_removes_upload_staging_dirs() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webup-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert(
            "attachment_a".into(),
            cached_upload_for_test(&staging, None),
        );
        inner.web_attachment_bytes = 64;

        clear_web_attachments(&mut inner);
        assert!(inner.web_attachments.is_empty());
        assert!(
            !staging.exists(),
            "clearing attachments must delete the upload staging dir"
        );
    }

    #[test]
    fn discarding_unreserved_web_attachment_removes_cache_and_staging_dir() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webup-discard-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert(
            "attachment_a".into(),
            cached_upload_for_test(&staging, None),
        );
        inner.web_attachment_bytes = 64;

        request_web_attachment_discard(&mut inner, "attachment_a");

        assert!(inner.web_attachments.is_empty());
        assert!(inner.web_attachment_order.is_empty());
        assert_eq!(inner.web_attachment_bytes, 0);
        assert!(
            !staging.exists(),
            "discarding a ready attachment must delete its staging dir"
        );
    }

    #[test]
    fn discarding_reserved_web_attachment_waits_for_turn_cleanup() {
        let staging = std::env::temp_dir().join(format!(
            "pinvou-webup-deferred-discard-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("notes.txt"), b"abc").unwrap();

        let mut inner = Inner::default();
        inner.web_attachment_order.push_back("attachment_a".into());
        inner.web_attachments.insert(
            "attachment_a".into(),
            cached_upload_for_test(&staging, Some("reservation_a")),
        );
        inner.web_attachment_bytes = 64;

        request_web_attachment_discard(&mut inner, "attachment_a");
        assert!(
            staging.exists(),
            "an active turn still needs the source file"
        );
        assert!(inner
            .web_attachments
            .get("attachment_a")
            .is_some_and(|cached| cached.discard_requested));

        finish_web_attachment_reservation_inner(
            &mut inner,
            "reservation_a",
            &["attachment_a".into()],
            false,
        )
        .expect("a rejected turn still honors the deferred discard");

        assert!(inner.web_attachments.is_empty());
        assert!(inner.web_attachment_order.is_empty());
        assert_eq!(inner.web_attachment_bytes, 0);
        assert!(
            !staging.exists(),
            "turn cleanup must delete a browser-discarded attachment"
        );
    }

    #[test]
    fn normalize_relay_address_accepts_bare_domain_as_tls() {
        let settings = normalize_relay_address("relay.example.com").expect("bare domain");
        assert_eq!(
            settings.relay_url,
            "wss://relay.example.com/pinvou3/remote/ws"
        );
        assert_eq!(
            settings.public_base_url,
            "https://relay.example.com/pinvou3/remote"
        );
    }

    #[test]
    fn normalize_relay_address_accepts_ip_with_port_and_explicit_ws() {
        let settings = normalize_relay_address("ws://192.168.1.20:8080").expect("plain ip");
        assert_eq!(
            settings.relay_url,
            "ws://192.168.1.20:8080/pinvou3/remote/ws"
        );
        assert_eq!(
            settings.public_base_url,
            "http://192.168.1.20:8080/pinvou3/remote"
        );
    }

    #[test]
    fn normalize_relay_address_bare_ip_defaults_to_tls_without_downgrade() {
        let settings = normalize_relay_address("203.0.113.7").expect("bare ip");
        assert_eq!(settings.relay_url, "wss://203.0.113.7/pinvou3/remote/ws");
    }

    #[test]
    fn normalize_relay_address_roundtrips_pasted_full_ws_url() {
        let settings = normalize_relay_address("wss://relay.example.com/pinvou3/remote/ws")
            .expect("full ws url");
        assert_eq!(
            settings.relay_url,
            "wss://relay.example.com/pinvou3/remote/ws"
        );
        assert_eq!(
            settings.public_base_url,
            "https://relay.example.com/pinvou3/remote"
        );
    }

    #[test]
    fn normalize_relay_address_respects_custom_path_and_trailing_slash() {
        let settings = normalize_relay_address("https://cdn.example.com/proxy/").expect("custom");
        assert_eq!(settings.relay_url, "wss://cdn.example.com/proxy/ws");
        assert_eq!(settings.public_base_url, "https://cdn.example.com/proxy");
    }

    #[test]
    fn normalize_relay_address_rejects_garbage() {
        for input in [
            "",
            "   ",
            "ftp://x.example.com",
            "wss://",
            "https://user:pw@host/x",
            "host/path?query=1",
            "host/#frag",
            "has space.example.com",
        ] {
            assert!(
                normalize_relay_address(input).is_err(),
                "should reject {input:?}"
            );
        }
    }

    #[test]
    fn embedded_policy_is_v2_and_blocks_native_authority() {
        let policy = AccessPolicy::load().expect("embedded policy");
        for command in [
            "chat",
            "ingest_file",
            "save_session_messages",
            "delete_model",
            "save_model",
            "set_active_model",
            "test_model_connection",
            "set_disabled_connectors",
            "install_marketplace_skill",
            "install_marketplace_tool",
            "uninstall_marketplace_skill",
            "uninstall_marketplace_tool",
            "install_voice_asr",
            "save_settings_and_restart",
            "artifact_info",
            "read_artifact_text",
            "write_artifact_text",
            "read_artifact_image_b64",
            "read_artifact_thumbnail",
            "render_artifact_visual",
            "list_deliverables",
            "get_role_prompt",
            "get_role_outputs",
            "get_role_logs",
            "get_gate_report",
            "update_settings",
            "create_session",
            "load_session",
            "start_skill_session",
            "web_access_bridge_ready",
            "web_access_rpc_begin",
            "web_access_rpc_respond",
        ] {
            assert!(
                !policy.commands.contains(command),
                "{command} must stay native-only"
            );
        }
        for command in [
            "web_access_artifact_info",
            "web_access_read_artifact_text",
            "web_access_write_artifact_text",
            "web_access_read_artifact_image_b64",
            "web_access_read_artifact_thumbnail",
            "web_access_render_artifact_visual",
            "web_access_list_deliverables",
            "web_access_get_role_prompt",
            "web_access_get_role_outputs",
            "web_access_get_role_logs",
            "web_access_get_gate_report",
            "web_access_update_settings",
            "web_access_create_session",
            "web_access_create_session_and_chat",
            "web_access_chat",
            "web_access_ingest_file",
            "web_access_upload_attachment_chunk",
            "web_access_abort_attachment_upload",
            "web_access_discard_attachment",
            "web_access_read_conversation_attachment_chunk",
            "web_access_load_session_chunk",
            "web_access_start_skill_session",
        ] {
            assert!(
                policy.commands.contains(command),
                "{command} must be Web-scoped"
            );
        }
        assert!(
            !policy
                .commands
                .contains("web_access_save_session_messages_chunk"),
            "frontend transcript writes must stay blocked"
        );
        assert!(policy.events.contains("chat:delta"));
        assert!(policy.events.contains("chat:reasoning_start"));
        assert!(policy.events.contains("chat:reasoning_delta"));
        assert!(policy.events.contains("chat:reasoning_done"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:reasoning_start"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:reasoning_delta"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:reasoning_done"));
        assert!(policy.events.contains("chat:user_message"));
        assert!(policy.events.contains("chat:plan_resolved"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"chat:plan_resolved"));
        assert!(policy.events.contains("chat:transcript_committed"));
        assert!(policy.events.contains("session:deleted"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:deleted"));
        assert!(policy.events.contains("session:list_changed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:list_changed"));
        assert!(policy.events.contains("session:model_changed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:model_changed"));
        assert!(policy.events.contains("session:persona_changed"));
        assert!(RUST_FORWARDED_EVENTS.contains(&"session:persona_changed"));
    }

    #[test]
    fn capability_snapshot_policy_is_bounded_before_web_access_starts() {
        let policy = AccessPolicy::load().expect("embedded policy");
        assert!(
            policy.capability_snapshot_wire_bytes().unwrap()
                <= relay_client::MAX_CONTROL_FRAME_BYTES
        );

        let mut oversized = AccessPolicy::default();
        oversized
            .commands
            .insert("x".repeat(relay_client::MAX_CONTROL_FRAME_BYTES));
        let error = oversized
            .validate_capability_snapshot()
            .expect_err("oversized capability snapshot must fail closed");
        assert!(error.contains("capability snapshot is too large"));
    }

    #[test]
    fn web_access_config_matches_relay_bounds_and_has_a_bounded_loader() {
        let valid = WebAccessConfig {
            relay_url: "wss://example.test/ws".into(),
            endpoint_id: "ep_123456".into(),
            access_token: "a".repeat(24),
            desktop_secret: "d".repeat(24),
        };
        validate_config(&valid).expect("valid Web access config");

        let mut invalid = valid.clone();
        invalid.endpoint_id = "e".repeat(129);
        assert!(validate_config(&invalid).is_err());
        invalid = valid.clone();
        invalid.access_token = "a".repeat(1_025);
        assert!(validate_config(&invalid).is_err());
        invalid = valid.clone();
        invalid.relay_url = "wss://example.test/ws with-space".into();
        assert!(validate_config(&invalid).is_err());

        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-config-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(8)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("web-access.json");
        std::fs::write(&path, serde_json::to_vec(&valid).unwrap()).unwrap();
        assert_eq!(load_config_from(&path).unwrap(), Some(valid));

        std::fs::write(&path, vec![b'x'; MAX_WEB_ACCESS_CONFIG_BYTES + 1]).unwrap();
        let error = load_config_from(&path).expect_err("oversized config must fail closed");
        assert!(error.contains("config is larger"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn pending_rpc(
        dispatched_generation: Option<&str>,
        acknowledged_generation: Option<&str>,
    ) -> RpcCacheEntry {
        RpcCacheEntry::Pending(PendingRpc {
            lease_id: "lease".into(),
            command: "chat".into(),
            args: json!({ "sessionId": "s1" }),
            fingerprint: "a".repeat(64),
            dispatched_generation: dispatched_generation.map(ToOwned::to_owned),
            acknowledged_generation: acknowledged_generation.map(ToOwned::to_owned),
            dispatched_at: Instant::now(),
        })
    }

    #[test]
    fn a_new_bridge_generation_redelivers_only_unacknowledged_rpcs() {
        let mut inner = Inner::default();
        inner.bridge_generation = Some("generation-old".into());
        inner
            .rpc_cache
            .insert("queued".into(), pending_rpc(Some("generation-old"), None));
        inner.rpc_order.push_back("queued".into());

        let first = prepare_bridge_generation(&mut inner, "generation-new");
        assert_eq!(first.len(), 1);
        assert!(matches!(
            &first[0],
            RpcReadyAction::Dispatch(dispatch)
                if dispatch.request_id == "queued"
                    && dispatch.command == "chat"
                    && dispatch.bridge_generation == "generation-new"
        ));
        assert!(prepare_bridge_generation(&mut inner, "generation-new").is_empty());
    }

    #[test]
    fn an_acknowledged_rpc_becomes_outcome_unknown_after_bridge_replacement() {
        let mut inner = Inner::default();
        inner.bridge_generation = Some("generation-old".into());
        inner.rpc_cache.insert(
            "started".into(),
            pending_rpc(Some("generation-old"), Some("generation-old")),
        );
        inner.rpc_order.push_back("started".into());

        assert!(prepare_bridge_generation(&mut inner, "generation-new").is_empty());
        let Some(RpcCacheEntry::Complete { completion, .. }) = inner.rpc_cache.get("started")
        else {
            panic!("started request must become a conservative completion");
        };
        assert!(!completion.ok);
        assert_eq!(completion.error_code.as_deref(), Some("outcome_unknown"));
    }

    #[test]
    fn rpc_tombstones_survive_serialization_and_reject_conflicting_reuse() {
        let fingerprint = rpc_fingerprint("chat", &json!({ "b": 2, "a": 1 })).unwrap();
        let mut ledger = RpcLedger::for_endpoint("ep_test");
        assert!(ledger.remember("request-1", &fingerprint).unwrap());
        let encoded = serde_json::to_vec(&ledger).unwrap();
        let restored: RpcLedger = serde_json::from_slice(&encoded).unwrap();
        let tombstone = restored.get("request-1").expect("durable tombstone");
        assert_eq!(
            tombstone_completion(tombstone, &fingerprint)
                .error_code
                .as_deref(),
            Some("request_not_started")
        );
        assert_eq!(
            tombstone_completion(tombstone, &"f".repeat(64))
                .error_code
                .as_deref(),
            Some("request_id_conflict")
        );
        let mut acknowledged = restored.clone();
        acknowledged
            .acknowledge("request-1", &fingerprint)
            .expect("acknowledge tombstone");
        assert_eq!(
            tombstone_completion(
                acknowledged
                    .get("request-1")
                    .expect("acknowledged tombstone"),
                &fingerprint,
            )
            .error_code
            .as_deref(),
            Some("outcome_unknown")
        );
    }

    #[test]
    fn rpc_ledger_retains_only_the_most_recent_512_ids() {
        let mut ledger = RpcLedger::for_endpoint("ep_test");
        for index in 0..=RPC_CACHE_CAPACITY {
            ledger
                .remember(&format!("request-{index}"), &"a".repeat(64))
                .unwrap();
        }
        assert_eq!(ledger.entries.len(), RPC_CACHE_CAPACITY);
        assert!(ledger.get("request-0").is_none());
        assert!(ledger
            .get(&format!("request-{RPC_CACHE_CAPACITY}"))
            .is_some());
    }

    #[test]
    fn rejected_rpcs_never_consume_durable_tombstone_capacity() {
        let protected_fingerprint = rpc_fingerprint("delete_session", &json!({ "id": "s1" }))
            .expect("protected fingerprint");
        let mut ledger = RpcLedger::for_endpoint("ep_test");
        ledger
            .remember("protected-request", &protected_fingerprint)
            .expect("remember protected request");
        ledger
            .acknowledge("protected-request", &protected_fingerprint)
            .expect("acknowledge protected request");

        let invalid_scope = rpc_error_completion("invalid_session_scope", "missing Session");
        for index in 0..RPC_CACHE_CAPACITY {
            let fingerprint = rpc_fingerprint("rejected", &json!({ "index": index }))
                .expect("rejected fingerprint");
            let admission = match index % 3 {
                0 => prepare_new_rpc_admission(
                    &ledger,
                    &format!("rejected-{index}"),
                    &fingerprint,
                    Some(&invalid_scope),
                    true,
                    0,
                ),
                1 => prepare_new_rpc_admission(
                    &ledger,
                    &format!("rejected-{index}"),
                    &fingerprint,
                    None,
                    false,
                    0,
                ),
                _ => prepare_new_rpc_admission(
                    &ledger,
                    &format!("rejected-{index}"),
                    &fingerprint,
                    None,
                    true,
                    MAX_RPC_IN_FLIGHT,
                ),
            }
            .expect("classify rejected request");
            assert!(matches!(admission, NewRpcAdmission::Rejected(_)));
        }

        assert_eq!(ledger.entries.len(), 1);
        let protected = ledger
            .get("protected-request")
            .expect("acknowledged tombstone must remain");
        assert!(protected.acknowledged);
        assert_eq!(protected.fingerprint, protected_fingerprint);
    }

    #[test]
    fn rpc_command_and_error_envelopes_are_bounded() {
        assert!(validate_rpc_command("web_access_chat").is_ok());
        assert!(validate_rpc_command("workflow:resume-2").is_ok());
        assert!(validate_rpc_command("").is_err());
        assert!(validate_rpc_command(&"x".repeat(MAX_RPC_COMMAND_BYTES + 1)).is_err());
        assert!(validate_rpc_command("chat/../../native").is_err());

        let completion = rpc_error_completion("test", "e".repeat(20_000));
        assert_eq!(
            completion.error.as_deref().unwrap_or_default().len(),
            16_384
        );
    }

    #[test]
    fn native_active_session_fallbacks_are_not_valid_web_scopes() {
        for command in [
            "cancel_generation",
            "compact_now",
            "edit_last_turn",
            "get_session_pinvou_scene_events",
            "get_session_timeline",
            "kick_workflow",
            "save_session_pinvou_scene_events",
            "stop_workflow",
            "web_access_chat",
        ] {
            assert_eq!(
                web_session_scope(command),
                Some(WebSessionScope::Required("sessionId")),
                "{command} must require the browser-selected Session"
            );
        }
        assert_eq!(
            web_session_scope("start_workflow"),
            Some(WebSessionScope::Optional("sessionId"))
        );
    }

    #[test]
    fn oversized_rpc_results_become_small_structured_errors() {
        let completion = bounded_rpc_completion(
            true,
            Some(Value::String("x".repeat(MAX_RPC_RESPONSE_BYTES + 1))),
            None,
        );
        assert!(!completion.ok);
        assert_eq!(completion.error_code.as_deref(), Some("response_too_large"));
        assert!(serde_json::to_vec(&completion.result).unwrap().len() < 1024);
    }

    #[test]
    fn process_lock_rejects_a_second_desktop_owner() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-access-lock-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("owner.lock");
        let first = acquire_process_lock(&path).expect("first owner");
        assert!(acquire_process_lock(&path).is_err());
        drop(first);
        let replacement = acquire_process_lock(&path).expect("replacement after release");
        drop(replacement);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rpc_fingerprint_is_independent_of_object_key_order() {
        assert_eq!(
            rpc_fingerprint("chat", &json!({ "a": 1, "b": { "x": 2, "y": 3 } })).unwrap(),
            rpc_fingerprint("chat", &json!({ "b": { "y": 3, "x": 2 }, "a": 1 })).unwrap()
        );
    }

    #[test]
    fn public_link_keeps_secrets_in_the_fragment_and_encodes_relay() {
        let config = WebAccessConfig {
            relay_url: "ws://127.0.0.1:8787/ws?x=1".into(),
            endpoint_id: "ep_test".into(),
            access_token: "a+b/c".into(),
            desktop_secret: "never-in-url".into(),
        };
        let link = public_url(&config);
        assert!(link.contains("#endpoint=ep_test&token=a%2Bb%2Fc"));
        assert!(link.contains("relay=ws%3A%2F%2F127.0.0.1%3A8787%2Fws%3Fx%3D1"));
        assert!(!link.contains("never-in-url"));
    }

    #[test]
    fn private_json_write_atomically_replaces_the_target() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-access-test-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.json");
        atomic_write_private_json(&path, &json!({ "value": 1 }), "test state").unwrap();
        atomic_write_private_json(&path, &json!({ "value": 2 }), "test state").unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["value"], 2);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert!(platform::private_file_is_restricted(&path).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn test_pending_revocation(endpoint_id: &str) -> PendingRevocation {
        PendingRevocation {
            relay_url: "wss://relay.example.test/ws".into(),
            endpoint_id: endpoint_id.into(),
            desktop_secret: format!("desktop-secret-{endpoint_id}-0123456789"),
        }
    }

    fn test_web_config(endpoint_id: &str) -> WebAccessConfig {
        let pending = test_pending_revocation(endpoint_id);
        WebAccessConfig {
            relay_url: pending.relay_url,
            endpoint_id: pending.endpoint_id,
            access_token: "browser-token-01234567890123456789".into(),
            desktop_secret: pending.desktop_secret,
        }
    }

    #[test]
    fn pending_revocations_round_trip_in_a_bounded_private_ledger() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-revocation-roundtrip-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        let first = test_pending_revocation("ep_retired_one");
        let second = test_pending_revocation("ep_retired_two");

        queue_pending_revocation_at(&path, first.clone()).unwrap();
        queue_pending_revocation_at(&path, second.clone()).unwrap();
        // An idempotent retry must not grow the durable queue.
        queue_pending_revocation_at(&path, first.clone()).unwrap();

        let restored = load_pending_revocations_from(&path).unwrap();
        assert_eq!(restored.version, PENDING_REVOCATIONS_VERSION);
        assert_eq!(restored.entries, VecDeque::from([first, second]));
        assert!(platform::private_file_is_restricted(&path).unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn revocation_ack_removes_only_the_matching_durable_entry() {
        let directory = std::env::temp_dir().join(format!(
            "pinvou-web-revocation-ack-{}-{}",
            std::process::id(),
            crate::features::remote_control::short_token(12)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("pending.json");
        let first = test_pending_revocation("ep_retired_one");
        let second = test_pending_revocation("ep_retired_two");
        queue_pending_revocation_at(&path, first.clone()).unwrap();
        queue_pending_revocation_at(&path, second.clone()).unwrap();

        acknowledge_pending_revocation_at(&path, &first).unwrap();
        assert_eq!(
            load_pending_revocations_from(&path).unwrap().entries,
            VecDeque::from([second.clone()])
        );
        acknowledge_pending_revocation_at(&path, &second).unwrap();
        assert!(!path.exists(), "an empty revocation ledger is removed");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restart_replays_retired_entries_but_not_an_uncommitted_current_one() {
        let current = test_web_config("ep_current");
        let retired = test_pending_revocation("ep_retired");
        let prepared_current = PendingRevocation::from(&current);
        let encoded = serde_json::to_vec(&PendingRevocationLedger {
            version: PENDING_REVOCATIONS_VERSION,
            entries: VecDeque::from([prepared_current.clone(), retired.clone()]),
        })
        .unwrap();
        let restored: PendingRevocationLedger = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(
            eligible_pending_revocations(&restored, Some(&current)),
            vec![retired.clone()]
        );
        assert_eq!(
            eligible_pending_revocations(&restored, None),
            vec![prepared_current, retired]
        );
    }

    #[test]
    fn only_explicit_terminal_relay_messages_acknowledge_a_revocation() {
        assert!(pending_revocation_ack(
            &json!({
                "type": "desktop_endpoint_revoked",
                "endpoint_id": "ep_retired"
            }),
            "ep_retired"
        ));
        assert!(pending_revocation_ack(
            &json!({
                "type": "desktop_endpoint_replaced",
                "endpoint_id": "ep_retired"
            }),
            "ep_retired"
        ));
        assert!(pending_revocation_ack(
            &json!({ "type": "error", "code": "endpoint_not_found" }),
            "ep_retired"
        ));
        assert!(!pending_revocation_ack(
            &json!({
                "type": "desktop_endpoint_revoked",
                "endpoint_id": "ep_other"
            }),
            "ep_retired"
        ));
    }

    #[test]
    fn active_session_downloads_are_rejected_not_evicted() {
        let download = |id: &str, reserved_bytes: usize| {
            (
                id.to_string(),
                WebSessionDownload {
                    session_id: format!("session_{id}"),
                    path: PathBuf::from(format!("{id}.json")),
                    reserved_bytes,
                    total: reserved_bytes,
                    ready: true,
                    last_touched: Instant::now(),
                },
            )
        };

        let mut by_count = Inner::default();
        by_count
            .web_session_downloads
            .extend([download("first", 1), download("second", 1)]);
        let ids_before = by_count
            .web_session_downloads
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        assert!(ensure_web_session_download_capacity(&by_count, 1).is_err());
        assert_eq!(
            by_count
                .web_session_downloads
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
            ids_before
        );

        let mut by_bytes = Inner::default();
        by_bytes
            .web_session_downloads
            .extend([download("large", MAX_WEB_SESSION_DOWNLOAD_TOTAL_BYTES - 1)]);
        assert!(ensure_web_session_download_capacity(&by_bytes, 2).is_err());
        assert!(by_bytes.web_session_downloads.contains_key("large"));
    }
}
