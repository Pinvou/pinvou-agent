use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccessScope {
    Read,
    Manage,
    Owner,
}

impl AccessScope {
    pub fn can_manage(self) -> bool {
        matches!(self, Self::Manage | Self::Owner)
    }

    pub fn is_owner(self) -> bool {
        matches!(self, Self::Owner)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub server_id: String,
    #[serde(default)]
    pub identity: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_protocol_version")]
    pub protocol_version: u32,
    #[serde(default)]
    pub tls_ca: String,
    pub initialized: bool,
    pub ready: bool,
    pub model: String,
}

fn default_protocol_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareCreateRequest {
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub auto_approve_read: bool,
    pub expires_in_hours: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareCreated {
    pub share_id: String,
    pub share: String,
    pub expires_at: i64,
    pub auto_approve_read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareRecord {
    pub id: String,
    pub endpoints: Vec<String>,
    pub auto_approve_read: bool,
    pub created_at: i64,
    pub expires_at: i64,
    pub stopped_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinRequestStatus {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequestCreate {
    pub device_name: String,
    pub device_token_hash: String,
    pub claim_secret: String,
    pub share_secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequestClaim {
    pub claim_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequestRecord {
    pub id: String,
    pub device_name: String,
    pub status: JoinRequestStatus,
    pub scope: Option<AccessScope>,
    pub share_id: Option<String>,
    pub device_id: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequestReceipt {
    pub request: JoinRequestRecord,
    pub server: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveJoinRequest {
    pub scope: AccessScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairResponse {
    pub server: ServerInfo,
    pub token: String,
    pub scope: AccessScope,
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub doc_count: i64,
    pub chunk_count: i64,
    pub total_bytes: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub id: i64,
    pub collection_id: i64,
    pub name: String,
    pub ext: Option<String>,
    pub size: i64,
    pub sha256: String,
    pub status: String,
    pub n_chunks: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrashedDocument {
    #[serde(flatten)]
    pub document: Document,
    pub collection_name: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentStatusRequest {
    pub document_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCollectionRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    #[serde(default)]
    pub collection_ids: Vec<i64>,
    pub query: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

fn default_search_limit() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub collection_id: i64,
    pub document_id: i64,
    pub document_name: String,
    pub text: String,
    pub ord: i64,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceWindowRequest {
    pub collection_id: i64,
    pub document_id: i64,
    pub start_ord: i64,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceWindow {
    pub document: Document,
    pub chunks: Vec<SourceChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceChunk {
    pub ord: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceGrant {
    pub id: String,
    pub name: String,
    pub scope: AccessScope,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDeviceRequest {
    pub name: Option<String>,
    pub scope: Option<AccessScope>,
    pub revoked: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanReadAccessRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiMessage {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub name: String,
    pub ready: bool,
    pub downloading: bool,
    pub error: Option<String>,
}
