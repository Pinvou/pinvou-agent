use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use futures_util::StreamExt;
use rand::RngCore;
use reqwest::{multipart, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::model::*;
use crate::MAX_UPLOAD_BYTES;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_IDENTITY_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_JSON_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_RESPONSE_BYTES: usize = 64 * 1024;
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
// Replacement remains atomic and waits for parsing plus embedding before the
// server commits it. External parsers may legitimately consume up to 120s, so
// this operation must not inherit the generic 90s request timeout and report an
// ambiguous failure while the server is still working.
const REPLACE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct KnowledgeClient {
    endpoint: String,
    token: String,
    expected_server_id: Option<String>,
    http: reqwest::Client,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrivateEndpointKind {
    Lan,
    Tailscale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteKnowledgeProbe {
    pub endpoint: String,
    pub network_kind: PrivateEndpointKind,
    pub server_id: String,
    pub server_identity: String,
    pub server_name: String,
    pub protocol_version: u32,
    pub tls_ca: String,
    /// Upper-case SHA-256 of the stable CA certificate DER bytes.
    pub ca_fingerprint: String,
    /// A compact, human-comparable rendering of the same CA fingerprint.
    pub identity_code: String,
    pub ready: bool,
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
            expected_server_id: None,
            http,
        })
    }

    pub fn new_pinned(
        endpoint: impl Into<String>,
        token: impl Into<String>,
        tls_ca: &str,
        expected_server_id: &str,
    ) -> Result<Self, String> {
        crate::ensure_tls_crypto_provider();
        let endpoint = normalize_endpoint(&endpoint.into())?;
        if !endpoint.starts_with("https://") {
            return Err("共享知识库加密连接必须使用 HTTPS".to_string());
        }
        let expected_server_id = expected_server_id.trim();
        if expected_server_id.is_empty()
            || expected_server_id.len() > 512
            || expected_server_id.chars().any(char::is_control)
        {
            return Err("共享知识库服务身份无效".to_string());
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
            expected_server_id: Some(expected_server_id.to_string()),
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
        decode_bounded(response, MAX_IDENTITY_RESPONSE_BYTES).await
    }

    pub async fn bootstrap_identity(endpoint: &str) -> Result<ServerInfo, String> {
        crate::ensure_tls_crypto_provider();
        let endpoint = normalize_endpoint(endpoint)?;
        if !endpoint.starts_with("https://") {
            return Err("共享知识库首次连接必须使用 HTTPS".to_string());
        }
        // The private CA can only be bootstrapped over the local loopback
        // channel used by the root-owned host helper. Remote first contact must
        // carry the CA in a reusable share link; learning it from the same
        // unauthenticated network connection would be circular TOFU.
        let info = Self::local_health_untrusted(&endpoint).await?;
        let verified = Self::new_pinned(endpoint, "", &info.tls_ca, &info.server_id)?
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

    /// Retrieves the public service identity without a device token.
    ///
    /// The returned CA is not trusted yet: callers must display its fingerprint
    /// and require explicit user confirmation. A second request pinned to that
    /// CA prevents an address from changing identity within the probe itself.
    pub async fn probe_private_identity(endpoint: &str) -> Result<RemoteKnowledgeProbe, String> {
        crate::ensure_tls_crypto_provider();
        let (endpoint, network_kind) = normalize_private_user_endpoint(endpoint)?;
        // Resolve MagicDNS once, verify that it maps to a private Tailnet/ULA
        // address, then pin all subsequent traffic to the concrete IP. This
        // closes the DNS-rebinding gap between probe, confirmation and join.
        let endpoint = resolve_private_endpoint(&endpoint, network_kind).await?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(PROBE_TIMEOUT)
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        let response = http
            .get(format!("{endpoint}/api/v1/info"))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let info: ServerInfo = decode_bounded(response, MAX_IDENTITY_RESPONSE_BYTES).await?;
        validate_probed_server_info(&info)?;
        let ca_fingerprint = ca_fingerprint(&info.tls_ca)?;
        let verified = Self::new_pinned(&endpoint, "", &info.tls_ca, &info.server_id)?
            .health()
            .await?;
        if !same_probed_server_identity(&info, &verified) {
            return Err("共享知识库在建立加密连接时身份发生变化".to_string());
        }
        Ok(RemoteKnowledgeProbe {
            endpoint,
            network_kind,
            server_id: info.server_id,
            server_identity: info.identity,
            server_name: info.name,
            protocol_version: info.protocol_version,
            tls_ca: info.tls_ca,
            identity_code: identity_code_from_fingerprint(&ca_fingerprint),
            ca_fingerprint,
            ready: verified.ready,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn health(&self) -> Result<ServerInfo, String> {
        let response = self
            .authorized(self.http.get(self.url("/api/v1/info")))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        decode_bounded(response, MAX_IDENTITY_RESPONSE_BYTES).await
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

    pub async fn request_join(
        endpoint: &str,
        tls_ca: &str,
        expected_server_id: &str,
        device_name: &str,
        share_secret: Option<&str>,
        credentials: &NewJoinCredentials,
    ) -> Result<JoinRequestReceipt, String> {
        let client = Self::new_pinned(endpoint, "", tls_ca, expected_server_id)?;
        client
            .post_with_timeout(
                "/api/v2/join-requests",
                &JoinRequestCreate {
                    device_name: device_name.to_string(),
                    device_token_hash: credentials.device_token_hash.clone(),
                    claim_secret: credentials.claim_secret.clone(),
                    share_secret: share_secret.map(str::to_string),
                },
                JOIN_TIMEOUT,
            )
            .await
    }

    pub async fn join_request_status(
        endpoint: &str,
        tls_ca: &str,
        expected_server_id: &str,
        request_id: &str,
        claim_secret: &str,
    ) -> Result<JoinRequestReceipt, String> {
        let client = Self::new_pinned(endpoint, "", tls_ca, expected_server_id)?;
        client
            .post_with_timeout(
                &format!("/api/v2/join-requests/{request_id}/status"),
                &JoinRequestClaim {
                    claim_secret: claim_secret.to_string(),
                },
                JOIN_TIMEOUT,
            )
            .await
    }

    pub async fn cancel_join_request(
        endpoint: &str,
        tls_ca: &str,
        expected_server_id: &str,
        request_id: &str,
        claim_secret: &str,
    ) -> Result<JoinRequestRecord, String> {
        let client = Self::new_pinned(endpoint, "", tls_ca, expected_server_id)?;
        client
            .post_with_timeout(
                &format!("/api/v2/join-requests/{request_id}/cancel"),
                &JoinRequestClaim {
                    claim_secret: claim_secret.to_string(),
                },
                JOIN_TIMEOUT,
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
        decode(
            request
                .timeout(UPLOAD_TIMEOUT)
                .send()
                .await
                .map_err(|error| error.to_string())?,
        )
        .await
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
        let request = if let Some(expected_server_id) = &self.expected_server_id {
            request.header(crate::EXPECTED_SERVER_ID_HEADER, expected_server_id)
        } else {
            request
        };
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

/// Accepts only explicit private endpoints used by discovery/manual joining.
/// Public addresses, loopback/link-local addresses and arbitrary DNS names are
/// rejected before any network request is made.
pub fn normalize_private_user_endpoint(
    value: &str,
) -> Result<(String, PrivateEndpointKind), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("请输入局域网或 Tailscale 服务地址".to_string());
    }
    let value = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let mut url = url::Url::parse(&value).map_err(|_| "服务地址无效".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("首次私网连接必须使用不含路径或凭据的 HTTPS 地址".to_string());
    }
    if url.port().is_none() {
        url.set_port(Some(3210))
            .map_err(|_| "服务端口无效".to_string())?;
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let network_kind = match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) if is_rfc1918(address) => PrivateEndpointKind::Lan,
        Ok(IpAddr::V4(address)) if is_tailnet(address) => PrivateEndpointKind::Tailscale,
        Ok(IpAddr::V6(address)) if is_tailscale_ipv6(address) => PrivateEndpointKind::Tailscale,
        Ok(IpAddr::V6(address)) if is_ula(address) => PrivateEndpointKind::Lan,
        Ok(_) => return Err("仅支持 RFC1918/ULA 局域网地址或 Tailscale 地址".to_string()),
        Err(_) if is_tailscale_dns_name(&host) => PrivateEndpointKind::Tailscale,
        Err(_) => return Err("DNS 地址仅支持显式的 Tailscale *.ts.net 名称".to_string()),
    };
    Ok((canonical_endpoint(url), network_kind))
}

pub fn ca_fingerprint(tls_ca: &str) -> Result<String, String> {
    let pem = URL_SAFE_NO_PAD
        .decode(tls_ca.trim())
        .map_err(|_| "共享知识库加密身份无效".to_string())?;
    reqwest::Certificate::from_pem(&pem).map_err(|_| "共享知识库加密身份无效".to_string())?;
    let pem_text = std::str::from_utf8(&pem).map_err(|_| "共享知识库加密身份无效".to_string())?;
    let encoded_der = pem_text
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::trim)
        .collect::<String>();
    let der = STANDARD
        .decode(encoded_der)
        .map_err(|_| "共享知识库加密身份无效".to_string())?;
    Ok(Sha256::digest(&der)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect())
}

pub fn identity_code(tls_ca: &str) -> Result<String, String> {
    ca_fingerprint(tls_ca).map(|fingerprint| identity_code_from_fingerprint(&fingerprint))
}

fn identity_code_from_fingerprint(fingerprint: &str) -> String {
    format!(
        "PINVOU-{}-{}-{}-{}",
        &fingerprint[0..4],
        &fingerprint[4..8],
        &fingerprint[8..12],
        &fingerprint[12..16]
    )
}

fn validate_probed_server_info(info: &ServerInfo) -> Result<(), String> {
    if info.protocol_version < 2
        || info.server_id.trim().is_empty()
        || info.server_id.len() > 512
        || info.identity.trim().is_empty()
        || info.identity.len() > 512
        || info.name.trim().is_empty()
        || info.name.len() > 512
    {
        return Err("目标地址不是受支持的 PINVOU 共享知识库".to_string());
    }
    ca_fingerprint(&info.tls_ca).map(|_| ())
}

fn same_probed_server_identity(first: &ServerInfo, pinned: &ServerInfo) -> bool {
    first.server_id == pinned.server_id
        && first.identity == pinned.identity
        && first.name == pinned.name
        && first.protocol_version == pinned.protocol_version
        && first.tls_ca == pinned.tls_ca
}

fn is_rfc1918(address: std::net::Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

fn is_tailnet(address: std::net::Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_ula(address: std::net::Ipv6Addr) -> bool {
    (address.segments()[0] & 0xfe00) == 0xfc00
}

fn is_tailscale_ipv6(address: std::net::Ipv6Addr) -> bool {
    let segments = address.segments();
    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
}

fn is_tailscale_dns_name(host: &str) -> bool {
    host.strip_suffix(".ts.net")
        .is_some_and(|prefix| !prefix.is_empty() && !prefix.ends_with('.'))
}

async fn resolve_private_endpoint(
    endpoint: &str,
    network_kind: PrivateEndpointKind,
) -> Result<String, String> {
    let mut url = url::Url::parse(endpoint).map_err(|_| "服务地址无效".to_string())?;
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host.parse::<IpAddr>().is_ok() {
        return Ok(endpoint.to_string());
    }
    if network_kind != PrivateEndpointKind::Tailscale || !is_tailscale_dns_name(host) {
        return Err("私网 DNS 地址无效".to_string());
    }
    let port = url.port().unwrap_or(3210);
    let mut resolved = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("无法解析 Tailscale 地址：{error}"))?
        .map(|address| address.ip())
        .filter(|address| match address {
            IpAddr::V4(address) => is_tailnet(*address),
            IpAddr::V6(address) => is_tailscale_ipv6(*address),
        })
        .collect::<Vec<_>>();
    resolved.sort_by_key(|address| (matches!(address, IpAddr::V6(_)), address.to_string()));
    resolved.dedup();
    let address = resolved.into_iter().next().ok_or_else(|| {
        "Tailscale 名称没有解析到 100.64.0.0/10 或 Tailscale IPv6 地址".to_string()
    })?;
    url.set_host(Some(&address.to_string()))
        .map_err(|_| "Tailscale 地址无效".to_string())?;
    Ok(canonical_endpoint(url))
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
    decode_bounded(response, MAX_JSON_RESPONSE_BYTES).await
}

async fn decode_bounded<T: DeserializeOwned>(
    response: reqwest::Response,
    limit: usize,
) -> Result<T, String> {
    let status = response.status();
    let body_limit = if status.is_success() {
        limit
    } else {
        limit.min(MAX_ERROR_RESPONSE_BYTES)
    };
    let body = read_bounded_body(response, body_limit).await?;
    if status.is_success() {
        serde_json::from_slice(&body).map_err(|error| error.to_string())
    } else {
        Err(decode_error_body(status, &body))
    }
}

async fn read_bounded_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!("知识库服务器响应超过 {} KiB 上限", limit / 1024));
    }
    let mut body = Vec::with_capacity(response.content_length().unwrap_or(0) as usize);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(format!("知识库服务器响应超过 {} KiB 上限", limit / 1024));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn decode_error(response: reqwest::Response) -> String {
    let status = response.status();
    match read_bounded_body(response, MAX_ERROR_RESPONSE_BYTES).await {
        Ok(body) => decode_error_body(status, &body),
        Err(error) => error,
    }
}

fn decode_error_body(status: StatusCode, body: &[u8]) -> String {
    serde_json::from_slice::<ApiMessage>(body)
        .map(|message| message.message)
        .unwrap_or_else(|_| default_status_message(status, &String::from_utf8_lossy(body)))
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
    use tokio::io::AsyncWriteExt;

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
    fn private_first_contact_accepts_lan_and_explicit_tailscale_only() {
        for (source, expected, kind) in [
            (
                "192.168.1.20",
                "https://192.168.1.20:3210",
                PrivateEndpointKind::Lan,
            ),
            (
                "https://172.31.4.9:4321/",
                "https://172.31.4.9:4321",
                PrivateEndpointKind::Lan,
            ),
            (
                "https://[fd12::3]:3210",
                "https://[fd12::3]:3210",
                PrivateEndpointKind::Lan,
            ),
            (
                "100.100.12.34",
                "https://100.100.12.34:3210",
                PrivateEndpointKind::Tailscale,
            ),
            (
                "cube.team-name.ts.net:4321",
                "https://cube.team-name.ts.net:4321",
                PrivateEndpointKind::Tailscale,
            ),
        ] {
            assert_eq!(
                normalize_private_user_endpoint(source).unwrap(),
                (expected.to_string(), kind),
                "{source}"
            );
        }
    }

    #[test]
    fn private_first_contact_rejects_public_loopback_link_local_and_ambiguous_names() {
        for source in [
            "8.8.8.8:3210",
            "127.0.0.1:3210",
            "169.254.1.2:3210",
            "172.32.0.1:3210",
            "fe80::1",
            "knowledge.example.com:3210",
            "ts.net:3210",
            "http://192.168.1.20:3210",
            "https://user:secret@192.168.1.20:3210",
            "https://192.168.1.20:3210/api/v1/info",
            "https://192.168.1.20:3210?redirect=1",
        ] {
            assert!(normalize_private_user_endpoint(source).is_err(), "{source}");
        }
    }

    #[test]
    fn pinned_probe_allows_runtime_readiness_to_change_without_changing_identity() {
        let first = ServerInfo {
            server_id: "server".to_string(),
            identity: "identity".to_string(),
            name: "Cube".to_string(),
            version: "0.8.1".to_string(),
            protocol_version: 2,
            tls_ca: "ca".to_string(),
            initialized: true,
            ready: false,
            model: "bge-m3".to_string(),
        };
        let mut pinned = first.clone();
        pinned.ready = true;

        assert!(same_probed_server_identity(&first, &pinned));
        pinned.server_id = "other".to_string();
        assert!(!same_probed_server_identity(&first, &pinned));
    }

    #[cfg(feature = "server")]
    #[test]
    fn identity_code_is_stably_derived_from_the_public_ca() {
        let root = tempfile::tempdir().unwrap();
        let tls = crate::tls::ensure_tls_identity(root.path()).unwrap();

        let fingerprint = ca_fingerprint(&tls.ca_encoded).unwrap();

        assert_eq!(fingerprint.len(), 64);
        assert_eq!(identity_code(&tls.ca_encoded).unwrap().len(), 26);
        assert_eq!(
            identity_code(&tls.ca_encoded).unwrap(),
            identity_code_from_fingerprint(&fingerprint)
        );
    }

    #[tokio::test]
    async fn remote_identity_cannot_be_bootstrapped_from_an_untrusted_connection() {
        let error = KnowledgeClient::bootstrap_identity("https://192.168.1.20:3210")
            .await
            .unwrap_err();

        assert!(error.contains("回环"));
    }

    #[test]
    fn downloaded_filename_cannot_escape_the_selected_directory() {
        assert_eq!(safe_download_filename("../../secret.txt"), "secret.txt");
        assert_eq!(safe_download_filename(".."), "document");
    }

    #[test]
    fn atomic_replacement_timeout_exceeds_external_parser_budget() {
        assert!(REPLACE_TIMEOUT > Duration::from_secs(120));
        assert!(UPLOAD_TIMEOUT > Duration::from_secs(120));
    }

    #[tokio::test]
    async fn identity_probe_rejects_oversized_success_and_error_bodies() {
        for status in ["200 OK", "500 Internal Server Error"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let body = vec![b'x'; MAX_IDENTITY_RESPONSE_BYTES + 1];
            let status = status.to_string();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await;
                let header = format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n");
                stream.write_all(header.as_bytes()).await.unwrap();
                let _ = stream.write_all(&body).await;
            });
            let response = reqwest::Client::new()
                .get(format!("http://{address}/api/v1/info"))
                .send()
                .await
                .unwrap();

            let error = decode_bounded::<ServerInfo>(response, MAX_IDENTITY_RESPONSE_BYTES)
                .await
                .unwrap_err();

            assert!(error.contains("64 KiB"));
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn oversized_upload_is_rejected_before_reading_the_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(MAX_UPLOAD_BYTES as u64 + 1).unwrap();

        let error = read_upload_path(file.path()).await.unwrap_err();

        assert!(error.contains("64 MiB"));
    }
}
