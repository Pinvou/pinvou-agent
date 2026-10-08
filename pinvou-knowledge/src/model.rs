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
    /// 2026-10 前发布的客户端把本字段定义为 serde 必需(无 default),服务端一旦
    /// 停止序列化,混合版本对端(LAN 发现 / 独立 knowledge-host)首次接触即
    /// `missing field 'initialized'`。`#[serde(default)]` 只保新客户端的缺键容错,
    /// 兼容旧客户端靠的是服务端继续序列化(见 wire_compat_tests)。
    #[serde(default)]
    pub initialized: bool,
    pub ready: bool,
    /// 模型文件是否已在磁盘上。懒装载语义下 `ready` 只反映「已进内存」,
    /// 挂载方据此字段判断可用性(首次检索会按需装载)。旧服务器无此字段。
    #[serde(default)]
    pub model_present: bool,
    /// 旧客户端必需字段,同 `initialized`(线上兼容,勿删)。
    #[serde(default)]
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
    /// 旧客户端必需字段(serde 无 default,勿删)。不下发,置 None 仅保线上
    /// 兼容(见 wire_compat_tests)。
    #[serde(default)]
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
    /// 旧客户端必需字段(serde 无 default,勿删)。当前不下发真实体积,置 0
    /// 仅保线上兼容(见 wire_compat_tests)。
    #[serde(default)]
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
    /// 旧客户端必需字段(serde 无 default,勿删)。不下发真实时间戳,置 0
    /// 仅保线上兼容(见 wire_compat_tests)。
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
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
    /// 2026-10 前发布的客户端把 score 定义为 serde 必需，必须继续下发
    /// （服务端排序本就计算该值）；`default` 只负责旧服务端 + 新客户端
    /// 的缺键容错。
    #[serde(default)]
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
    /// 旧客户端必需字段(serde 无 default,勿删)。不再读真实时间,置 0
    /// 仅保线上兼容(见 wire_compat_tests)。
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
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

// 2026-10 前发布的客户端把若干响应字段定义为 serde 必需(结构里无 #[serde(default)])。
// 服务端一旦停止序列化这些键,混合版本对端(LAN 发现本就是跨机场景;Linux 独立
// knowledge-host helper 可能落后于 app 升级)首次接触即在版本检查之前死于
// `missing field …`——旧客户端先反序列化 /api/v1/info,再看 protocol_version。
// 以下测试钉住:兼容键必须始终出现在序列化输出;`#[serde(default)]` 只负责
// 新客户端面对缺键时的容错,替代不了服务端继续序列化。
#[cfg(test)]
mod wire_compat_tests {
    use super::*;

    #[test]
    fn server_info_keeps_legacy_fields_on_the_wire() {
        let info = ServerInfo {
            server_id: "server".into(),
            identity: "identity".into(),
            name: "PINVOU Knowledge".into(),
            version: "0.0.0".into(),
            protocol_version: 2,
            tls_ca: String::new(),
            initialized: true,
            ready: false,
            model_present: true,
            model: "bge-m3".into(),
        };
        let json = serde_json::to_value(&info).unwrap();
        for key in ["initialized", "model", "ready", "modelPresent"] {
            assert!(json.get(key).is_some(), "legacy key missing on wire: {key}");
        }
        // 缺键对已带 #[serde(default)] 的新客户端无害。
        let minimal: ServerInfo = serde_json::from_value(serde_json::json!({
            "serverId": "server",
            "name": "PINVOU Knowledge",
            "version": "0.0.0",
            "protocolVersion": 2,
            "ready": true,
        }))
        .unwrap();
        assert!(!minimal.initialized);
        assert_eq!(minimal.model, "");
    }

    #[test]
    fn row_models_keep_legacy_fields_on_the_wire() {
        let collection = Collection {
            id: 1,
            name: "c".into(),
            description: None,
            status: "ok".into(),
            doc_count: 0,
            chunk_count: 0,
            total_bytes: 0,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
        };
        let document = Document {
            id: 1,
            collection_id: 1,
            name: "d".into(),
            ext: None,
            size: 0,
            sha256: String::new(),
            status: "ok".into(),
            n_chunks: 0,
            created_at: 0,
            updated_at: 0,
            deleted_at: None,
            error: None,
            already_exists: false,
        };
        let grant = DeviceGrant {
            id: "g".into(),
            name: "device".into(),
            scope: AccessScope::Read,
            created_at: 0,
            last_seen_at: None,
            revoked: false,
        };
        let request = JoinRequestRecord {
            id: "j".into(),
            device_name: "device".into(),
            status: JoinRequestStatus::Pending,
            scope: None,
            share_id: None,
            device_id: None,
            created_at: 0,
            expires_at: 0,
            resolved_at: None,
        };
        let collection = serde_json::to_value(&collection).unwrap();
        let document = serde_json::to_value(&document).unwrap();
        let grant = serde_json::to_value(&grant).unwrap();
        let request = serde_json::to_value(&request).unwrap();
        for (json, key) in [
            (&collection, "totalBytes"),
            (&document, "createdAt"),
            (&document, "updatedAt"),
            (&grant, "createdAt"),
            (&grant, "lastSeenAt"),
            (&request, "resolvedAt"),
        ] {
            assert!(json.get(key).is_some(), "legacy key missing on wire: {key}");
        }
    }

    #[test]
    fn search_hit_keeps_score_on_the_wire() {
        let hit = SearchHit {
            collection_id: 1,
            document_id: 2,
            document_name: "d".into(),
            text: "匹配文本".into(),
            ord: 0,
            score: 0.5,
        };
        let json = serde_json::to_value(&hit).unwrap();
        assert!(
            json.get("score").is_some(),
            "legacy key missing on wire: score"
        );
        // 缺键对已带 #[serde(default)] 的新客户端无害。
        let minimal: SearchHit = serde_json::from_value(serde_json::json!({
            "collectionId": 1,
            "documentId": 2,
            "documentName": "d",
            "text": "匹配文本",
            "ord": 0,
        }))
        .unwrap();
        assert_eq!(minimal.score, 0.0);
    }
}
