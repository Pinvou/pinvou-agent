//! 远程知识库连接域：只持久化服务器元数据，设备令牌始终进入系统凭据库。

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use futures_util::future::join_all;
use parking_lot::RwLock;
use pinvou_knowledge::client::{
    ca_fingerprint, identity_code, new_join_credentials, normalize_private_user_endpoint,
    normalize_stored_endpoint, normalize_user_endpoint, parse_share, KnowledgeClient,
    NewJoinCredentials, RemoteKnowledgeProbe,
};
use pinvou_knowledge::model::{
    AccessScope, Collection, CreateCollectionRequest, DeviceGrant, Document, JoinRequestRecord,
    JoinRequestStatus, ModelStatus, PairResponse, SearchHit, SearchRequest, ShareCreateRequest,
    ShareCreated, ShareRecord, SourceWindow, SourceWindowRequest, TrashedDocument,
    UpdateDeviceRequest,
};
use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::platform::credential_store::{
    CredentialReference, CredentialStore, SystemCredentialStore,
};

const CONNECTIONS_SCHEMA_VERSION: u32 = 3;
const MAX_FOLDER_IMPORT_FILES: usize = 10_000;
const SKIPPED_FOLDER_NAMES: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    ".cache",
    ".pinvou3",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFolderDiscovery {
    pub paths: Vec<String>,
    pub skipped: usize,
    pub limit_exceeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnection {
    pub server_id: String,
    pub name: String,
    pub endpoint: String,
    pub scope: AccessScope,
    pub device_id: String,
    #[serde(default)]
    pub tls_ca: String,
    /// `true` only for a pre-v2 HTTP FQDN retained during migration. Such a
    /// connection remains visible/removable, but its bearer token is never sent.
    #[serde(default)]
    pub legacy_insecure_http: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnectionStatus {
    #[serde(flatten)]
    pub connection: RemoteConnection,
    pub online: bool,
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingJoin {
    pub request_id: String,
    pub server_id: String,
    pub server_identity: String,
    pub server_name: String,
    #[serde(default)]
    pub tls_ca: String,
    pub endpoint: String,
    pub device_name: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinOutcome {
    pub status: JoinRequestStatus,
    pub request: JoinRequestRecord,
    pub connection: Option<RemoteConnection>,
    pub pending: Option<PendingJoin>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteKnowledgeIdentity {
    pub server_id: String,
    pub ca_fingerprint: String,
    pub identity_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingJoinSecrets {
    device_token: String,
    claim_secret: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionsFile {
    version: u32,
    connections: Vec<RemoteConnection>,
}

pub struct RemoteKnowledgeService {
    path: PathBuf,
    pending_path: PathBuf,
    connections: RwLock<Vec<RemoteConnection>>,
    pending_joins: RwLock<Vec<PendingJoin>>,
    credentials: SystemCredentialStore,
    pairing: tokio::sync::Mutex<()>,
    persistence: parking_lot::Mutex<()>,
}

pub fn discover_folder_files(roots: &[String]) -> Result<RemoteFolderDiscovery, String> {
    if roots.is_empty() {
        return Ok(RemoteFolderDiscovery {
            paths: Vec::new(),
            skipped: 0,
            limit_exceeded: false,
        });
    }
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut skipped = 0usize;
    let mut limit_exceeded = false;

    'roots: for root in roots {
        let root = Path::new(root);
        if !root.is_dir() {
            return Err(format!("不是可读取的文件夹：{}", root.display()));
        }
        let entries = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(keep_folder_entry);
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            if !entry.file_type().is_file() || hidden_name(&entry) {
                continue;
            }
            let path = entry.path();
            let Some(path_text) = path.to_str() else {
                skipped += 1;
                continue;
            };
            let identity = crate::platform::os::filesystem_path_identity_key(path_text);
            if !seen.insert(identity) {
                continue;
            }
            if !pinvou_knowledge::is_supported_document_path(path) {
                skipped += 1;
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            if metadata.len() == 0 || metadata.len() > pinvou_knowledge::MAX_UPLOAD_BYTES as u64 {
                skipped += 1;
                continue;
            }
            if paths.len() == MAX_FOLDER_IMPORT_FILES {
                limit_exceeded = true;
                break 'roots;
            }
            paths.push(path_text.to_string());
        }
    }
    paths.sort_by_key(|path| crate::platform::os::filesystem_path_identity_key(path));
    Ok(RemoteFolderDiscovery {
        paths,
        skipped,
        limit_exceeded,
    })
}

fn keep_folder_entry(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !name.starts_with('.') && !SKIPPED_FOLDER_NAMES.contains(&name.as_ref())
}

fn hidden_name(entry: &DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

impl RemoteKnowledgeService {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let connections = load_connections(&path)?;
        let pending_path = path.with_file_name("pending-shared-knowledge-joins.json");
        let pending_joins = load_pending_joins(&pending_path)?;
        Ok(Self {
            path,
            pending_path,
            connections: RwLock::new(connections),
            pending_joins: RwLock::new(pending_joins),
            credentials: SystemCredentialStore::new(),
            pairing: tokio::sync::Mutex::new(()),
            persistence: parking_lot::Mutex::new(()),
        })
    }

    pub fn default_path() -> PathBuf {
        crate::platform::paths::pinvou3_home()
            .join("knowledge")
            .join("remote-connections.json")
    }

    pub fn configured_connections(&self) -> Vec<RemoteConnection> {
        self.connections.read().clone()
    }

    pub fn has_connections(&self) -> bool {
        !self.connections.read().is_empty()
    }

    pub fn shared_backup_identity(&self) -> Result<Option<String>, String> {
        self.credentials
            .get(&CredentialReference::for_shared_knowledge_backup())
            .map_err(|error| error.user_message())
    }

    pub fn set_shared_backup_identity(&self, value: &str) -> Result<(), String> {
        self.credentials
            .set(&CredentialReference::for_shared_knowledge_backup(), value)
            .map_err(|error| error.user_message())
    }

    pub async fn statuses(&self) -> Result<Vec<RemoteConnectionStatus>, String> {
        let connections = self.configured_connections();
        // Probe independent servers concurrently. A dead connection should cost one
        // timeout window, not one timeout window per configured server.
        let probed = join_all(
            connections
                .into_iter()
                .map(|connection| self.status_for_connection(connection)),
        )
        .await;
        let mut statuses = Vec::with_capacity(probed.len());
        let mut scope_updates = Vec::new();
        for (status, scope_update) in probed {
            statuses.push(status);
            if let Some(update) = scope_update {
                scope_updates.push(update);
            }
        }
        self.apply_scope_updates(&scope_updates)?;
        Ok(statuses)
    }

    async fn status_for_connection(
        &self,
        mut connection: RemoteConnection,
    ) -> (
        RemoteConnectionStatus,
        Option<(String, String, AccessScope)>,
    ) {
        let client = match self.client_for(&connection.server_id) {
            Ok(client) => client,
            Err(error) => {
                return (
                    RemoteConnectionStatus {
                        connection,
                        online: false,
                        ready: false,
                        error: Some(redact_error(&error)),
                    },
                    None,
                )
            }
        };
        let info = match client.health().await {
            Ok(info) => info,
            Err(error) => {
                return (
                    RemoteConnectionStatus {
                        connection,
                        online: false,
                        ready: false,
                        error: Some(redact_error(&error)),
                    },
                    None,
                )
            }
        };
        match client.access().await {
            Ok(Some(grant)) if grant.id != connection.device_id => (
                RemoteConnectionStatus {
                    connection,
                    online: true,
                    ready: false,
                    error: Some("服务器返回的设备授权与本地连接不匹配".to_string()),
                },
                None,
            ),
            Ok(Some(grant)) => {
                let scope_update = (grant.scope != connection.scope).then(|| {
                    connection.scope = grant.scope;
                    (
                        connection.server_id.clone(),
                        connection.device_id.clone(),
                        connection.scope,
                    )
                });
                (
                    RemoteConnectionStatus {
                        ready: info.ready,
                        connection,
                        online: true,
                        error: None,
                    },
                    scope_update,
                )
            }
            Ok(None) => (
                RemoteConnectionStatus {
                    ready: info.ready,
                    connection,
                    online: true,
                    error: None,
                },
                None,
            ),
            Err(error) => (
                RemoteConnectionStatus {
                    connection,
                    online: true,
                    ready: false,
                    error: Some(redact_error(&error)),
                },
                None,
            ),
        }
    }

    pub async fn register_local_owner(
        &self,
        endpoint: &str,
        token: &str,
    ) -> Result<RemoteConnection, String> {
        let _pairing = self.pairing.lock().await;
        let endpoint = normalize_user_endpoint(endpoint)?;
        let server = KnowledgeClient::bootstrap_identity(&endpoint).await?;
        let client =
            KnowledgeClient::new_pinned(&endpoint, token, &server.tls_ca, &server.server_id)?;
        let grant = client
            .access()
            .await?
            .ok_or_else(|| "服务端不支持所有者凭据校验".to_string())?;
        if !grant.scope.is_owner() {
            return Err("服务端未授予本机所有者权限".to_string());
        }
        self.persist_local_owner_connection(endpoint, server, grant, token)
    }

    fn persist_local_owner_connection(
        &self,
        endpoint: String,
        server: pinvou_knowledge::model::ServerInfo,
        grant: DeviceGrant,
        token: &str,
    ) -> Result<RemoteConnection, String> {
        let connection = RemoteConnection {
            server_id: server.server_id,
            name: server.name,
            endpoint,
            scope: grant.scope,
            device_id: grant.id,
            tls_ca: server.tls_ca,
            legacy_insecure_http: false,
        };
        let _persistence = self.persistence.lock();
        let reference = credential_reference(&connection.server_id);
        let previous_token = self
            .credentials
            .get(&reference)
            .map_err(|error| error.user_message())?;
        self.credentials
            .set(&reference, token)
            .map_err(|error| error.user_message())?;

        let mut next = self.configured_connections();
        if let Some(slot) = next
            .iter_mut()
            .find(|item| item.server_id == connection.server_id)
        {
            *slot = connection.clone();
        } else {
            next.push(connection.clone());
        }
        next.sort_by(|left, right| left.name.cmp(&right.name));
        if let Err(error) = save_connections(&self.path, &next) {
            let rollback = if let Some(previous_token) = previous_token {
                self.credentials.set(&reference, &previous_token)
            } else {
                self.credentials.delete(&reference)
            };
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => format!(
                    "保存本机所有者连接失败，且凭据回滚失败：{error}；{}",
                    rollback.user_message()
                ),
            });
        }
        *self.connections.write() = next;
        Ok(connection)
    }

    /// Rebinds an already authorized local owner after the bundled host moves
    /// from a legacy transport or rotates its leaf certificate. The server CA
    /// and identity are verified before any connection metadata is replaced.
    pub async fn rebind_local_owner(&self, endpoint: &str) -> Result<RemoteConnection, String> {
        let _pairing = self.pairing.lock().await;
        let endpoint = normalize_user_endpoint(endpoint)?;
        let server = KnowledgeClient::bootstrap_identity(&endpoint).await?;
        let existing = self
            .configured_connections()
            .into_iter()
            .find(|item| item.server_id == server.server_id && item.scope.is_owner())
            .ok_or_else(|| "本机共享知识库的所有者连接不存在".to_string())?;
        let reference = credential_reference(&server.server_id);
        let token = self
            .credentials
            .get(&reference)
            .map_err(|error| error.user_message())?
            .ok_or_else(|| "本机共享知识库的所有者凭据不存在".to_string())?;
        let grant =
            KnowledgeClient::new_pinned(&endpoint, &token, &server.tls_ca, &server.server_id)?
                .access()
                .await?
                .ok_or_else(|| "服务端不支持所有者凭据校验".to_string())?;
        if !grant.scope.is_owner() || grant.id != existing.device_id {
            return Err("本机共享知识库的所有者凭据与服务身份不一致".to_string());
        }

        let connection = RemoteConnection {
            server_id: server.server_id.clone(),
            name: server.name,
            endpoint,
            scope: grant.scope,
            device_id: grant.id,
            tls_ca: server.tls_ca,
            legacy_insecure_http: false,
        };
        let _persistence = self.persistence.lock();
        let mut next = self.configured_connections();
        let slot = next
            .iter_mut()
            .find(|item| item.server_id == server.server_id)
            .ok_or_else(|| "本机共享知识库连接已发生变化，请重试".to_string())?;
        *slot = connection.clone();
        next.sort_by(|left, right| left.name.cmp(&right.name));
        save_connections(&self.path, &next)?;
        *self.connections.write() = next;
        Ok(connection)
    }

    pub fn pending_joins(&self) -> Vec<PendingJoin> {
        self.pending_joins.read().clone()
    }

    pub async fn request_join(
        &self,
        source: &str,
        device_name: &str,
    ) -> Result<JoinOutcome, String> {
        let _pairing = self.pairing.lock().await;
        let source = source.trim();
        let device_name = device_name.trim();
        if source.is_empty() || device_name.is_empty() {
            return Err("请输入共享链接或服务器地址，并填写姓名".to_string());
        }
        let share = parse_share(source).map_err(|_| {
            "首次加入必须使用所有者生成的共享链接，以安全验证服务器身份".to_string()
        })?;
        let endpoints = share.endpoints;
        let tls_ca = share.tls_ca;
        let share_secret = Some(share.secret);
        let expected_server = Some(share.server_id);
        let expected_identity = Some(share.identity);
        // Reject a second address for an already connected server before the
        // server creates a pending join request. The post-response check below
        // remains as a race guard, but must not be the normal duplicate path.
        if let Some(server_id) = expected_server.as_deref() {
            self.ensure_server_id_not_connected(server_id)?;
        }
        let credentials = new_join_credentials();
        let mut failures = Vec::new();
        for endpoint in endpoints {
            self.ensure_endpoint_not_connected(&endpoint)?;
            match KnowledgeClient::request_join(
                &endpoint,
                &tls_ca,
                expected_server.as_deref().unwrap_or_default(),
                device_name,
                share_secret.as_deref(),
                &credentials,
            )
            .await
            {
                Ok(receipt) => {
                    if expected_server
                        .as_deref()
                        .is_some_and(|expected| expected != receipt.server.server_id)
                        || expected_identity
                            .as_deref()
                            .is_some_and(|expected| expected != receipt.server.identity)
                    {
                        let _ = KnowledgeClient::cancel_join_request(
                            &endpoint,
                            &tls_ca,
                            expected_server.as_deref().unwrap_or_default(),
                            &receipt.request.id,
                            &credentials.claim_secret,
                        )
                        .await;
                        failures.push(format!("{endpoint}: 服务器身份与共享链接不一致"));
                        continue;
                    }
                    self.ensure_server_id_not_connected(&receipt.server.server_id)?;
                    return self
                        .persist_join_outcome(
                            endpoint,
                            tls_ca.clone(),
                            device_name,
                            credentials,
                            receipt,
                        )
                        .await;
                }
                Err(error) => failures.push(format!("{endpoint}: {error}")),
            }
        }
        Err(format!("无法连接共享知识库：{}", failures.join("；")))
    }

    /// Creates an owner-approved join request only after the caller has shown
    /// and explicitly confirmed the stable CA identity obtained by a private
    /// network probe.
    pub async fn request_join_confirmed(
        &self,
        probe: RemoteKnowledgeProbe,
        device_name: &str,
        confirmed_ca_fingerprint: &str,
        confirmed_identity_code: &str,
    ) -> Result<JoinOutcome, String> {
        let _pairing = self.pairing.lock().await;
        let device_name = device_name.trim();
        if device_name.is_empty() {
            return Err("请填写设备名称".to_string());
        }
        let (endpoint, network_kind) = normalize_private_user_endpoint(&probe.endpoint)?;
        let actual_fingerprint = ca_fingerprint(&probe.tls_ca)?;
        let actual_code = identity_code(&probe.tls_ca)?;
        if probe.ca_fingerprint != actual_fingerprint
            || probe.identity_code != actual_code
            || confirmed_ca_fingerprint != actual_fingerprint
            || confirmed_identity_code != actual_code
            || probe.network_kind != network_kind
        {
            return Err("共享知识库身份确认信息不一致，请重新探测并核对".to_string());
        }
        self.ensure_endpoint_not_connected(&endpoint)?;
        self.ensure_server_id_not_connected(&probe.server_id)?;
        let verified = KnowledgeClient::new_pinned(&endpoint, "", &probe.tls_ca, &probe.server_id)?
            .health()
            .await?;
        if verified.server_id != probe.server_id
            || verified.identity != probe.server_identity
            || verified.name != probe.server_name
            || verified.protocol_version != probe.protocol_version
            || verified.tls_ca != probe.tls_ca
        {
            return Err("共享知识库身份已变化，请重新探测并核对".to_string());
        }
        let credentials = new_join_credentials();
        let receipt = KnowledgeClient::request_join(
            &endpoint,
            &probe.tls_ca,
            &probe.server_id,
            device_name,
            None,
            &credentials,
        )
        .await?;
        if receipt.server.server_id != probe.server_id
            || receipt.server.identity != probe.server_identity
            || receipt.server.tls_ca != probe.tls_ca
        {
            let _ = KnowledgeClient::cancel_join_request(
                &endpoint,
                &probe.tls_ca,
                &probe.server_id,
                &receipt.request.id,
                &credentials.claim_secret,
            )
            .await;
            return Err("共享知识库在创建加入申请时身份发生变化".to_string());
        }
        self.ensure_server_id_not_connected(&receipt.server.server_id)?;
        self.persist_join_outcome(endpoint, probe.tls_ca, device_name, credentials, receipt)
            .await
    }

    /// Returns only public CA-derived identity material for an existing local
    /// connection. No device token, join secret, or private key is exposed.
    pub fn connection_identity(&self, server_id: &str) -> Result<RemoteKnowledgeIdentity, String> {
        let connection = self.connection(server_id)?;
        if connection.tls_ca.trim().is_empty() {
            return Err("该连接没有可核对的稳定 CA 身份".to_string());
        }
        Ok(RemoteKnowledgeIdentity {
            server_id: connection.server_id,
            ca_fingerprint: ca_fingerprint(&connection.tls_ca)?,
            identity_code: identity_code(&connection.tls_ca)?,
        })
    }

    pub async fn refresh_join(&self, request_id: &str) -> Result<JoinOutcome, String> {
        let pending = self
            .pending_joins()
            .into_iter()
            .find(|item| item.request_id == request_id)
            .ok_or_else(|| "加入申请不存在".to_string())?;
        let secrets = self.pending_join_secrets(request_id)?;
        let receipt = KnowledgeClient::join_request_status(
            &pending.endpoint,
            &pending.tls_ca,
            &pending.server_id,
            request_id,
            &secrets.claim_secret,
        )
        .await?;
        if receipt.server.server_id != pending.server_id
            || receipt.server.identity != pending.server_identity
        {
            return Err("服务器身份已变化，已停止处理该加入申请".to_string());
        }
        if receipt.request.status == JoinRequestStatus::Approved {
            return self
                .finish_approved_join(pending, secrets.device_token, receipt.request)
                .await;
        }
        if receipt.request.status != JoinRequestStatus::Pending {
            self.remove_pending_join(request_id)?;
        }
        let status = receipt.request.status;
        Ok(JoinOutcome {
            status,
            request: receipt.request,
            connection: None,
            pending: (status == JoinRequestStatus::Pending).then_some(pending),
        })
    }

    pub async fn cancel_join(&self, request_id: &str) -> Result<JoinRequestRecord, String> {
        let pending = self
            .pending_joins()
            .into_iter()
            .find(|item| item.request_id == request_id)
            .ok_or_else(|| "加入申请不存在".to_string())?;
        let secrets = self.pending_join_secrets(request_id)?;
        let remote = KnowledgeClient::cancel_join_request(
            &pending.endpoint,
            &pending.tls_ca,
            &pending.server_id,
            request_id,
            &secrets.claim_secret,
        )
        .await;
        self.remove_pending_join(request_id)?;
        match remote {
            Ok(request) => Ok(request),
            Err(_) => Ok(JoinRequestRecord {
                id: pending.request_id,
                device_name: pending.device_name,
                status: JoinRequestStatus::Cancelled,
                scope: None,
                share_id: None,
                device_id: None,
                created_at: pending.created_at,
                expires_at: pending.expires_at,
                resolved_at: Some(chrono::Utc::now().timestamp()),
            }),
        }
    }

    async fn persist_join_outcome(
        &self,
        endpoint: String,
        tls_ca: String,
        device_name: &str,
        credentials: NewJoinCredentials,
        receipt: pinvou_knowledge::model::JoinRequestReceipt,
    ) -> Result<JoinOutcome, String> {
        if receipt.request.status == JoinRequestStatus::Approved {
            let pending = PendingJoin {
                request_id: receipt.request.id.clone(),
                server_id: receipt.server.server_id.clone(),
                server_identity: receipt.server.identity.clone(),
                server_name: receipt.server.name.clone(),
                tls_ca: tls_ca.clone(),
                endpoint,
                device_name: device_name.to_string(),
                created_at: receipt.request.created_at,
                expires_at: receipt.request.expires_at,
            };
            return self
                .finish_approved_join(pending, credentials.device_token, receipt.request)
                .await;
        }
        let pending = PendingJoin {
            request_id: receipt.request.id.clone(),
            server_id: receipt.server.server_id,
            server_identity: receipt.server.identity,
            server_name: receipt.server.name,
            tls_ca,
            endpoint,
            device_name: device_name.to_string(),
            created_at: receipt.request.created_at,
            expires_at: receipt.request.expires_at,
        };
        let secret = serde_json::to_string(&PendingJoinSecrets {
            device_token: credentials.device_token,
            claim_secret: credentials.claim_secret,
        })
        .map_err(|error| error.to_string())?;
        let reference = pending_join_credential_reference(&pending.request_id);
        self.credentials
            .set(&reference, &secret)
            .map_err(|error| error.user_message())?;
        if let Err(error) = self.save_pending_join(pending.clone()) {
            let _ = self.credentials.delete(&reference);
            return Err(error);
        }
        Ok(JoinOutcome {
            status: receipt.request.status,
            request: receipt.request,
            connection: None,
            pending: Some(pending),
        })
    }

    async fn finish_approved_join(
        &self,
        pending: PendingJoin,
        device_token: String,
        request: JoinRequestRecord,
    ) -> Result<JoinOutcome, String> {
        let scope = request
            .scope
            .ok_or_else(|| "服务器未返回成员权限".to_string())?;
        let device_id = request
            .device_id
            .clone()
            .ok_or_else(|| "服务器未返回设备标识".to_string())?;
        let connection = self.persist_paired_connection(
            pending.endpoint.clone(),
            PairResponse {
                server: pinvou_knowledge::model::ServerInfo {
                    server_id: pending.server_id.clone(),
                    identity: pending.server_identity.clone(),
                    name: pending.server_name.clone(),
                    version: String::new(),
                    protocol_version: 2,
                    tls_ca: pending.tls_ca.clone(),
                    initialized: true,
                    ready: false,
                    model: String::new(),
                },
                token: device_token,
                scope,
                device_id,
            },
        )?;
        if self
            .pending_joins()
            .iter()
            .any(|item| item.request_id == pending.request_id)
        {
            self.remove_pending_join(&pending.request_id)?;
        }
        Ok(JoinOutcome {
            status: request.status,
            request,
            connection: Some(connection),
            pending: None,
        })
    }

    fn save_pending_join(&self, pending: PendingJoin) -> Result<(), String> {
        let _persistence = self.persistence.lock();
        let mut next = self.pending_joins();
        next.retain(|item| item.request_id != pending.request_id);
        next.push(pending);
        next.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        save_pending_joins(&self.pending_path, &next)?;
        *self.pending_joins.write() = next;
        Ok(())
    }

    fn pending_join_secrets(&self, request_id: &str) -> Result<PendingJoinSecrets, String> {
        let secret = self
            .credentials
            .get(&pending_join_credential_reference(request_id))
            .map_err(|error| error.user_message())?
            .ok_or_else(|| "加入申请的本机凭据缺失，请取消后重新申请".to_string())?;
        serde_json::from_str(&secret).map_err(|_| "加入申请的本机凭据损坏".to_string())
    }

    fn remove_pending_join(&self, request_id: &str) -> Result<(), String> {
        let _persistence = self.persistence.lock();
        let mut next = self.pending_joins();
        next.retain(|item| item.request_id != request_id);
        save_pending_joins(&self.pending_path, &next)?;
        self.credentials
            .delete(&pending_join_credential_reference(request_id))
            .map_err(|error| error.user_message())?;
        *self.pending_joins.write() = next;
        Ok(())
    }

    pub async fn create_share(
        &self,
        server_id: &str,
        endpoints: Vec<String>,
        auto_approve_read: bool,
    ) -> Result<ShareCreated, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?
            .create_share(&ShareCreateRequest {
                endpoints,
                auto_approve_read,
                expires_in_hours: None,
            })
            .await
    }

    pub async fn shares(&self, server_id: &str) -> Result<Vec<ShareRecord>, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.shares().await
    }

    pub async fn stop_share(&self, server_id: &str, share_id: &str) -> Result<ShareRecord, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.stop_share(share_id).await
    }

    pub async fn join_requests(&self, server_id: &str) -> Result<Vec<JoinRequestRecord>, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.join_requests().await
    }

    pub async fn approve_join_request(
        &self,
        server_id: &str,
        request_id: &str,
        scope: AccessScope,
    ) -> Result<JoinRequestRecord, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?
            .approve_join_request(request_id, scope)
            .await
    }

    pub async fn reject_join_request(
        &self,
        server_id: &str,
        request_id: &str,
    ) -> Result<JoinRequestRecord, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?
            .reject_join_request(request_id)
            .await
    }

    pub async fn model_status(&self, server_id: &str) -> Result<ModelStatus, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.model_status().await
    }

    pub async fn download_model(&self, server_id: &str) -> Result<ModelStatus, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.download_model().await
    }

    pub async fn devices(&self, server_id: &str) -> Result<Vec<DeviceGrant>, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.devices().await
    }

    pub async fn update_device(
        &self,
        server_id: &str,
        device_id: &str,
        request: UpdateDeviceRequest,
    ) -> Result<DeviceGrant, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?
            .update_device(device_id, &request)
            .await
    }

    pub async fn remove_device(&self, server_id: &str, device_id: &str) -> Result<(), String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.remove_device(device_id).await
    }

    pub async fn trashed_collections(&self, server_id: &str) -> Result<Vec<Collection>, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.trashed_collections().await
    }

    pub async fn trashed_documents(&self, server_id: &str) -> Result<Vec<TrashedDocument>, String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?.trashed_documents().await
    }

    pub async fn permanently_delete_collection(
        &self,
        server_id: &str,
        id: i64,
    ) -> Result<(), String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?
            .permanently_delete_collection(id)
            .await
    }

    pub async fn permanently_delete_document(
        &self,
        server_id: &str,
        id: i64,
    ) -> Result<(), String> {
        self.require_owner(server_id)?;
        self.client_for(server_id)?
            .permanently_delete_document(id)
            .await
    }

    fn ensure_endpoint_not_connected(&self, endpoint: &str) -> Result<(), String> {
        if self
            .configured_connections()
            .iter()
            .any(|connection| connection.endpoint == endpoint)
        {
            return Err(
                "这台知识库服务器已经连接，无需重复添加；如需重新授权，请先断开现有连接"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn ensure_server_id_not_connected(&self, server_id: &str) -> Result<(), String> {
        if self
            .configured_connections()
            .iter()
            .any(|connection| connection.server_id == server_id)
        {
            return Err(
                "这台知识库服务器已通过另一个地址连接；为避免现有会话被静默改指，请先断开旧连接后再添加"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn persist_paired_connection(
        &self,
        endpoint: String,
        paired: PairResponse,
    ) -> Result<RemoteConnection, String> {
        let _persistence = self.persistence.lock();
        // The unauthenticated identity probe prevents this in normal operation.
        // Recheck the signed pairing response before touching credentials so a
        // server whose identity changed between both requests cannot redirect
        // existing mounts to a different endpoint.
        self.ensure_server_id_not_connected(&paired.server.server_id)?;
        let connection = RemoteConnection {
            server_id: paired.server.server_id.clone(),
            name: paired.server.name,
            endpoint,
            scope: paired.scope,
            device_id: paired.device_id,
            tls_ca: paired.server.tls_ca,
            legacy_insecure_http: false,
        };
        let reference = credential_reference(&connection.server_id);
        let previous_token = self
            .credentials
            .get(&reference)
            .map_err(|error| error.user_message())?;
        self.credentials
            .set(&reference, &paired.token)
            .map_err(|error| error.user_message())?;

        let mut next = self.configured_connections();
        next.push(connection.clone());
        next.sort_by(|left, right| left.name.cmp(&right.name));
        if let Err(error) = save_connections(&self.path, &next) {
            let rollback = if let Some(previous_token) = previous_token {
                self.credentials.set(&reference, &previous_token)
            } else {
                self.credentials.delete(&reference)
            };
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => format!(
                    "保存远程知识库连接失败，且凭据回滚失败：{error}；{}",
                    rollback.user_message()
                ),
            });
        }
        *self.connections.write() = next;
        Ok(connection)
    }

    pub fn remove_connection(&self, server_id: &str) -> Result<(), String> {
        let _persistence = self.persistence.lock();
        let previous = self.configured_connections();
        let mut next = previous.clone();
        let before = next.len();
        next.retain(|item| item.server_id != server_id);
        if before == next.len() {
            return Err("远程知识库连接不存在".to_string());
        }
        save_connections(&self.path, &next)?;
        if let Err(error) = self.credentials.delete(&credential_reference(server_id)) {
            let rollback = save_connections(&self.path, &previous);
            return Err(match rollback {
                Ok(()) => error.user_message(),
                Err(rollback) => format!(
                    "删除远程知识库凭据失败，且连接元数据回滚失败：{}；{}",
                    error.user_message(),
                    rollback
                ),
            });
        }
        *self.connections.write() = next;
        Ok(())
    }

    pub fn connection(&self, server_id: &str) -> Result<RemoteConnection, String> {
        self.connections
            .read()
            .iter()
            .find(|item| item.server_id == server_id)
            .cloned()
            .ok_or_else(|| "远程知识库连接不存在".to_string())
    }

    pub fn client_for(&self, server_id: &str) -> Result<KnowledgeClient, String> {
        let connection = self.connection(server_id)?;
        if connection.legacy_insecure_http {
            return Err(
                "该连接由旧版保留，使用无法验证为内网的 HTTP 域名；为避免泄露设备凭据已暂停访问。请让管理员提供 HTTPS 地址，或断开后使用局域网 IP / Tailscale 地址重新连接"
                    .to_string(),
            );
        }
        let token = self
            .credentials
            .get(&credential_reference(server_id))
            .map_err(|error| error.user_message())?
            .ok_or_else(|| "远程知识库凭据缺失，请删除连接后重新接受邀请".to_string())?;
        if connection.tls_ca.is_empty() {
            KnowledgeClient::new(connection.endpoint, token)
        } else {
            KnowledgeClient::new_pinned(
                connection.endpoint,
                token,
                &connection.tls_ca,
                &connection.server_id,
            )
        }
    }

    pub async fn collections(
        &self,
        server_id: &str,
        include_deleted: bool,
    ) -> Result<Vec<Collection>, String> {
        self.client_for(server_id)?
            .collections(include_deleted)
            .await
    }

    pub async fn create_collection(
        &self,
        server_id: &str,
        name: String,
        description: Option<String>,
    ) -> Result<Collection, String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?
            .create_collection(&CreateCollectionRequest { name, description })
            .await
    }

    pub async fn update_collection(
        &self,
        server_id: &str,
        id: i64,
        name: String,
        description: Option<String>,
    ) -> Result<Collection, String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?
            .update_collection(id, &CreateCollectionRequest { name, description })
            .await
    }

    pub async fn delete_collection(&self, server_id: &str, id: i64) -> Result<(), String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?.delete_collection(id).await
    }

    pub async fn restore_collection(&self, server_id: &str, id: i64) -> Result<(), String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?.restore_collection(id).await
    }

    pub async fn documents(
        &self,
        server_id: &str,
        collection_id: i64,
        include_deleted: bool,
    ) -> Result<Vec<Document>, String> {
        self.client_for(server_id)?
            .documents(collection_id, include_deleted)
            .await
    }

    pub async fn documents_page(
        &self,
        server_id: &str,
        collection_id: i64,
        include_deleted: bool,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<Document>, String> {
        self.client_for(server_id)?
            .documents_page(collection_id, include_deleted, limit, offset)
            .await
    }

    pub async fn document_statuses(
        &self,
        server_id: &str,
        document_ids: &[i64],
    ) -> Result<Vec<Document>, String> {
        self.client_for(server_id)?
            .document_statuses(document_ids)
            .await
    }

    pub async fn upload_paths(
        &self,
        server_id: &str,
        collection_id: i64,
        paths: Vec<String>,
    ) -> Result<Vec<Document>, String> {
        self.require_manage(server_id)?;
        if paths.is_empty() {
            return Err("请选择至少一个文件".to_string());
        }
        let client = self.client_for(server_id)?;
        let mut uploaded = Vec::with_capacity(paths.len());
        for path in paths {
            uploaded.push(client.upload_path(collection_id, Path::new(&path)).await?);
        }
        Ok(uploaded)
    }

    pub async fn replace_document(
        &self,
        server_id: &str,
        document_id: i64,
        path: String,
    ) -> Result<Document, String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?
            .replace_document_path(document_id, Path::new(&path))
            .await
    }

    pub async fn delete_document(&self, server_id: &str, id: i64) -> Result<(), String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?.delete_document(id).await
    }

    pub async fn restore_document(&self, server_id: &str, id: i64) -> Result<(), String> {
        self.require_manage(server_id)?;
        self.client_for(server_id)?.restore_document(id).await
    }

    pub async fn download_document(
        &self,
        server_id: &str,
        id: i64,
        destination: String,
    ) -> Result<String, String> {
        let (filename, bytes) = self.client_for(server_id)?.download_document(id).await?;
        let destination = PathBuf::from(destination);
        let target = write_download_without_overwrite(&destination, &filename, &bytes)?;
        Ok(target.to_string_lossy().into_owned())
    }

    pub async fn search(
        &self,
        server_id: &str,
        collection_ids: Vec<i64>,
        query: String,
        limit: usize,
    ) -> Result<Vec<SearchHit>, String> {
        self.client_for(server_id)?
            .search(&SearchRequest {
                collection_ids,
                query,
                limit,
            })
            .await
    }

    pub async fn source_window(
        &self,
        server_id: &str,
        request: SourceWindowRequest,
    ) -> Result<SourceWindow, String> {
        self.client_for(server_id)?.source_window(&request).await
    }

    fn require_manage(&self, server_id: &str) -> Result<(), String> {
        if self.connection(server_id)?.scope.can_manage() {
            Ok(())
        } else {
            Err("该设备只有只读权限".to_string())
        }
    }

    fn require_owner(&self, server_id: &str) -> Result<(), String> {
        if self.connection(server_id)?.scope.is_owner() {
            Ok(())
        } else {
            Err("该操作仅限共享知识库所有者".to_string())
        }
    }

    fn apply_scope_updates(&self, updates: &[(String, String, AccessScope)]) -> Result<(), String> {
        if updates.is_empty() {
            return Ok(());
        }
        let _persistence = self.persistence.lock();
        let mut next = self.configured_connections();
        let mut changed = false;
        for connection in &mut next {
            if let Some((_, _, scope)) = updates.iter().find(|(server_id, device_id, _)| {
                server_id == &connection.server_id && device_id == &connection.device_id
            }) {
                if connection.scope != *scope {
                    connection.scope = *scope;
                    changed = true;
                }
            }
        }
        if changed {
            save_connections(&self.path, &next)?;
            *self.connections.write() = next;
        }
        Ok(())
    }
}

fn credential_reference(server_id: &str) -> CredentialReference {
    CredentialReference::for_remote_knowledge(server_id)
}

fn pending_join_credential_reference(request_id: &str) -> CredentialReference {
    CredentialReference::for_remote_knowledge_join(request_id)
}

fn load_pending_joins(path: &Path) -> Result<Vec<PendingJoin>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut joins: Vec<PendingJoin> =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    joins.retain(|item| {
        !item.request_id.trim().is_empty()
            && !item.server_id.trim().is_empty()
            && !item.endpoint.trim().is_empty()
    });
    joins.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(joins)
}

fn save_pending_joins(path: &Path, joins: &[PendingJoin]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(joins).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn load_connections(path: &Path) -> Result<Vec<RemoteConnection>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let file: ConnectionsFile =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if file.version > CONNECTIONS_SCHEMA_VERSION {
        return Err(format!(
            "远程知识库连接文件版本 {} 高于当前支持版本 {}",
            file.version, CONNECTIONS_SCHEMA_VERSION
        ));
    }
    let source_version = file.version;
    let mut migrated = source_version < CONNECTIONS_SCHEMA_VERSION;
    let mut seen = HashSet::new();
    let mut connections = Vec::with_capacity(file.connections.len());
    for mut connection in file.connections {
        if connection.server_id.trim().is_empty() || seen.contains(&connection.server_id) {
            migrated = true;
            continue;
        }
        let stored = match normalize_stored_endpoint(&connection.endpoint) {
            Ok(stored) => stored,
            Err(_) => {
                migrated = true;
                continue;
            }
        };
        if connection.endpoint != stored.endpoint
            || connection.legacy_insecure_http != stored.requires_secure_upgrade
        {
            migrated = true;
        }
        connection.endpoint = stored.endpoint;
        connection.legacy_insecure_http = stored.requires_secure_upgrade;
        seen.insert(connection.server_id.clone());
        connections.push(connection);
    }
    if migrated {
        save_connections(path, &connections)?;
    }
    Ok(connections)
}

fn save_connections(path: &Path, connections: &[RemoteConnection]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&ConnectionsFile {
        version: CONNECTIONS_SCHEMA_VERSION,
        connections: connections.to_vec(),
    })
    .map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let backup = path.with_extension("json.bak");
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if path.exists() {
        let _ = fs::remove_file(&backup);
        fs::rename(path, &backup).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(error.to_string());
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn redact_error(error: &str) -> String {
    crate::platform::credential_store::redact_secret(error)
}

fn write_download_without_overwrite(
    destination: &Path,
    filename: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    if !destination.is_dir() {
        return write_new_file(destination, bytes).map_err(|error| error.to_string());
    }

    let source_name = Path::new(filename);
    let stem = source_name
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("document");
    let extension = source_name.extension().and_then(|value| value.to_str());
    for suffix in 0..10_000 {
        let candidate_name = if suffix == 0 {
            filename.to_string()
        } else if let Some(extension) = extension {
            format!("{stem} ({suffix}).{extension}")
        } else {
            format!("{stem} ({suffix})")
        };
        let target = destination.join(candidate_name);
        match write_new_file(&target, bytes) {
            Ok(path) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("下载目录中同名文件过多，请更换保存目录".to_string())
}

fn write_new_file(target: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(target);
        return Err(error);
    }
    Ok(target.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_roundtrip_never_contains_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-connections.json");
        let connection = RemoteConnection {
            server_id: "srv-1".to_string(),
            name: "Cube".to_string(),
            endpoint: "https://100.64.0.1:3210".to_string(),
            scope: AccessScope::Read,
            device_id: "dev-1".to_string(),
            tls_ca: "test-ca".to_string(),
            legacy_insecure_http: false,
        };
        save_connections(&path, &[connection.clone()]).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("token"));
        assert_eq!(load_connections(&path).unwrap(), vec![connection]);
    }

    #[test]
    fn duplicate_server_identity_is_rejected_without_replacing_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-connections.json");
        let existing = RemoteConnection {
            server_id: "srv-existing".to_string(),
            name: "Company Knowledge".to_string(),
            endpoint: "https://100.64.0.10:3210".to_string(),
            scope: AccessScope::Read,
            device_id: "dev-existing".to_string(),
            tls_ca: "test-ca".to_string(),
            legacy_insecure_http: false,
        };
        save_connections(&path, std::slice::from_ref(&existing)).unwrap();
        let service = RemoteKnowledgeService::load(path.clone()).unwrap();

        let error = service
            .ensure_server_id_not_connected("srv-existing")
            .unwrap_err();
        assert!(error.contains("另一个地址"));
        assert!(service.ensure_server_id_not_connected("srv-other").is_ok());
        assert_eq!(load_connections(&path).unwrap(), vec![existing]);
    }

    #[tokio::test]
    async fn unconfirmed_private_probe_never_persists_join_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-connections.json");
        let service = RemoteKnowledgeService::load(path.clone()).unwrap();
        let probe = RemoteKnowledgeProbe {
            endpoint: "https://192.168.1.20:3210".to_string(),
            network_kind: pinvou_knowledge::client::PrivateEndpointKind::Lan,
            server_id: "server".to_string(),
            server_identity: "identity".to_string(),
            server_name: "Cube".to_string(),
            protocol_version: 2,
            tls_ca: "untrusted".to_string(),
            ca_fingerprint: "not-confirmed".to_string(),
            identity_code: "PINVOU-0000-0000-0000-0000".to_string(),
            ready: false,
        };

        assert!(service
            .request_join_confirmed(probe, "device", "different", "different")
            .await
            .is_err());
        assert!(service.configured_connections().is_empty());
        assert!(service.pending_joins().is_empty());
        assert!(!path.exists());
        assert!(!service.pending_path.exists());
    }

    #[test]
    fn legacy_http_fqdn_is_migrated_kept_visible_and_blocked_with_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-connections.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "connections": [{
                    "serverId": "srv-legacy",
                    "name": "Company Knowledge",
                    "endpoint": "http://knowledge.corp.example:3210/old/path?ignored=true",
                    "scope": "read",
                    "deviceId": "dev-legacy"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let service = RemoteKnowledgeService::load(path.clone()).unwrap();
        let connection = service.connection("srv-legacy").unwrap();
        assert_eq!(connection.endpoint, "http://knowledge.corp.example:3210");
        assert!(connection.legacy_insecure_http);

        let error = match service.client_for("srv-legacy") {
            Ok(_) => panic!("legacy HTTP FQDN must not create an authenticated client"),
            Err(error) => error,
        };
        assert!(error.contains("旧版保留"));
        assert!(error.contains("HTTPS"));

        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            persisted["version"].as_u64(),
            Some(CONNECTIONS_SCHEMA_VERSION as u64)
        );
        assert_eq!(
            persisted["connections"][0]["legacyInsecureHttp"].as_bool(),
            Some(true)
        );
        assert!(!path.with_extension("json.bak").exists());
        assert!(!fs::read_to_string(path).unwrap().contains("token"));
    }

    #[test]
    fn refreshed_scope_is_persisted_without_touching_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-connections.json");
        let connection = RemoteConnection {
            server_id: "srv-1".to_string(),
            name: "Cube".to_string(),
            endpoint: "https://100.64.0.1:3210".to_string(),
            scope: AccessScope::Read,
            device_id: "dev-1".to_string(),
            tls_ca: "test-ca".to_string(),
            legacy_insecure_http: false,
        };
        save_connections(&path, &[connection]).unwrap();
        let service = RemoteKnowledgeService::load(path.clone()).unwrap();

        service
            .apply_scope_updates(&[(
                "srv-1".to_string(),
                "dev-1".to_string(),
                AccessScope::Manage,
            )])
            .unwrap();

        assert_eq!(
            service.connection("srv-1").unwrap().scope,
            AccessScope::Manage
        );
        assert_eq!(
            load_connections(&path).unwrap()[0].scope,
            AccessScope::Manage
        );
    }

    #[test]
    fn stale_scope_refresh_does_not_modify_a_replaced_device() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-connections.json");
        let connection = RemoteConnection {
            server_id: "srv-1".to_string(),
            name: "Cube".to_string(),
            endpoint: "https://100.64.0.1:3210".to_string(),
            scope: AccessScope::Read,
            device_id: "dev-new".to_string(),
            tls_ca: "test-ca".to_string(),
            legacy_insecure_http: false,
        };
        save_connections(&path, &[connection]).unwrap();
        let service = RemoteKnowledgeService::load(path.clone()).unwrap();

        service
            .apply_scope_updates(&[(
                "srv-1".to_string(),
                "dev-old".to_string(),
                AccessScope::Manage,
            )])
            .unwrap();

        assert_eq!(
            service.connection("srv-1").unwrap().scope,
            AccessScope::Read
        );
        assert_eq!(load_connections(&path).unwrap()[0].scope, AccessScope::Read);
    }

    #[test]
    fn repeated_download_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("report.txt");
        fs::write(&original, b"local copy").unwrap();

        let saved =
            write_download_without_overwrite(dir.path(), "report.txt", b"remote copy").unwrap();

        assert_eq!(saved, dir.path().join("report (1).txt"));
        assert_eq!(fs::read(original).unwrap(), b"local copy");
        assert_eq!(fs::read(saved).unwrap(), b"remote copy");
    }

    #[test]
    fn folder_discovery_is_recursive_filtered_and_deduplicated() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("team").join("reports");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.path().join("notes.md"), b"notes").unwrap();
        fs::write(nested.join("report.pdf"), b"pdf").unwrap();
        fs::write(nested.join("archive.zip"), b"zip").unwrap();
        fs::write(nested.join("empty.txt"), b"").unwrap();
        let oversized = nested.join("oversized.pdf");
        fs::File::create(&oversized)
            .unwrap()
            .set_len(pinvou_knowledge::MAX_UPLOAD_BYTES as u64 + 1)
            .unwrap();
        let ignored = root.path().join(".git");
        fs::create_dir_all(&ignored).unwrap();
        fs::write(ignored.join("secret.md"), b"ignored").unwrap();

        let discovery = discover_folder_files(&[
            root.path().to_string_lossy().into_owned(),
            nested.to_string_lossy().into_owned(),
        ])
        .unwrap();
        let names: Vec<_> = discovery
            .paths
            .iter()
            .filter_map(|path| Path::new(path).file_name()?.to_str())
            .collect();

        assert_eq!(names, vec!["notes.md", "report.pdf"]);
        assert_eq!(discovery.skipped, 3);
        assert!(!discovery.limit_exceeded);
    }
}
