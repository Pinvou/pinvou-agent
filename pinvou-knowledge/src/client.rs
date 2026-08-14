use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use reqwest::{multipart, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::model::*;
use crate::MAX_UPLOAD_BYTES;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const PAIR_TIMEOUT: Duration = Duration::from_secs(15);
// Replacement remains atomic and waits for parsing plus embedding before the
// server commits it. External parsers may legitimately consume up to 120s, so
// this operation must not inherit the generic 90s request timeout and report an
// ambiguous failure while the server is still working.
const REPLACE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct KnowledgeClient {
    endpoint: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInvite {
    pub endpoint: String,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedShare {
    pub server_id: String,
    pub identity: String,
    pub tls_ca: String,
    pub endpoints: Vec<String>,
    pub secret: String,
}

#[derive(Clone)]
pub struct NewJoinCredentials {
    pub device_token: String,
    pub device_token_hash: String,
    pub claim_secret: String,
}

/// A syntactically valid endpoint read from an older on-disk connection file.
///
/// New connections must still pass [`normalize_endpoint`]. This separate result
/// lets callers retain a legacy HTTP FQDN long enough to show a migration error
/// without ever treating it as safe for a newly paired connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEndpoint {
    pub endpoint: String,
    pub requires_secure_upgrade: bool,
}

impl KnowledgeClient {
    pub fn new(endpoint: impl Into<String>, token: impl Into<String>) -> Result<Self, String> {
        crate::ensure_tls_crypto_provider();
        let endpoint = normalize_endpoint(&endpoint.into())?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            endpoint,
            token: token.into(),
            http,
        })
    }

    pub fn new_pinned(
        endpoint: impl Into<String>,
        token: impl Into<String>,
        tls_ca: &str,
    ) -> Result<Self, String> {
        crate::ensure_tls_crypto_provider();
        let endpoint = normalize_endpoint(&endpoint.into())?;
        if !endpoint.starts_with("https://") {
            return Err("共享知识库加密连接必须使用 HTTPS".to_string());
        }
        let certificate_pem = URL_SAFE_NO_PAD
            .decode(tls_ca.trim())
            .map_err(|_| "共享知识库加密身份无效".to_string())?;
        let certificate = reqwest::Certificate::from_pem(&certificate_pem)
            .map_err(|_| "共享知识库加密身份无效".to_string())?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(90))
            .tls_certs_only([certificate])
            // The private service CA is the stable identity. Leaf certificates
            // are intentionally short-lived and a host can be reached through
            // a DHCP address, MagicDNS name or another private alias that was
            // not known when the service booted. With native roots disabled,
            // accepting a hostname mismatch does not accept another server.
            .tls_danger_accept_invalid_hostnames(true)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            endpoint,
            token: token.into(),
            http,
        })
    }

    pub async fn local_health_untrusted(endpoint: &str) -> Result<ServerInfo, String> {
        crate::ensure_tls_crypto_provider();
        let endpoint = normalize_endpoint(endpoint)?;
        let url = url::Url::parse(&endpoint).map_err(|error| error.to_string())?;
        let host = url.host_str().unwrap_or_default();
        if host != "localhost"
            && host
                .parse::<IpAddr>()
                .ok()
                .is_none_or(|address| !address.is_loopback())
        {
            return Err("不受信任的健康检查仅限本机回环地址".to_string());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(PROBE_TIMEOUT)
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|error| error.to_string())?;
        let response = http
            .get(format!("{endpoint}/api/v1/info"))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        decode(response).await
    }

    pub async fn bootstrap_identity(endpoint: &str) -> Result<ServerInfo, String> {
        crate::ensure_tls_crypto_provider();
        let endpoint = normalize_endpoint(endpoint)?;
        if !endpoint.starts_with("https://") {
            return Err("共享知识库首次连接必须使用 HTTPS".to_string());
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(PROBE_TIMEOUT)
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|error| error.to_string())?;
        let info: ServerInfo = decode(
            http.get(format!("{endpoint}/api/v1/info"))
                .send()
                .await
                .map_err(|error| error.to_string())?,
        )
        .await?;
        let verified = Self::new_pinned(endpoint, "", &info.tls_ca)?
            .health()
            .await?;
        if verified.server_id != info.server_id
            || verified.identity != info.identity
            || verified.tls_ca != info.tls_ca
        {
            return Err("共享知识库在建立加密连接时身份发生变化".to_string());
        }
        Ok(info)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn health(&self) -> Result<ServerInfo, String> {
        let response = self
            .http
            .get(self.url("/api/v1/info"))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        decode(response).await
    }

    /// 返回当前设备在服务器上的实时授权。旧版服务器没有该接口时返回 `None`，
    /// 以便新客户端仍可使用配对时缓存的权限。
    pub async fn access(&self) -> Result<Option<DeviceGrant>, String> {
        let response = self
            .authorized(
                self.http
                    .get(self.url("/api/v1/access"))
                    .timeout(PROBE_TIMEOUT),
            )
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        decode(response).await.map(Some)
    }

    pub async fn pair(invite: &ParsedInvite, device_name: &str) -> Result<PairResponse, String> {
        let client = Self::new(&invite.endpoint, "")?;
        client
            .post_with_timeout(
                "/api/v1/pair/redeem",
                &PairRequest {
                    invite_secret: invite.secret.clone(),
                    device_name: device_name.to_string(),
                },
                PAIR_TIMEOUT,
            )
            .await
    }

    pub async fn request_join(
        endpoint: &str,
        tls_ca: &str,
        device_name: &str,
        share_secret: Option<&str>,
        credentials: &NewJoinCredentials,
    ) -> Result<JoinRequestReceipt, String> {
        let client = Self::new_pinned(endpoint, "", tls_ca)?;
        client
            .post_with_timeout(
                "/api/v2/join-requests",
                &JoinRequestCreate {
                    device_name: device_name.to_string(),
                    device_token_hash: credentials.device_token_hash.clone(),
                    claim_secret: credentials.claim_secret.clone(),
                    share_secret: share_secret.map(str::to_string),
                },
                PAIR_TIMEOUT,
            )
            .await
    }

    pub async fn join_request_status(
        endpoint: &str,
        tls_ca: &str,
        request_id: &str,
        claim_secret: &str,
    ) -> Result<JoinRequestReceipt, String> {
        let client = Self::new_pinned(endpoint, "", tls_ca)?;
        client
            .post_with_timeout(
                &format!("/api/v2/join-requests/{request_id}/status"),
                &JoinRequestClaim {
                    claim_secret: claim_secret.to_string(),
                },
                PAIR_TIMEOUT,
            )
            .await
    }

    pub async fn cancel_join_request(
        endpoint: &str,
        tls_ca: &str,
        request_id: &str,
        claim_secret: &str,
    ) -> Result<JoinRequestRecord, String> {
        let client = Self::new_pinned(endpoint, "", tls_ca)?;
        client
            .post_with_timeout(
                &format!("/api/v2/join-requests/{request_id}/cancel"),
                &JoinRequestClaim {
                    claim_secret: claim_secret.to_string(),
                },
                PAIR_TIMEOUT,
            )
            .await
    }

    pub async fn create_share(&self, request: &ShareCreateRequest) -> Result<ShareCreated, String> {
        self.post("/api/v2/owner/shares", request).await
    }

    pub async fn shares(&self) -> Result<Vec<ShareRecord>, String> {
        self.get("/api/v2/owner/shares").await
    }

    pub async fn stop_share(&self, share_id: &str) -> Result<ShareRecord, String> {
        self.send_json::<ShareRecord, serde_json::Value>(
            Method::DELETE,
            &format!("/api/v2/owner/shares/{share_id}"),
            None,
        )
        .await
    }

    pub async fn join_requests(&self) -> Result<Vec<JoinRequestRecord>, String> {
        self.get("/api/v2/owner/join-requests").await
    }

    pub async fn approve_join_request(
        &self,
        request_id: &str,
        scope: AccessScope,
    ) -> Result<JoinRequestRecord, String> {
        self.post(
            &format!("/api/v2/owner/join-requests/{request_id}/approve"),
            &ResolveJoinRequest { scope },
        )
        .await
    }

    pub async fn reject_join_request(&self, request_id: &str) -> Result<JoinRequestRecord, String> {
        self.send_json::<JoinRequestRecord, serde_json::Value>(
            Method::POST,
            &format!("/api/v2/owner/join-requests/{request_id}/reject"),
            None,
        )
        .await
    }

    pub async fn devices(&self) -> Result<Vec<DeviceGrant>, String> {
        self.get("/api/v2/owner/devices?limit=200&offset=0").await
    }

    pub async fn update_device(
        &self,
        device_id: &str,
        request: &UpdateDeviceRequest,
    ) -> Result<DeviceGrant, String> {
        self.send_json(
            Method::PATCH,
            &format!("/api/v2/owner/devices/{device_id}"),
            Some(request),
        )
        .await
    }

    pub async fn remove_device(&self, device_id: &str) -> Result<(), String> {
        self.send_empty(
            Method::DELETE,
            &format!("/api/v2/owner/devices/{device_id}"),
        )
        .await
    }

    pub async fn trashed_collections(&self) -> Result<Vec<Collection>, String> {
        self.get("/api/v2/owner/trash/collections?limit=200&offset=0")
            .await
    }

    pub async fn trashed_documents(&self) -> Result<Vec<TrashedDocument>, String> {
        self.get("/api/v2/owner/trash/documents?limit=200&offset=0")
            .await
    }

    pub async fn permanently_delete_collection(&self, id: i64) -> Result<(), String> {
        self.send_empty(
            Method::DELETE,
            &format!("/api/v2/owner/trash/collections/{id}"),
        )
        .await
    }

    pub async fn permanently_delete_document(&self, id: i64) -> Result<(), String> {
        self.send_empty(
            Method::DELETE,
            &format!("/api/v2/owner/trash/documents/{id}"),
        )
        .await
    }

    pub async fn model_status(&self) -> Result<ModelStatus, String> {
        self.get("/api/v2/owner/model").await
    }

    pub async fn download_model(&self) -> Result<ModelStatus, String> {
        self.send_json::<ModelStatus, serde_json::Value>(Method::POST, "/api/v2/owner/model", None)
            .await
    }

    pub async fn collections(&self, include_deleted: bool) -> Result<Vec<Collection>, String> {
        self.get(&format!(
            "/api/v1/collections?includeDeleted={}",
            if include_deleted { "true" } else { "false" }
        ))
        .await
    }

    pub async fn create_collection(
        &self,
        request: &CreateCollectionRequest,
    ) -> Result<Collection, String> {
        self.post("/api/v1/collections", request).await
    }

    pub async fn update_collection(
        &self,
        id: i64,
        request: &CreateCollectionRequest,
    ) -> Result<Collection, String> {
        self.send_json(
            Method::PUT,
            &format!("/api/v1/collections/{id}"),
            Some(request),
        )
        .await
    }

    pub async fn delete_collection(&self, id: i64) -> Result<(), String> {
        self.send_empty(Method::DELETE, &format!("/api/v1/collections/{id}"))
            .await
    }

    pub async fn restore_collection(&self, id: i64) -> Result<(), String> {
        self.send_empty(Method::POST, &format!("/api/v1/collections/{id}/restore"))
            .await
    }

    pub async fn documents(
        &self,
        collection_id: i64,
        include_deleted: bool,
    ) -> Result<Vec<Document>, String> {
        self.get(&format!(
            "/api/v1/collections/{collection_id}/documents?includeDeleted={}",
            if include_deleted { "true" } else { "false" }
        ))
        .await
    }

    pub async fn documents_page(
        &self,
        collection_id: i64,
        include_deleted: bool,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<Vec<Document>, String> {
        let mut path = format!(
            "/api/v1/collections/{collection_id}/documents?includeDeleted={}&offset={offset}",
            if include_deleted { "true" } else { "false" }
        );
        if let Some(limit) = limit {
            path.push_str(&format!("&limit={limit}"));
        }
        self.get(&path).await
    }

    pub async fn document_statuses(&self, document_ids: &[i64]) -> Result<Vec<Document>, String> {
        self.post(
            "/api/v1/documents/status",
            &DocumentStatusRequest {
                document_ids: document_ids.to_vec(),
            },
        )
        .await
    }

    pub async fn upload_path(&self, collection_id: i64, path: &Path) -> Result<Document, String> {
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "文件名无效".to_string())?;
        let bytes = read_upload_path(path).await?;
        self.upload_bytes(collection_id, filename, bytes).await
    }

    pub async fn upload_bytes(
        &self,
        collection_id: i64,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<Document, String> {
        let part = multipart::Part::bytes(bytes).file_name(filename.to_string());
        let form = multipart::Form::new().part("file", part);
        let request = self
            .authorized(
                self.http
                    .post(self.url(&format!("/api/v1/collections/{collection_id}/documents"))),
            )
            .multipart(form);
        decode(request.send().await.map_err(|error| error.to_string())?).await
    }

    pub async fn replace_document_path(
        &self,
        document_id: i64,
        path: &Path,
    ) -> Result<Document, String> {
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "文件名无效".to_string())?;
        let bytes = read_upload_path(path).await?;
        let part = multipart::Part::bytes(bytes).file_name(filename.to_string());
        let form = multipart::Form::new().part("file", part);
        let request = self
            .authorized(
                self.http
                    .put(self.url(&format!("/api/v1/documents/{document_id}")))
                    .timeout(REPLACE_TIMEOUT),
            )
            .multipart(form);
        decode(request.send().await.map_err(|error| error.to_string())?).await
    }

    pub async fn delete_document(&self, id: i64) -> Result<(), String> {
        self.send_empty(Method::DELETE, &format!("/api/v1/documents/{id}"))
            .await
    }

    pub async fn restore_document(&self, id: i64) -> Result<(), String> {
        self.send_empty(Method::POST, &format!("/api/v1/documents/{id}/restore"))
            .await
    }

    pub async fn search(&self, request: &SearchRequest) -> Result<Vec<SearchHit>, String> {
        self.post("/api/v1/search", request).await
    }

    pub async fn source_window(
        &self,
        request: &SourceWindowRequest,
    ) -> Result<SourceWindow, String> {
        self.post("/api/v1/source/window", request).await
    }

    pub async fn download_document(&self, id: i64) -> Result<(String, Vec<u8>), String> {
        let response = self
            .authorized(
                self.http
                    .get(self.url(&format!("/api/v1/documents/{id}/download"))),
            )
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(decode_error(response).await);
        }
        let filename = response
            .headers()
            .get("x-pinvou-filename-b64")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
            .and_then(|value| String::from_utf8(value).ok())
            .unwrap_or_else(|| "document".to_string());
        let filename = safe_download_filename(&filename);
        let bytes = response
            .bytes()
            .await
            .map_err(|error| error.to_string())?
            .to_vec();
        Ok((filename, bytes))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let response = self
            .authorized(self.http.get(self.url(path)))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        decode(response).await
    }

    async fn post<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        self.send_json(Method::POST, path, Some(body)).await
    }

    async fn post_with_timeout<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, String> {
        let response = self
            .authorized(self.http.post(self.url(path)).json(body).timeout(timeout))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        decode(response).await
    }

    async fn send_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, String> {
        let mut request = self.authorized(self.http.request(method, self.url(path)));
        if let Some(body) = body {
            request = request.json(body);
        }
        decode(request.send().await.map_err(|error| error.to_string())?).await
    }

    async fn send_empty(&self, method: Method, path: &str) -> Result<(), String> {
        let response = self
            .authorized(self.http.request(method, self.url(path)))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(decode_error(response).await)
        }
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.token.is_empty() {
            request
        } else {
            request.bearer_auth(&self.token)
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.endpoint, path)
    }
}

fn safe_download_filename(value: &str) -> String {
    let filename = Path::new(value)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("document");
    let filename: String = filename
        .chars()
        .filter(|character| !character.is_control())
        .take(240)
        .collect();
    if filename.is_empty() || filename == "." || filename == ".." {
        "document".to_string()
    } else {
        filename
    }
}

async fn read_upload_path(path: &Path) -> Result<Vec<u8>, String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().await.map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("请选择普通文件".to_string());
    }
    if metadata.len() == 0 || metadata.len() > MAX_UPLOAD_BYTES as u64 {
        return Err(format!(
            "文件必须大于 0 且不超过 {} MiB",
            MAX_UPLOAD_BYTES / 1024 / 1024
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_UPLOAD_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "文件必须大于 0 且不超过 {} MiB",
            MAX_UPLOAD_BYTES / 1024 / 1024
        ));
    }
    Ok(bytes)
}

pub fn parse_invite(value: &str) -> Result<ParsedInvite, String> {
    let url = url::Url::parse(value.trim()).map_err(|_| "连接邀请格式无效".to_string())?;
    if url.scheme() != "pinvou-knowledge" || url.host_str() != Some("connect") {
        return Err("这不是 Pinvou 知识库连接邀请".to_string());
    }
    let mut endpoint = None;
    let mut secret = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "endpoint" => endpoint = Some(normalize_endpoint(&value)?),
            "invite" => secret = Some(value.into_owned()),
            _ => {}
        }
    }
    Ok(ParsedInvite {
        endpoint: endpoint.ok_or_else(|| "连接邀请缺少服务地址".to_string())?,
        secret: secret
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "连接邀请缺少一次性凭证".to_string())?,
    })
}

pub fn parse_share(value: &str) -> Result<ParsedShare, String> {
    let url = url::Url::parse(value.trim()).map_err(|_| "分享连接格式无效".to_string())?;
    if url.scheme() != "pinvou-knowledge" || url.host_str() != Some("share") {
        return Err("这不是 Pinvou 共享知识库连接".to_string());
    }
    let mut server_id = None;
    let mut identity = None;
    let mut tls_ca = None;
    let mut secret = None;
    let mut endpoints = Vec::new();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "server" => server_id = Some(value.into_owned()),
            "identity" => identity = Some(value.into_owned()),
            "ca" => tls_ca = Some(value.into_owned()),
            "share" => secret = Some(value.into_owned()),
            "endpoint" => {
                let endpoint = normalize_endpoint(&value)?;
                if !endpoints.contains(&endpoint) {
                    endpoints.push(endpoint);
                }
            }
            _ => {}
        }
    }
    if endpoints.is_empty() || endpoints.len() > 8 {
        return Err("分享连接没有可用地址".to_string());
    }
    Ok(ParsedShare {
        server_id: required_share_value(server_id, "服务身份")?,
        identity: required_share_value(identity, "安全身份")?,
        tls_ca: required_share_material(tls_ca)?,
        endpoints,
        secret: required_share_value(secret, "申请凭据")?,
    })
}

fn required_share_material(value: Option<String>) -> Result<String, String> {
    let value = value
        .filter(|value| !value.trim().is_empty() && value.len() <= 4096)
        .ok_or_else(|| "分享连接缺少加密身份".to_string())?;
    let pem = URL_SAFE_NO_PAD
        .decode(value.trim())
        .map_err(|_| "分享连接的加密身份无效".to_string())?;
    reqwest::Certificate::from_pem(&pem).map_err(|_| "分享连接的加密身份无效".to_string())?;
    Ok(value)
}

pub fn new_join_credentials() -> NewJoinCredentials {
    let device_token = random_client_secret(32);
    NewJoinCredentials {
        device_token_hash: hash_client_secret(&device_token),
        device_token,
        claim_secret: random_client_secret(32),
    }
}

fn required_share_value(value: Option<String>, label: &str) -> Result<String, String> {
    value
        .filter(|value| !value.trim().is_empty() && value.len() <= 512)
        .ok_or_else(|| format!("分享连接缺少{label}"))
}

fn random_client_secret(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn hash_client_secret(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn normalize_endpoint(value: &str) -> Result<String, String> {
    let url = parse_endpoint_url(value)?;
    if url.scheme() == "http" && !is_loopback_host(url.host_str().unwrap_or_default()) {
        return Err("共享知识库连接必须使用 HTTPS；仅本机回环地址允许 HTTP".to_string());
    }
    Ok(canonical_endpoint(url))
}

/// Normalizes an endpoint that already existed in the connection store.
///
/// This deliberately validates only syntax and credentials-in-URL. A legacy
/// HTTP FQDN is returned with `requires_secure_upgrade = true`, so the desktop
/// app can keep it visible while refusing to send its bearer token. Pairing and
/// all other newly entered endpoints continue to use the strict policy above.
pub fn normalize_stored_endpoint(value: &str) -> Result<StoredEndpoint, String> {
    let url = parse_endpoint_url(value)?;
    let requires_secure_upgrade =
        url.scheme() == "http" && !is_loopback_host(url.host_str().unwrap_or_default());
    Ok(StoredEndpoint {
        endpoint: canonical_endpoint(url),
        requires_secure_upgrade,
    })
}

fn parse_endpoint_url(value: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(value.trim()).map_err(|_| "服务地址无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("服务地址必须是 http:// 或 https:// 地址".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("服务地址不能包含用户名或密码".to_string());
    }
    Ok(url)
}

fn canonical_endpoint(mut url: url::Url) -> String {
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    url.as_str().trim_end_matches('/').to_string()
}

pub fn normalize_user_endpoint(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("请输入服务器地址".to_string());
    }
    let value = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let mut url = url::Url::parse(&value).map_err(|_| "服务地址无效".to_string())?;
    if url.port().is_none() && matches!(url.scheme(), "http" | "https") {
        url.set_port(Some(3210))
            .map_err(|_| "服务端口无效".to_string())?;
    }
    normalize_endpoint(url.as_str())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    false
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
    if response.status().is_success() {
        response.json().await.map_err(|error| error.to_string())
    } else {
        Err(decode_error(response).await)
    }
}

async fn decode_error(response: reqwest::Response) -> String {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    serde_json::from_str::<ApiMessage>(&body)
        .map(|message| message.message)
        .unwrap_or_else(|_| default_status_message(status, &body))
}

fn default_status_message(status: StatusCode, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("知识库服务器返回 HTTP {status}")
    } else {
        format!("知识库服务器返回 HTTP {status}: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_parser_keeps_private_endpoint_and_secret() {
        let parsed = parse_invite(
            "pinvou-knowledge://connect?endpoint=https%3A%2F%2F100.64.0.1%3A3210&invite=once",
        )
        .unwrap();
        assert_eq!(parsed.endpoint, "https://100.64.0.1:3210");
        assert_eq!(parsed.secret, "once");
    }

    #[test]
    fn plain_http_is_limited_to_loopback() {
        assert!(normalize_endpoint("http://127.0.0.1:3210").is_ok());
        assert!(normalize_endpoint("http://[::1]:3210").is_ok());
        assert!(normalize_endpoint("http://100.64.12.34:3210").is_err());
        assert!(normalize_endpoint("http://cube.tail123.ts.net:3210").is_err());
        assert!(normalize_endpoint("http://192.168.1.12:3210").is_err());
        assert!(normalize_endpoint("http://8.8.8.8:3210").is_err());
        assert!(normalize_endpoint("http://knowledge.example.com:3210").is_err());
        assert!(normalize_endpoint("https://knowledge.example.com").is_ok());
    }

    #[test]
    fn stored_http_fqdn_is_retained_but_marked_for_secure_upgrade() {
        let stored = normalize_stored_endpoint(
            "http://knowledge.corp.example:3210/old/path?stale=value#fragment",
        )
        .unwrap();

        assert_eq!(stored.endpoint, "http://knowledge.corp.example:3210");
        assert!(stored.requires_secure_upgrade);
        assert!(normalize_endpoint(&stored.endpoint).is_err());
    }

    #[test]
    fn stored_endpoint_compatibility_does_not_allow_embedded_credentials() {
        assert!(
            normalize_stored_endpoint("http://user:secret@knowledge.corp.example:3210").is_err()
        );
    }

    #[test]
    fn bare_private_endpoint_gets_https_and_default_port() {
        assert_eq!(
            normalize_user_endpoint("192.168.1.20").unwrap(),
            "https://192.168.1.20:3210"
        );
        assert_eq!(
            normalize_user_endpoint("100.64.12.34:4321").unwrap(),
            "https://100.64.12.34:4321"
        );
    }

    #[test]
    fn downloaded_filename_cannot_escape_the_selected_directory() {
        assert_eq!(safe_download_filename("../../secret.txt"), "secret.txt");
        assert_eq!(safe_download_filename(".."), "document");
    }

    #[test]
    fn atomic_replacement_timeout_exceeds_external_parser_budget() {
        assert!(REPLACE_TIMEOUT > Duration::from_secs(120));
    }

    #[tokio::test]
    async fn oversized_upload_is_rejected_before_reading_the_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(MAX_UPLOAD_BYTES as u64 + 1).unwrap();

        let error = read_upload_path(file.path()).await.unwrap_err();

        assert!(error.contains("64 MiB"));
    }
}
