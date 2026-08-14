use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::StreamExt;
use parking_lot::{Mutex, RwLock};
use sha2::{Digest, Sha256};

use crate::client::normalize_endpoint;
use crate::model::*;
use crate::parser::parse_document;
use crate::store::{DocumentIndexUpdate, Store};
use crate::tls::TlsIdentity;
use crate::{chunk_text, Embedder, MAX_UPLOAD_BYTES};

pub const MODEL_NAME: &str = "bge-m3";
const CHUNK_CHARS: usize = 600;
const CHUNK_OVERLAP: usize = 90;
const EMBED_BATCH: usize = 32;
// Bound parsed/indexed text independently from the source upload. Office/PDF
// extraction can expand a compressed source substantially, and keeping the
// text, chunks and BGE-M3 vectors alive together otherwise creates a several-
// hundred-MiB peak for a single document.
const MAX_INDEX_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_INDEX_CHUNKS: usize = 16_000;
const MAX_VECTOR_DIMENSIONS: usize = 4096;
const MAX_TOTAL_VECTOR_FLOATS: usize = 16 * 1024 * 1024;
const TRASH_RETENTION_SECONDS: i64 = 30 * 24 * 60 * 60;
const JOIN_REQUEST_RATE_WINDOW: Duration = Duration::from_secs(60);
const JOIN_REQUEST_PER_SOURCE_LIMIT: usize = 6;
const JOIN_REQUEST_GLOBAL_LIMIT: usize = 300;
const DEFAULT_SHARE_HOURS: u64 = 24;
const MAX_SHARE_HOURS: u64 = 7 * 24;
const JOIN_REQUEST_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
const HOST_OWNER_DEVICE_META: &str = "host_owner_device_id";

pub struct ServiceBoot {
    pub service: Arc<KnowledgeService>,
}

pub struct KnowledgeService {
    data_dir: PathBuf,
    documents_dir: PathBuf,
    model_dir: PathBuf,
    tls: TlsIdentity,
    store: Store,
    embedder: RwLock<Option<Arc<Embedder>>>,
    model_error: RwLock<Option<String>>,
    model_downloading: AtomicBool,
    join_request_rate: Mutex<AttemptRate>,
    indexing: tokio::sync::Mutex<()>,
    search_slots: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct AttemptRate {
    global: VecDeque<Instant>,
    by_source: HashMap<IpAddr, VecDeque<Instant>>,
}

enum RateDecision {
    Allowed,
    SourceLimited,
    GlobalLimited,
}

fn check_attempt_rate(
    rate: &Mutex<AttemptRate>,
    source: IpAddr,
    window: Duration,
    per_source_limit: usize,
    global_limit: usize,
) -> RateDecision {
    let now = Instant::now();
    let cutoff = now - window;
    let mut rate = rate.lock();
    while rate.global.front().is_some_and(|seen| *seen <= cutoff) {
        rate.global.pop_front();
    }
    rate.by_source.retain(|_, attempts| {
        while attempts.front().is_some_and(|seen| *seen <= cutoff) {
            attempts.pop_front();
        }
        !attempts.is_empty()
    });
    if rate.global.len() >= global_limit {
        return RateDecision::GlobalLimited;
    }
    let source_attempts = rate.by_source.entry(source).or_default();
    if source_attempts.len() >= per_source_limit {
        return RateDecision::SourceLimited;
    }
    source_attempts.push_back(now);
    rate.global.push_back(now);
    RateDecision::Allowed
}

impl KnowledgeService {
    pub fn boot(data_dir: PathBuf, model_dir: Option<PathBuf>) -> Result<ServiceBoot, String> {
        crate::backup::recover_interrupted_restore(&data_dir)?;
        std::fs::create_dir_all(&data_dir).map_err(|error| error.to_string())?;
        let tls = crate::tls::ensure_tls_identity(&data_dir)?;
        let documents_dir = data_dir.join("documents");
        std::fs::create_dir_all(&documents_dir).map_err(|error| error.to_string())?;
        let model_dir = model_dir.unwrap_or_else(|| data_dir.join("models").join(MODEL_NAME));
        let store =
            Store::open(&data_dir.join("knowledge.db")).map_err(|error| error.to_string())?;
        if store
            .meta("server_id")
            .map_err(|error| error.to_string())?
            .is_none()
        {
            store
                .set_meta("server_id", &random_secret(18))
                .map_err(|error| error.to_string())?;
        }
        if store
            .meta("server_name")
            .map_err(|error| error.to_string())?
            .is_none()
        {
            store
                .set_meta("server_name", "PINVOU Knowledge")
                .map_err(|error| error.to_string())?;
        }
        if store
            .meta("server_identity")
            .map_err(|error| error.to_string())?
            .is_none()
        {
            store
                .set_meta("server_identity", &random_secret(32))
                .map_err(|error| error.to_string())?;
        }
        // v2 is claimed by the host PINVOU through an Owner device credential.
        // Remove obsolete Web-console and abandoned quick-join credentials during migration.
        let _ = std::fs::remove_file(data_dir.join("initialization.key"));
        for key in [
            "initialization_key_hash",
            "owner_password",
            "lan_read_access_enabled",
        ] {
            store.delete_meta(key).map_err(|error| error.to_string())?;
        }
        let expired_paths = store
            .purge_expired_trash(chrono::Utc::now().timestamp() - TRASH_RETENTION_SECONDS)
            .map_err(|error| error.to_string())?;
        for storage_path in expired_paths {
            let _ = std::fs::remove_file(documents_dir.join(storage_path));
        }
        let service = Arc::new(Self {
            data_dir,
            documents_dir,
            model_dir,
            tls,
            store,
            embedder: RwLock::new(None),
            model_error: RwLock::new(None),
            model_downloading: AtomicBool::new(false),
            join_request_rate: Mutex::new(AttemptRate::default()),
            indexing: tokio::sync::Mutex::new(()),
            search_slots: Arc::new(tokio::sync::Semaphore::new(search_parallelism())),
        });
        if service.model_directory_complete() {
            if let Err(error) = service.load_model() {
                *service.model_error.write() = Some(error);
            }
        }
        Ok(ServiceBoot { service })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn tls_identity(&self) -> &TlsIdentity {
        &self.tls
    }

    pub fn server_info(&self) -> Result<ServerInfo, String> {
        Ok(ServerInfo {
            server_id: self
                .store
                .meta("server_id")
                .map_err(|error| error.to_string())?
                .unwrap_or_default(),
            identity: self
                .store
                .meta("server_identity")
                .map_err(|error| error.to_string())?
                .unwrap_or_default(),
            name: self
                .store
                .meta("server_name")
                .map_err(|error| error.to_string())?
                .unwrap_or_else(|| "PINVOU Knowledge".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: 2,
            tls_ca: self.tls.ca_encoded.clone(),
            initialized: self.initialized(),
            ready: self.ready(),
            model: MODEL_NAME.to_string(),
        })
    }

    pub fn initialized(&self) -> bool {
        self.store
            .list_devices()
            .map(|devices| {
                devices
                    .iter()
                    .any(|device| device.scope.is_owner() && !device.revoked)
            })
            .unwrap_or(false)
    }

    pub fn ready(&self) -> bool {
        self.embedder.read().is_some()
    }

    pub fn model_error(&self) -> Option<String> {
        self.model_error.read().clone()
    }

    pub fn model_status(&self) -> ModelStatus {
        ModelStatus {
            name: MODEL_NAME.to_string(),
            ready: self.ready(),
            downloading: self.model_downloading(),
            error: self.model_error(),
        }
    }

    pub fn record_model_error(&self, error: String) {
        *self.model_error.write() = Some(error);
    }

    pub fn model_downloading(&self) -> bool {
        self.model_downloading.load(Ordering::Acquire)
    }

    pub fn begin_model_download(&self) -> Result<(), String> {
        self.model_downloading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "模型正在下载，请勿重复启动".to_string())?;
        *self.model_error.write() = None;
        Ok(())
    }

    pub fn finish_model_download(&self) {
        self.model_downloading.store(false, Ordering::Release);
    }

    pub fn ensure_host_owner(&self, device_name: &str, token: &str) -> Result<DeviceGrant, String> {
        let device_name = normalized_required_name(device_name)?;
        if token.len() < 32 || token.chars().any(char::is_control) {
            return Err("本机所有者凭据无效".to_string());
        }
        if let Some(current) = self.authorize(token)? {
            if current.scope.is_owner() {
                return Ok(current);
            }
            return Err("此设备凭据已经用于非所有者成员".to_string());
        }
        if self
            .store
            .list_devices()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|device| device.scope.is_owner() && !device.revoked)
        {
            return Err("共享知识库已经绑定主机所有者".to_string());
        }
        let device_id = random_secret(18);
        self.store
            .add_host_owner_device(
                &device_id,
                &device_name,
                &hash_secret(token),
                HOST_OWNER_DEVICE_META,
            )
            .map_err(|error| error.to_string())?;
        self.store
            .device(&device_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "创建本机所有者失败".to_string())
    }

    pub fn provision_host_owner<F>(
        &self,
        device_name: &str,
        persist_claim: F,
    ) -> Result<Option<DeviceGrant>, String>
    where
        F: FnOnce(&str, &str) -> Result<(), String>,
    {
        if self
            .store
            .list_devices()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|device| device.scope.is_owner() && !device.revoked)
        {
            return Ok(None);
        }
        let device_name = normalized_required_name(device_name)?;
        let token = random_secret(32);
        let device_id = random_secret(18);
        // Persist the one-time claim before committing its token hash. A crash
        // can then leave only a harmless stale claim, which the next boot
        // overwrites, but can never leave an active Owner whose token was lost.
        persist_claim(&device_id, &token)?;
        self.store
            .add_host_owner_device(
                &device_id,
                &device_name,
                &hash_secret(&token),
                HOST_OWNER_DEVICE_META,
            )
            .map_err(|error| error.to_string())?;
        self.store
            .device(&device_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "创建本机所有者失败".to_string())
            .map(Some)
    }

    pub fn recover_host_owner<F>(
        &self,
        device_name: &str,
        persist_claim: F,
    ) -> Result<DeviceGrant, String>
    where
        F: FnOnce(&str, &str) -> Result<(), String>,
    {
        let device_name = normalized_required_name(device_name)?;
        let existing_id = match self
            .store
            .meta(HOST_OWNER_DEVICE_META)
            .map_err(|error| error.to_string())?
        {
            Some(device_id)
                if self
                    .store
                    .device(&device_id)
                    .map_err(|error| error.to_string())?
                    .is_some() =>
            {
                Some(device_id)
            }
            _ => None,
        };
        let device_id = existing_id.unwrap_or_else(|| random_secret(18));
        let token = random_secret(32);
        // Persist the claim before invalidating the old token. If recovery is
        // interrupted between these steps, rerunning it safely replaces the
        // stale claim and no unrecorded credential becomes authoritative.
        persist_claim(&device_id, &token)?;
        self.store
            .recover_host_owner_device(
                &device_id,
                &device_name,
                &hash_secret(&token),
                HOST_OWNER_DEVICE_META,
            )
            .map_err(|error| error.to_string())?;
        self.store
            .device(&device_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "恢复本机所有者失败".to_string())
    }

    pub fn set_owner_device(&self, device_id: &str, owner: bool) -> Result<DeviceGrant, String> {
        Self::set_owner_device_in_store(&self.store, device_id, owner)
    }

    pub fn is_host_owner_device(&self, device_id: &str) -> Result<bool, String> {
        Ok(self
            .store
            .meta(HOST_OWNER_DEVICE_META)
            .map_err(|error| error.to_string())?
            .as_deref()
            == Some(device_id))
    }

    pub fn set_owner_device_in_data_dir(
        data_dir: &Path,
        device_id: &str,
        owner: bool,
    ) -> Result<DeviceGrant, String> {
        let store =
            Store::open(&data_dir.join("knowledge.db")).map_err(|error| error.to_string())?;
        Self::set_owner_device_in_store(&store, device_id, owner)
    }

    fn set_owner_device_in_store(
        store: &Store,
        device_id: &str,
        owner: bool,
    ) -> Result<DeviceGrant, String> {
        if store
            .meta(HOST_OWNER_DEVICE_META)
            .map_err(|error| error.to_string())?
            .as_deref()
            == Some(device_id)
        {
            return Err("不能修改主机所有者设备".to_string());
        }
        let current = store
            .device(device_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "成员设备不存在".to_string())?;
        if current.revoked {
            return Err("请先恢复该成员设备".to_string());
        }
        store
            .update_device(
                device_id,
                None,
                Some(if owner {
                    AccessScope::Owner
                } else {
                    AccessScope::Manage
                }),
                None,
            )
            .map_err(|error| error.to_string())
    }

    pub fn create_share(&self, request: ShareCreateRequest) -> Result<ShareCreated, String> {
        let mut endpoints = Vec::new();
        for endpoint in request.endpoints {
            let endpoint = normalize_endpoint(&endpoint)?;
            if !endpoint.starts_with("https://") {
                return Err("分享地址必须使用 HTTPS".to_string());
            }
            if !is_private_share_endpoint(&endpoint) {
                return Err("分享地址必须是局域网或 Tailnet 中可达的私网地址".to_string());
            }
            if !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
        if endpoints.is_empty() || endpoints.len() > 8 {
            return Err("分享连接需要包含 1 至 8 个服务地址".to_string());
        }
        let share_id = random_secret(12);
        let secret = random_secret(32);
        let expires_at = chrono::Utc::now().timestamp()
            + request
                .expires_in_hours
                .unwrap_or(DEFAULT_SHARE_HOURS)
                .clamp(1, MAX_SHARE_HOURS) as i64
                * 60
                * 60;
        self.store
            .create_share(
                &share_id,
                &hash_secret(&secret),
                &endpoints,
                request.auto_approve_read,
                expires_at,
            )
            .map_err(|error| error.to_string())?;
        let info = self.server_info()?;
        let mut share =
            url::Url::parse("pinvou-knowledge://share").map_err(|error| error.to_string())?;
        {
            let mut query = share.query_pairs_mut();
            query
                .append_pair("server", &info.server_id)
                .append_pair("identity", &info.identity)
                .append_pair("ca", &info.tls_ca)
                .append_pair("share", &secret);
            for endpoint in endpoints {
                query.append_pair("endpoint", &endpoint);
            }
        }
        Ok(ShareCreated {
            share_id,
            share: share.to_string(),
            expires_at,
            auto_approve_read: request.auto_approve_read,
        })
    }

    pub fn list_shares(&self) -> Result<Vec<ShareRecord>, String> {
        self.store.list_shares().map_err(|error| error.to_string())
    }

    pub fn stop_share(&self, id: &str) -> Result<ShareRecord, String> {
        self.store
            .stop_share(id)
            .map_err(|_| "分享连接不存在或已经停止".to_string())
    }

    pub fn submit_join_request(
        &self,
        source: IpAddr,
        request: JoinRequestCreate,
    ) -> Result<JoinRequestReceipt, String> {
        self.check_join_request_rate(source)?;
        let device_name = normalized_required_name(&request.device_name)?;
        if request.device_token_hash.len() != 64
            || !request
                .device_token_hash
                .bytes()
                .all(|value| value.is_ascii_hexdigit())
        {
            return Err("设备凭据摘要无效".to_string());
        }
        if request.claim_secret.len() < 32
            || request.claim_secret.len() > 256
            || request.claim_secret.chars().any(char::is_control)
        {
            return Err("申请凭据无效".to_string());
        }
        let request_id = random_secret(18);
        let device_id = random_secret(18);
        let expires_at = chrono::Utc::now().timestamp() + JOIN_REQUEST_RETENTION_SECONDS;
        let join_request = self
            .store
            .create_join_request(
                &request_id,
                &hash_secret(&request.claim_secret),
                &device_name,
                &device_id,
                &request.device_token_hash.to_ascii_lowercase(),
                request.share_secret.as_deref().map(hash_secret).as_deref(),
                expires_at,
            )
            .map_err(|_| "分享连接无效、已停止或已过期".to_string())?;
        Ok(JoinRequestReceipt {
            request: join_request,
            server: self.server_info()?,
        })
    }

    pub fn claim_join_request(
        &self,
        id: &str,
        claim_secret: &str,
    ) -> Result<JoinRequestReceipt, String> {
        let request = self
            .store
            .join_request_by_claim(id, &hash_secret(claim_secret))
            .map_err(|_| "加入申请不存在或申请凭据无效".to_string())?;
        Ok(JoinRequestReceipt {
            request,
            server: self.server_info()?,
        })
    }

    pub fn cancel_join_request(
        &self,
        id: &str,
        claim_secret: &str,
    ) -> Result<JoinRequestRecord, String> {
        self.store
            .cancel_join_request(id, &hash_secret(claim_secret))
            .map_err(|_| "加入申请不存在或已经处理".to_string())
    }

    pub fn list_join_requests(
        &self,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<JoinRequestRecord>, String> {
        self.store
            .list_join_requests(limit, offset)
            .map_err(|error| error.to_string())
    }

    pub fn approve_join_request(
        &self,
        id: &str,
        scope: AccessScope,
    ) -> Result<JoinRequestRecord, String> {
        if scope.is_owner() {
            return Err("所有者设备只能由共享知识库主机从成员列表提升".to_string());
        }
        self.store
            .approve_join_request(id, scope)
            .map_err(|_| "加入申请不存在、已过期或已经处理".to_string())
    }

    pub fn reject_join_request(&self, id: &str) -> Result<JoinRequestRecord, String> {
        self.store
            .reject_join_request(id)
            .map_err(|_| "加入申请不存在或已经处理".to_string())
    }

    pub fn create_invite(&self, request: InviteCreateRequest) -> Result<InviteCreated, String> {
        let endpoint = normalize_endpoint(&request.endpoint)?;
        let id = random_secret(12);
        let secret = random_secret(32);
        let expires_at = chrono::Utc::now().timestamp()
            + request.expires_in_minutes.unwrap_or(15).clamp(1, 24 * 60) as i64 * 60;
        self.store
            .create_invite(
                &id,
                &hash_secret(&secret),
                request.scope,
                request.label.as_deref(),
                expires_at,
            )
            .map_err(|error| error.to_string())?;
        let mut invite =
            url::Url::parse("pinvou-knowledge://connect").map_err(|error| error.to_string())?;
        invite
            .query_pairs_mut()
            .append_pair("endpoint", &endpoint)
            .append_pair("invite", &secret);
        Ok(InviteCreated {
            invite_id: id,
            invite: invite.to_string(),
            expires_at,
        })
    }

    pub fn redeem_invite(&self, request: PairRequest) -> Result<PairResponse, String> {
        let device_name = normalized_name(&request.device_name, "Pinvou device")?;
        let token = random_secret(32);
        let device_id = random_secret(18);
        let scope = self
            .store
            .redeem_invite_and_add_device(
                &hash_secret(&request.invite_secret),
                &device_id,
                &device_name,
                &hash_secret(&token),
            )
            .map_err(|_| "连接邀请无效、已使用或已过期".to_string())?;
        Ok(PairResponse {
            server: self.server_info()?,
            token,
            scope,
            device_id,
        })
    }

    fn check_join_request_rate(&self, source: IpAddr) -> Result<(), String> {
        match check_attempt_rate(
            &self.join_request_rate,
            source,
            JOIN_REQUEST_RATE_WINDOW,
            JOIN_REQUEST_PER_SOURCE_LIMIT,
            JOIN_REQUEST_GLOBAL_LIMIT,
        ) {
            RateDecision::GlobalLimited => {
                return Err("加入申请过多，请稍后重试".to_string());
            }
            RateDecision::SourceLimited => {
                return Err("此设备申请过于频繁，请稍后重试".to_string());
            }
            RateDecision::Allowed => {}
        }
        Ok(())
    }

    pub fn authorize(&self, token: &str) -> Result<Option<DeviceGrant>, String> {
        if token.is_empty() {
            return Ok(None);
        }
        self.store
            .authorize_token(&hash_secret(token))
            .map_err(|error| error.to_string())
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceGrant>, String> {
        self.store.list_devices().map_err(|error| error.to_string())
    }

    pub fn device_count(&self) -> Result<i64, String> {
        self.store.device_count().map_err(|error| error.to_string())
    }

    pub fn list_devices_page(
        &self,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<DeviceGrant>, String> {
        self.store
            .list_devices_page(limit, offset)
            .map_err(|error| error.to_string())
    }

    pub fn update_device(
        &self,
        id: &str,
        request: UpdateDeviceRequest,
    ) -> Result<DeviceGrant, String> {
        self.store
            .update_device(id, request.name.as_deref(), request.scope, request.revoked)
            .map_err(|error| error.to_string())
    }

    pub fn delete_device(&self, id: &str) -> Result<(), String> {
        self.store
            .delete_device(id)
            .map_err(|_| "授权设备不存在".to_string())
    }

    pub fn collections(&self, include_deleted: bool) -> Result<Vec<Collection>, String> {
        self.store
            .list_collections(include_deleted)
            .map_err(|error| error.to_string())
    }

    pub fn trashed_collections_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Collection>, String> {
        self.store
            .list_trashed_collections_page(limit, offset)
            .map_err(|error| error.to_string())
    }

    pub fn trashed_documents_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<TrashedDocument>, String> {
        self.store
            .list_trashed_documents_page(limit, offset)
            .map_err(|error| error.to_string())
    }

    pub fn create_collection(
        &self,
        request: CreateCollectionRequest,
    ) -> Result<Collection, String> {
        let name = normalized_name(&request.name, "知识集")?;
        self.store
            .create_collection(&name, request.description.as_deref())
            .map_err(|error| error.to_string())
    }

    pub fn update_collection(
        &self,
        id: i64,
        request: CreateCollectionRequest,
    ) -> Result<Collection, String> {
        let name = normalized_name(&request.name, "知识集")?;
        self.store
            .update_collection(id, &name, request.description.as_deref())
            .map_err(|_| "知识集不存在或已删除".to_string())?;
        self.store
            .collection(id, false)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "知识集不存在或已删除".to_string())
    }

    pub fn trash_collection(&self, id: i64) -> Result<(), String> {
        self.store
            .trash_collection(id)
            .map_err(|_| "知识集不存在或已删除".to_string())
    }

    pub fn restore_collection(self: &Arc<Self>, id: i64) -> Result<(), String> {
        self.store
            .restore_collection(id)
            .map_err(|_| "回收站中未找到该知识集".to_string())?;
        self.requeue_pending_indexing();
        Ok(())
    }

    pub fn permanently_delete_collection(&self, id: i64) -> Result<(), String> {
        let paths = self
            .store
            .permanently_delete_collection(id)
            .map_err(|_| "回收站中未找到该知识集".to_string())?;
        self.remove_managed_sources(paths);
        Ok(())
    }

    pub fn documents(
        &self,
        collection_id: i64,
        include_deleted: bool,
    ) -> Result<Vec<Document>, String> {
        self.store
            .list_documents(collection_id, include_deleted)
            .map_err(|error| error.to_string())
    }

    pub fn documents_page(
        &self,
        collection_id: i64,
        include_deleted: bool,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<Document>, String> {
        self.store
            .list_documents_page(collection_id, include_deleted, limit, offset)
            .map_err(|error| error.to_string())
    }

    pub fn document_statuses(&self, mut ids: Vec<i64>) -> Result<Vec<Document>, String> {
        ids.sort_unstable();
        ids.dedup();
        if ids.len() > 500 {
            return Err("单次最多查询 500 份文档状态".to_string());
        }
        self.store
            .documents_by_ids(&ids)
            .map_err(|error| error.to_string())
    }

    pub async fn upload_document(
        self: &Arc<Self>,
        collection_id: i64,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Document, String> {
        if bytes.is_empty() || bytes.len() > MAX_UPLOAD_BYTES {
            return Err(format!(
                "文件必须小于 {} MiB",
                MAX_UPLOAD_BYTES / 1024 / 1024
            ));
        }
        let filename = safe_filename(filename)?;
        let sha256 = hash_bytes(&bytes);
        let relative = PathBuf::from(random_secret(18)).join(&filename);
        let absolute = self.documents_dir.join(&relative);
        write_atomic(&absolute, &bytes)?;
        let ext = Path::new(&filename)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        let (mut document, created) = match self.store.insert_document_if_new(
            collection_id,
            &filename,
            ext.as_deref(),
            &relative.to_string_lossy(),
            bytes.len() as i64,
            &sha256,
        ) {
            Ok(document) => document,
            Err(error) => {
                let _ = std::fs::remove_file(&absolute);
                let _ = std::fs::remove_dir(absolute.parent().unwrap_or(&self.documents_dir));
                return Err(error.to_string());
            }
        };
        if !created {
            let _ = std::fs::remove_file(&absolute);
            let _ = std::fs::remove_dir(absolute.parent().unwrap_or(&self.documents_dir));
            document.already_exists = true;
            return Ok(document);
        }
        if self.ready() {
            // A successful upload means the managed source is durable. Index in the
            // background so a slow parser/model cannot make the client time out and
            // retry an upload that the server already committed.
            let background = Arc::clone(self);
            let document_id = document.id;
            tokio::spawn(async move {
                let _ = background.index_document(document_id).await;
            });
        }
        self.store
            .document(document.id, false)
            .map_err(|error| error.to_string())?
            .map(|stored| stored.document)
            .ok_or_else(|| "文档保存后不可见".to_string())
    }

    pub async fn replace_document(
        self: &Arc<Self>,
        document_id: i64,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Document, String> {
        if !self.ready() {
            return Err("embedding 模型未就绪，不能更新文档".to_string());
        }
        if bytes.is_empty() || bytes.len() > MAX_UPLOAD_BYTES {
            return Err(format!(
                "文件必须小于 {} MiB",
                MAX_UPLOAD_BYTES / 1024 / 1024
            ));
        }
        let _indexing = self.indexing.lock().await;
        let service = Arc::clone(self);
        let filename = filename.to_string();
        tokio::task::spawn_blocking(move || {
            service.replace_document_blocking(document_id, &filename, bytes)
        })
        .await
        .map_err(|error| format!("文档更新任务异常结束：{error}"))?
    }

    fn replace_document_blocking(
        &self,
        document_id: i64,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Document, String> {
        let current = self
            .store
            .document(document_id, false)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "文档不存在或已删除".to_string())?;
        let filename = safe_filename(filename)?;
        let relative = PathBuf::from(random_secret(18)).join(&filename);
        let absolute = self.documents_dir.join(&relative);
        write_atomic(&absolute, &bytes)?;
        let prepared = match self.prepare_index(&absolute) {
            Ok(value) => value,
            Err(error) => {
                let _ = std::fs::remove_file(&absolute);
                return Err(error);
            }
        };
        let ext = Path::new(&filename)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        if let Err(error) = self.store.replace_document_index(
            document_id,
            DocumentIndexUpdate {
                name: &filename,
                ext: ext.as_deref(),
                storage_path: &relative.to_string_lossy(),
                size: bytes.len() as i64,
                sha256: &hash_bytes(&bytes),
                chunks: &prepared.0,
                vectors: &prepared.1,
            },
        ) {
            let _ = std::fs::remove_file(&absolute);
            let _ = std::fs::remove_dir(absolute.parent().unwrap_or(&self.documents_dir));
            return Err(error.to_string());
        }
        let old = self.documents_dir.join(current.storage_path);
        if old != absolute {
            let _ = std::fs::remove_file(old);
        }
        self.store
            .document(document_id, false)
            .map_err(|error| error.to_string())?
            .map(|stored| stored.document)
            .ok_or_else(|| "文档更新后不可见".to_string())
    }

    pub async fn index_document(self: &Arc<Self>, document_id: i64) -> Result<(), String> {
        let _indexing = self.indexing.lock().await;
        let service = Arc::clone(self);
        let result =
            tokio::task::spawn_blocking(move || service.index_document_blocking(document_id)).await;
        match result {
            Ok(result) => result,
            Err(error) => {
                let message = format!("索引任务异常结束：{error}");
                let _ = self.store.mark_document_failed(document_id, &message);
                Err(message)
            }
        }
    }

    fn index_document_blocking(&self, document_id: i64) -> Result<(), String> {
        if !self.ready() {
            return Ok(());
        }
        let stored = self
            .store
            .document(document_id, false)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "文档不存在或已删除".to_string())?;
        if stored.document.status == "ready" {
            return Ok(());
        }
        let absolute = self.documents_dir.join(&stored.storage_path);
        let prepared = match self.prepare_index(&absolute) {
            Ok(value) => value,
            Err(error) => {
                self.store
                    .mark_document_failed(document_id, &error)
                    .map_err(|db_error| db_error.to_string())?;
                return Err(error);
            }
        };
        if let Err(error) = self.store.replace_document_index(
            document_id,
            DocumentIndexUpdate {
                name: &stored.document.name,
                ext: stored.document.ext.as_deref(),
                storage_path: &stored.storage_path,
                size: stored.document.size,
                sha256: &stored.document.sha256,
                chunks: &prepared.0,
                vectors: &prepared.1,
            },
        ) {
            let message = format!("索引落库失败：{error}");
            let _ = self.store.mark_document_failed(document_id, &message);
            return Err(message);
        }
        Ok(())
    }

    fn prepare_index(&self, path: &Path) -> Result<(Vec<String>, Vec<Vec<f32>>), String> {
        let text = parse_document(path)?;
        let chunks = prepare_chunks(&text)?;
        if chunks.is_empty() {
            return Err("文档没有可索引文本".to_string());
        }
        let embedder = self
            .embedder
            .read()
            .clone()
            .ok_or_else(|| "embedding 模型未就绪".to_string())?;
        let mut vectors = Vec::with_capacity(chunks.len());
        let mut total_vector_floats = 0usize;
        for batch in chunks.chunks(EMBED_BATCH) {
            let embedded = embedder.embed(batch)?;
            if embedded.len() != batch.len() {
                return Err("embedding 返回的向量数量不完整".to_string());
            }
            validate_vectors(&embedded)?;
            total_vector_floats =
                total_vector_floats.saturating_add(embedded.iter().map(Vec::len).sum::<usize>());
            if total_vector_floats > MAX_TOTAL_VECTOR_FLOATS {
                return Err("文档 embedding 向量总量超过 64 MiB 索引上限".to_string());
            }
            vectors.extend(embedded);
        }
        if vectors.len() != chunks.len() {
            return Err("embedding 返回的向量数量不完整".to_string());
        }
        Ok((chunks, vectors))
    }

    pub fn trash_document(&self, id: i64) -> Result<(), String> {
        self.store
            .trash_document(id)
            .map_err(|_| "文档不存在或已删除".to_string())
    }

    pub fn restore_document(self: &Arc<Self>, id: i64) -> Result<(), String> {
        self.store
            .restore_document(id)
            .map_err(|_| "回收站中未找到该文档，或所属知识集仍在回收站".to_string())?;
        self.requeue_pending_indexing();
        Ok(())
    }

    pub fn permanently_delete_document(&self, id: i64) -> Result<(), String> {
        let path = self
            .store
            .permanently_delete_document(id)
            .map_err(|_| "回收站中未找到该文档，或所属知识集仍在回收站".to_string())?;
        self.remove_managed_sources([path]);
        Ok(())
    }

    fn remove_managed_sources(&self, paths: impl IntoIterator<Item = String>) {
        for storage_path in paths {
            let relative = Path::new(&storage_path);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                continue;
            }
            let absolute = self.documents_dir.join(relative);
            let _ = std::fs::remove_file(&absolute);
            if let Some(parent) = absolute.parent() {
                if parent != self.documents_dir {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
    }

    fn requeue_pending_indexing(self: &Arc<Self>) {
        if self.ready() {
            let service = Arc::clone(self);
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = service.index_pending_documents().await;
                });
            }
        }
    }

    pub fn search(&self, request: SearchRequest) -> Result<Vec<SearchHit>, String> {
        let query = request.query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let embedder = self
            .embedder
            .read()
            .clone()
            .ok_or_else(|| "embedding 模型未就绪".to_string())?;
        let vector = embedder.embed_one(query)?;
        self.store
            .search(
                &request.collection_ids,
                query,
                &vector,
                request.limit.clamp(1, 50),
            )
            .map_err(|error| error.to_string())
    }

    pub async fn backfill_vector_signature_index(self: &Arc<Self>) -> Result<(), String> {
        const BATCH_SIZE: usize = 512;
        loop {
            let service = Arc::clone(self);
            let updated = tokio::task::spawn_blocking(move || {
                service
                    .store
                    .backfill_vector_signatures(BATCH_SIZE)
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| format!("向量索引迁移任务异常结束：{error}"))??;
            if updated < BATCH_SIZE {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    }

    pub async fn acquire_search_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        tokio::time::timeout(
            Duration::from_secs(10),
            Arc::clone(&self.search_slots).acquire_owned(),
        )
        .await
        .map_err(|_| "检索请求较多，请稍后重试".to_string())?
        .map_err(|_| "检索服务已停止".to_string())
    }

    pub fn source_window(&self, request: SourceWindowRequest) -> Result<SourceWindow, String> {
        let stored = self
            .store
            .document(request.document_id, false)
            .map_err(|error| error.to_string())?
            .filter(|stored| stored.document.collection_id == request.collection_id)
            .ok_or_else(|| "来源不属于该知识集或已删除".to_string())?;
        let chunks = self
            .store
            .source_chunks(
                request.collection_id,
                request.document_id,
                request.start_ord,
                request.limit,
            )
            .map_err(|error| error.to_string())?;
        Ok(SourceWindow {
            document: stored.document,
            chunks,
        })
    }

    pub fn source_file(&self, document_id: i64) -> Result<(Document, PathBuf), String> {
        let stored = self
            .store
            .document(document_id, false)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "文档不存在或已删除".to_string())?;
        let path = self.documents_dir.join(&stored.storage_path);
        if !path.is_file() {
            return Err("受管源文件丢失".to_string());
        }
        Ok((stored.document, path))
    }

    pub fn load_model(&self) -> Result<(), String> {
        let embedder = Embedder::from_dir(&self.model_dir, MODEL_NAME)?;
        *self.embedder.write() = Some(Arc::new(embedder));
        *self.model_error.write() = None;
        Ok(())
    }

    pub async fn load_model_and_index_pending(self: &Arc<Self>) -> Result<(), String> {
        self.load_model()?;
        self.index_pending_documents().await
    }

    pub async fn index_pending_documents(self: &Arc<Self>) -> Result<(), String> {
        let pending = self
            .store
            .pending_document_ids()
            .map_err(|error| error.to_string())?;
        for id in pending {
            let _ = self.index_document(id).await;
        }
        Ok(())
    }

    pub async fn download_model(self: &Arc<Self>) -> Result<(), String> {
        let parent = self
            .model_dir
            .parent()
            .ok_or_else(|| "模型目录无父目录".to_string())?
            .to_path_buf();
        std::fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
        let download = parent.join(format!(".{MODEL_NAME}.tar.gz.part"));
        let candidate = parent.join(format!(".{MODEL_NAME}.candidate-{}", random_secret(8)));
        let _ = std::fs::remove_file(&download);
        let result = async {
            std::fs::create_dir_all(&candidate).map_err(|error| error.to_string())?;
            let url = std::env::var("PINVOU_KNOWLEDGE_MODEL_URL")
                .unwrap_or_else(|_| crate::KNOWLEDGE_MODEL_ARCHIVE_URL.to_string());
            if url.trim().is_empty() {
                return Err("PINVOU_KNOWLEDGE_MODEL_URL 不能为空".to_string());
            }
            download_file(&url, &download).await?;
            let expected = std::env::var("PINVOU_KNOWLEDGE_MODEL_SHA256")
                .unwrap_or_else(|_| crate::KNOWLEDGE_MODEL_ARCHIVE_SHA256.to_string());
            verify_file(&download, &expected)?;
            let archive = std::fs::File::open(&download).map_err(|error| error.to_string())?;
            let decoder = flate2::read::GzDecoder::new(archive);
            let mut archive = tar::Archive::new(decoder);
            for entry in archive.entries().map_err(|error| error.to_string())? {
                let mut entry = entry.map_err(|error| error.to_string())?;
                if !entry
                    .unpack_in(&candidate)
                    .map_err(|error| error.to_string())?
                {
                    return Err("模型压缩包包含不安全路径".to_string());
                }
            }
            Embedder::from_dir(&candidate, MODEL_NAME)?;
            let backup = parent.join(format!(".{MODEL_NAME}.backup-{}", random_secret(8)));
            if self.model_dir.exists() {
                std::fs::rename(&self.model_dir, &backup).map_err(|error| error.to_string())?;
            }
            if let Err(error) = std::fs::rename(&candidate, &self.model_dir) {
                if backup.exists() {
                    let _ = std::fs::rename(&backup, &self.model_dir);
                }
                return Err(error.to_string());
            }
            let _ = std::fs::remove_file(&download);
            if backup.exists() {
                let _ = std::fs::remove_dir_all(backup);
            }
            self.load_model_and_index_pending().await
        }
        .await;
        if result.is_err() {
            let _ = std::fs::remove_file(&download);
            let _ = std::fs::remove_dir_all(&candidate);
        }
        result
    }

    fn model_directory_complete(&self) -> bool {
        (self.model_dir.join("model.onnx").is_file()
            || self
                .model_dir
                .join("onnx")
                .join("model_int8.onnx")
                .is_file())
            && [
                "tokenizer.json",
                "config.json",
                "special_tokens_map.json",
                "tokenizer_config.json",
            ]
            .iter()
            .all(|file| self.model_dir.join(file).is_file())
    }
}

fn random_secret(bytes: usize) -> String {
    URL_SAFE_NO_PAD.encode(random_bytes(bytes))
}

fn random_bytes(bytes: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    value
}

fn hash_secret(value: &str) -> String {
    hash_bytes(value.as_bytes())
}

fn hash_bytes(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

async fn download_file(url: &str, destination: &Path) -> Result<(), String> {
    crate::ensure_tls_crypto_provider();
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(90))
        .timeout(std::time::Duration::from_secs(60 * 60))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let mut file = std::fs::File::create(destination).map_err(|error| error.to_string())?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        use std::io::Write;
        file.write_all(&chunk.map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    }
    file.sync_all().map_err(|error| error.to_string())
}

fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    let actual = hash_file(path)?;
    if expected.eq_ignore_ascii_case(&actual) {
        Ok(())
    } else {
        Err(format!(
            "模型 SHA-256 校验失败({})：期望 {expected}，实际 {actual}",
            path.display()
        ))
    }
}

fn normalized_name(value: &str, fallback: &str) -> Result<String, String> {
    let value = value.trim();
    let value = if value.is_empty() { fallback } else { value };
    if value.chars().count() > 120 || value.chars().any(char::is_control) {
        return Err("名称无效或过长".to_string());
    }
    Ok(value.to_string())
}

fn normalized_required_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("请输入姓名或设备名称".to_string());
    }
    normalized_name(value, value)
}

fn is_private_share_endpoint(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Ipv4(address)) => {
            let value = u32::from(address);
            let tailnet = value >= u32::from_be_bytes([100, 64, 0, 0])
                && value <= u32::from_be_bytes([100, 127, 255, 255]);
            (address.is_private() || address.is_link_local() || tailnet) && !address.is_loopback()
        }
        Some(url::Host::Ipv6(address)) => {
            let first = address.segments()[0];
            !address.is_loopback() && ((first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
        }
        Some(url::Host::Domain(host)) => [".local", ".lan", ".internal", ".home.arpa", ".ts.net"]
            .iter()
            .any(|suffix| host.trim_end_matches('.').ends_with(suffix)),
        None => false,
    }
}

fn safe_filename(value: &str) -> Result<String, String> {
    let name = Path::new(value)
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or_else(|| "文件名无效".to_string())?;
    if name.is_empty() || name == "." || name == ".." || name.chars().any(char::is_control) {
        return Err("文件名无效".to_string());
    }
    Ok(name.chars().take(240).collect())
}

fn search_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .div_ceil(2)
        .clamp(1, 4)
}

fn prepare_chunks(text: &str) -> Result<Vec<String>, String> {
    if text.len() > MAX_INDEX_TEXT_BYTES {
        return Err(format!(
            "文档提取文本超过 {} MiB 索引上限，请拆分后上传",
            MAX_INDEX_TEXT_BYTES / 1024 / 1024
        ));
    }
    let chunks = chunk_text(text, CHUNK_CHARS, CHUNK_OVERLAP);
    if chunks.len() > MAX_INDEX_CHUNKS {
        return Err(format!(
            "文档切块超过 {MAX_INDEX_CHUNKS} 个索引上限，请拆分后上传"
        ));
    }
    Ok(chunks)
}

fn validate_vectors(vectors: &[Vec<f32>]) -> Result<(), String> {
    if vectors
        .iter()
        .any(|vector| vector.is_empty() || vector.len() > MAX_VECTOR_DIMENSIONS)
    {
        return Err(format!(
            "embedding 向量维度无效或超过 {MAX_VECTOR_DIMENSIONS} 维上限"
        ));
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "受管文件路径无效".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(".upload-{}.part", random_secret(8)));
    std::fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_rejects_expanded_text_and_excessive_vectors_before_persisting() {
        let oversized_text = "x".repeat(MAX_INDEX_TEXT_BYTES + 1);
        let text_error = prepare_chunks(&oversized_text).unwrap_err();
        assert!(text_error.contains("索引上限"));

        let oversized_vector = vec![vec![0.0; MAX_VECTOR_DIMENSIONS + 1]];
        let vector_error = validate_vectors(&oversized_vector).unwrap_err();
        assert!(vector_error.contains("向量维度"));
    }

    #[test]
    fn share_endpoints_allow_lan_and_tailnet_but_reject_public_or_loopback_hosts() {
        for endpoint in [
            "https://192.168.1.20:3210",
            "https://100.64.12.34:3210",
            "https://[fd7a:115c:a1e0::1]:3210",
            "https://cube.internal:3210",
            "https://cube.tailnet-name.ts.net:3210",
        ] {
            assert!(is_private_share_endpoint(endpoint), "{endpoint}");
        }
        for endpoint in [
            "https://127.0.0.1:3210",
            "https://8.8.8.8:3210",
            "https://knowledge.example.com",
        ] {
            assert!(!is_private_share_endpoint(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn boot_removes_obsolete_web_admin_and_quick_join_state() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("knowledge.db");
        let store = Store::open(&database).unwrap();
        for key in [
            "initialization_key_hash",
            "owner_password",
            "lan_read_access_enabled",
        ] {
            store.set_meta(key, "obsolete").unwrap();
        }
        drop(store);
        std::fs::write(root.path().join("initialization.key"), b"obsolete").unwrap();

        let boot = KnowledgeService::boot(root.path().to_path_buf(), None).unwrap();

        assert!(!root.path().join("initialization.key").exists());
        for key in [
            "initialization_key_hash",
            "owner_password",
            "lan_read_access_enabled",
        ] {
            assert_eq!(boot.service.store.meta(key).unwrap(), None, "{key}");
        }
    }

    #[test]
    fn host_owner_claim_is_persisted_before_the_owner_token_hash_is_committed() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;

        let error = service
            .provision_host_owner("Host PINVOU", |_, _| Err("disk full".to_string()))
            .unwrap_err();
        assert_eq!(error, "disk full");
        assert!(service.list_devices().unwrap().is_empty());

        let persisted = std::cell::RefCell::new(None);
        let owner = service
            .provision_host_owner("Host PINVOU", |device_id, token| {
                assert!(service.list_devices().unwrap().is_empty());
                persisted.replace(Some((device_id.to_string(), token.to_string())));
                Ok(())
            })
            .unwrap()
            .unwrap();
        let (device_id, token) = persisted.into_inner().unwrap();
        assert_eq!(owner.id, device_id);
        assert_eq!(owner.scope, AccessScope::Owner);
        assert_eq!(service.authorize(&token).unwrap().unwrap().id, device_id);
    }

    #[test]
    fn host_owner_recovery_rotates_only_after_the_new_claim_is_persisted() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let original_claim = std::cell::RefCell::new(None);
        let original = service
            .provision_host_owner("Host PINVOU", |device_id, token| {
                original_claim.replace(Some((device_id.to_string(), token.to_string())));
                Ok(())
            })
            .unwrap()
            .unwrap();
        let (_, original_token) = original_claim.into_inner().unwrap();

        let error = service
            .recover_host_owner("Host PINVOU", |_, _| Err("disk full".to_string()))
            .unwrap_err();
        assert_eq!(error, "disk full");
        assert_eq!(
            service.authorize(&original_token).unwrap().unwrap().id,
            original.id
        );

        let recovered_claim = std::cell::RefCell::new(None);
        let recovered = service
            .recover_host_owner("Host PINVOU", |device_id, token| {
                recovered_claim.replace(Some((device_id.to_string(), token.to_string())));
                Ok(())
            })
            .unwrap();
        let (recovered_id, recovered_token) = recovered_claim.into_inner().unwrap();
        assert_eq!(recovered.id, original.id);
        assert_eq!(recovered_id, original.id);
        assert!(service.authorize(&original_token).unwrap().is_none());
        assert_eq!(
            service.authorize(&recovered_token).unwrap().unwrap().id,
            original.id
        );
        assert_eq!(service.list_devices().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_upload_reuses_active_document_and_discards_staged_source() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let collection = service
            .create_collection(CreateCollectionRequest {
                name: "shared".to_string(),
                description: None,
            })
            .unwrap();

        let first = service
            .upload_document(collection.id, "first.md", b"same content".to_vec())
            .await
            .unwrap();
        let duplicate = service
            .upload_document(collection.id, "renamed.md", b"same content".to_vec())
            .await
            .unwrap();

        assert!(!first.already_exists);
        assert!(duplicate.already_exists);
        assert_eq!(duplicate.id, first.id);
        assert_eq!(duplicate.name, "first.md");
        assert_eq!(service.documents(collection.id, false).unwrap().len(), 1);
        assert_eq!(
            std::fs::read_dir(&service.documents_dir).unwrap().count(),
            1
        );
    }

    #[tokio::test]
    async fn permanent_delete_removes_managed_source_bytes() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let collection = service
            .create_collection(CreateCollectionRequest {
                name: "shared".to_string(),
                description: None,
            })
            .unwrap();
        let document = service
            .upload_document(collection.id, "removed.md", b"remove me".to_vec())
            .await
            .unwrap();
        let source = service
            .store
            .document(document.id, false)
            .unwrap()
            .map(|stored| service.documents_dir.join(stored.storage_path))
            .unwrap();
        assert!(source.is_file());

        service.trash_document(document.id).unwrap();
        service.permanently_delete_document(document.id).unwrap();

        assert!(!source.exists());
        assert!(service.store.document(document.id, true).unwrap().is_none());
    }

    #[tokio::test]
    async fn search_parallelism_is_bounded() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let capacity = search_parallelism();
        let mut permits = Vec::new();
        for _ in 0..capacity {
            permits.push(service.acquire_search_slot().await.unwrap());
        }
        assert_eq!(service.search_slots.available_permits(), 0);
        drop(permits.pop());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), service.acquire_search_slot())
                .await
                .is_ok()
        );
    }
}
