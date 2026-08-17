use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{middleware, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use crate::model::*;
use crate::service::KnowledgeService;
use crate::store::DeviceMutationError;
use crate::MAX_UPLOAD_BYTES;

const SMALL_JSON_LIMIT: usize = 16 * 1024;
const UPLOAD_REQUEST_LIMIT: usize = MAX_UPLOAD_BYTES + 1024 * 1024;

pub async fn serve(service: Arc<KnowledgeService>, bind: SocketAddr) -> Result<(), String> {
    // mDNS only announces a display name and a listening address. The stable
    // CA and server identity are learned by an explicit untrusted probe and
    // must be confirmed by the user before a join request is created.
    let _discovery = if bind.ip().is_unspecified() && bind.port() != 0 {
        service.server_info().ok().and_then(|info| {
            match crate::discovery::advertise(&info.name, bind.port()) {
                Ok(advertisement) => Some(advertisement),
                Err(error) => {
                    eprintln!("[pinvou-knowledge] LAN discovery unavailable: {error}");
                    None
                }
            }
        })
    } else {
        None
    };
    let app = router(service.clone());
    let tls = tls_config(&service).await?;
    axum_server::bind_rustls(bind, tls)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .map_err(|error| error.to_string())
}

async fn tls_config(
    service: &KnowledgeService,
) -> Result<axum_server::tls_rustls::RustlsConfig, String> {
    crate::ensure_tls_crypto_provider();
    axum_server::tls_rustls::RustlsConfig::from_pem_file(
        &service.tls_identity().certificate_path,
        &service.tls_identity().private_key_path,
    )
    .await
    .map_err(|error| error.to_string())
}

pub fn router(service: Arc<KnowledgeService>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/info", get(info))
        .route("/api/v1/access", get(current_access))
        .route(
            "/api/v2/join-requests",
            post(create_join_request).layer(DefaultBodyLimit::max(SMALL_JSON_LIMIT)),
        )
        .route(
            "/api/v2/join-requests/{id}/status",
            post(claim_join_request).layer(DefaultBodyLimit::max(SMALL_JSON_LIMIT)),
        )
        .route(
            "/api/v2/join-requests/{id}/cancel",
            post(cancel_join_request).layer(DefaultBodyLimit::max(SMALL_JSON_LIMIT)),
        )
        .route(
            "/api/v2/owner/shares",
            get(owner_shares).post(owner_create_share),
        )
        .route(
            "/api/v2/owner/shares/{id}",
            axum::routing::delete(owner_stop_share),
        )
        .route("/api/v2/owner/join-requests", get(owner_join_requests))
        .route(
            "/api/v2/owner/join-requests/{id}/approve",
            post(owner_approve_join_request),
        )
        .route(
            "/api/v2/owner/join-requests/{id}/reject",
            post(owner_reject_join_request),
        )
        .route("/api/v2/owner/devices", get(owner_devices))
        .route(
            "/api/v2/owner/devices/{id}",
            patch(owner_update_device).delete(owner_delete_device),
        )
        .route(
            "/api/v2/owner/trash/collections",
            get(owner_trashed_collections),
        )
        .route(
            "/api/v2/owner/trash/collections/{id}",
            axum::routing::delete(owner_permanently_delete_collection),
        )
        .route(
            "/api/v2/owner/trash/documents",
            get(owner_trashed_documents),
        )
        .route(
            "/api/v2/owner/trash/documents/{id}",
            axum::routing::delete(owner_permanently_delete_document),
        )
        .route(
            "/api/v2/owner/model",
            get(owner_model_status).post(owner_download_model),
        )
        .route(
            "/api/v1/collections",
            get(collections).post(create_collection),
        )
        .route(
            "/api/v1/collections/{id}",
            put(update_collection).delete(delete_collection),
        )
        .route("/api/v1/collections/{id}/restore", post(restore_collection))
        .route(
            "/api/v1/collections/{id}/documents",
            get(documents).post(upload_document),
        )
        .route("/api/v1/documents/status", post(document_statuses))
        .route(
            "/api/v1/documents/{id}",
            put(replace_document).delete(delete_document),
        )
        .route("/api/v1/documents/{id}/restore", post(restore_document))
        .route("/api/v1/documents/{id}/download", get(download_document))
        .route("/api/v1/search", post(search))
        .route("/api/v1/source/window", post(source_window))
        .layer(middleware::from_fn(apply_route_body_limit))
        .layer(middleware::from_fn_with_state(
            service.clone(),
            enforce_expected_server,
        ))
        .layer(CatchPanicLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(service)
}

async fn enforce_expected_server(
    State(service): State<Arc<KnowledgeService>>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    if let Some(expected) = request.headers().get(crate::EXPECTED_SERVER_ID_HEADER) {
        let Ok(expected) = expected.to_str() else {
            return ApiError::bad_request("共享知识库服务身份无效").into_response();
        };
        if expected != service.server_id() {
            return ApiError::conflict("共享知识库服务身份与连接记录不一致").into_response();
        }
    }
    next.run(request).await
}

async fn apply_route_body_limit(
    mut request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let path = request.uri().path();
    let collection_upload = path
        .strip_prefix("/api/v1/collections/")
        .and_then(|remainder| remainder.strip_suffix("/documents"))
        .is_some_and(|id| id.parse::<i64>().is_ok());
    let document_replace = path
        .strip_prefix("/api/v1/documents/")
        .is_some_and(|id| id.parse::<i64>().is_ok());
    let is_upload = (*request.method() == axum::http::Method::POST && collection_upload)
        || (*request.method() == axum::http::Method::PUT && document_replace);
    DefaultBodyLimit::max(if is_upload {
        UPLOAD_REQUEST_LIMIT
    } else {
        SMALL_JSON_LIMIT
    })
    .apply(&mut request);
    next.run(request).await
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"ok": true}))
}

async fn info(State(service): State<Arc<KnowledgeService>>) -> ApiResult<Json<ServerInfo>> {
    Ok(Json(service.server_info().map_err(ApiError::internal)?))
}

async fn current_access(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
) -> ApiResult<Json<DeviceGrant>> {
    Ok(Json(require_device_access(&service, &headers)?))
}

async fn create_join_request(
    State(service): State<Arc<KnowledgeService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    uri: Uri,
    headers: HeaderMap,
    Json(request): Json<JoinRequestCreate>,
) -> ApiResult<impl IntoResponse> {
    require_private_direct_connection(source.ip(), &uri, &headers)?;
    let receipt = service
        .submit_join_request(source.ip(), request)
        .map_err(map_join_request_error)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((StatusCode::CREATED, response_headers, Json(receipt)))
}

async fn claim_join_request(
    State(service): State<Arc<KnowledgeService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<JoinRequestClaim>,
) -> ApiResult<impl IntoResponse> {
    require_private_direct_connection(source.ip(), &uri, &headers)?;
    service
        .check_join_claim_rate(source.ip())
        .map_err(map_join_request_error)?;
    let claim_secret = request.claim_secret;
    let receipt =
        tokio::task::spawn_blocking(move || service.claim_join_request(&id, &claim_secret))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::not_found)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((response_headers, Json(receipt)))
}

async fn cancel_join_request(
    State(service): State<Arc<KnowledgeService>>,
    ConnectInfo(source): ConnectInfo<SocketAddr>,
    uri: Uri,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<JoinRequestClaim>,
) -> ApiResult<impl IntoResponse> {
    require_private_direct_connection(source.ip(), &uri, &headers)?;
    service
        .check_join_claim_rate(source.ip())
        .map_err(map_join_request_error)?;
    let claim_secret = request.claim_secret;
    let request =
        tokio::task::spawn_blocking(move || service.cancel_join_request(&id, &claim_secret))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::conflict)?;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok((response_headers, Json(request)))
}

async fn owner_create_share(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Json(request): Json<ShareCreateRequest>,
) -> ApiResult<Json<ShareCreated>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .create_share(request)
            .map_err(ApiError::bad_request)?,
    ))
}

async fn owner_shares(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<ShareRecord>>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(service.list_shares().map_err(ApiError::internal)?))
}

async fn owner_stop_share(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<ShareRecord>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(service.stop_share(&id).map_err(ApiError::not_found)?))
}

async fn owner_join_requests(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<JoinRequestRecord>>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .list_join_requests(query.limit, query.offset)
            .map_err(ApiError::internal)?,
    ))
}

async fn owner_approve_join_request(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ResolveJoinRequest>,
) -> ApiResult<Json<JoinRequestRecord>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .approve_join_request(&id, request.scope)
            .map_err(ApiError::conflict)?,
    ))
}

async fn owner_reject_join_request(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<JoinRequestRecord>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .reject_join_request(&id)
            .map_err(ApiError::conflict)?,
    ))
}

async fn owner_devices(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<DeviceGrant>>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .list_devices_page(query.limit, query.offset)
            .map_err(ApiError::internal)?,
    ))
}

async fn owner_update_device(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<UpdateDeviceRequest>,
) -> ApiResult<Json<DeviceGrant>> {
    require_owner_device(&service, &headers)?;
    if request.scope.is_some_and(AccessScope::is_owner) {
        return Err(ApiError::forbidden("所有者只能由主机本机 PINVOU 提升"));
    }
    Ok(Json(
        service
            .update_device(&id, request)
            .map_err(map_device_mutation_error)?,
    ))
}

async fn owner_delete_device(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let caller = require_owner_device(&service, &headers)?;
    if caller.id == id {
        return Err(ApiError::forbidden("不能移除当前所有者设备"));
    }
    service
        .delete_device(&id)
        .map_err(map_device_mutation_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn map_device_mutation_error(error: DeviceMutationError) -> ApiError {
    match error {
        DeviceMutationError::NotFound => ApiError::not_found("设备不存在"),
        DeviceMutationError::OwnerProtected => ApiError::forbidden("所有者设备只能在主机本机管理"),
        DeviceMutationError::HostOwnerProtected => ApiError::forbidden("不能修改主机所有者设备"),
        DeviceMutationError::Revoked => ApiError::conflict("请先恢复该成员设备"),
        DeviceMutationError::Database(error) => ApiError::internal(error.to_string()),
    }
}

async fn owner_trashed_collections(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<Collection>>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .trashed_collections_page(query.limit.unwrap_or(200), query.offset)
            .map_err(ApiError::internal)?,
    ))
}

async fn owner_trashed_documents(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Vec<TrashedDocument>>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(
        service
            .trashed_documents_page(query.limit.unwrap_or(200), query.offset)
            .map_err(ApiError::internal)?,
    ))
}

async fn owner_permanently_delete_collection(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_owner_device(&service, &headers)?;
    service
        .permanently_delete_collection(id)
        .map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn owner_permanently_delete_document(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_owner_device(&service, &headers)?;
    service
        .permanently_delete_document(id)
        .map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn owner_model_status(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
) -> ApiResult<Json<ModelStatus>> {
    require_owner_device(&service, &headers)?;
    Ok(Json(service.model_status()))
}

async fn owner_download_model(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, Json<ModelStatus>)> {
    let caller = require_owner_device(&service, &headers)?;
    if !service
        .is_host_owner_device(&caller.id)
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::forbidden("模型下载只能由宿主本机 PINVOU 启动"));
    }
    service.begin_model_download().map_err(ApiError::conflict)?;
    let task = service.clone();
    tokio::spawn(async move {
        let _guard = ModelDownloadGuard(task.clone());
        if let Err(error) = task.download_model().await {
            task.record_model_error(error);
        }
    });
    Ok((StatusCode::ACCEPTED, Json(service.model_status())))
}

struct ModelDownloadGuard(Arc<KnowledgeService>);

impl Drop for ModelDownloadGuard {
    fn drop(&mut self) {
        self.0.finish_model_download();
    }
}

fn map_join_request_error(error: String) -> ApiError {
    if error.contains("过多") || error.contains("频繁") {
        ApiError::too_many_requests(error)
    } else {
        ApiError::bad_request(error)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    #[serde(default)]
    include_deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentListQuery {
    #[serde(default)]
    include_deleted: bool,
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
}

async fn collections(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<Collection>>> {
    // Recycle-bin metadata is management data. Read-only devices may browse
    // active content, but must not enumerate objects that a maintainer removed.
    require_access(&service, &headers, query.include_deleted)?;
    Ok(Json(
        service
            .collections(query.include_deleted)
            .map_err(ApiError::internal)?,
    ))
}

async fn create_collection(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Json(request): Json<CreateCollectionRequest>,
) -> ApiResult<Json<Collection>> {
    require_access(&service, &headers, true)?;
    Ok(Json(
        service
            .create_collection(request)
            .map_err(ApiError::bad_request)?,
    ))
}

async fn update_collection(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<CreateCollectionRequest>,
) -> ApiResult<Json<Collection>> {
    require_access(&service, &headers, true)?;
    Ok(Json(
        service
            .update_collection(id, request)
            .map_err(ApiError::bad_request)?,
    ))
}

async fn delete_collection(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_access(&service, &headers, true)?;
    service
        .trash_collection(id)
        .map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore_collection(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_access(&service, &headers, true)?;
    service
        .restore_collection(id)
        .map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn documents(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<DocumentListQuery>,
) -> ApiResult<Json<Vec<Document>>> {
    require_access(&service, &headers, query.include_deleted)?;
    let documents = if query.limit.is_none() && query.offset == 0 {
        service.documents(id, query.include_deleted)
    } else {
        service.documents_page(id, query.include_deleted, query.limit, query.offset)
    };
    Ok(Json(documents.map_err(ApiError::internal)?))
}

async fn multipart_file(mut multipart: Multipart) -> ApiResult<(String, Vec<u8>)> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(ApiError::bad_request)?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field.file_name().unwrap_or("document").to_string();
        let bytes = field.bytes().await.map_err(ApiError::bad_request)?.to_vec();
        return Ok((filename, bytes));
    }
    Err(ApiError::bad_request("请求中没有 file 字段"))
}

async fn upload_document(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> ApiResult<Json<Document>> {
    require_access(&service, &headers, true)?;
    let (filename, bytes) = multipart_file(multipart).await?;
    Ok(Json(
        service
            .upload_document(id, &filename, bytes)
            .await
            .map_err(ApiError::bad_request)?,
    ))
}

async fn document_statuses(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Json(request): Json<DocumentStatusRequest>,
) -> ApiResult<Json<Vec<Document>>> {
    require_access(&service, &headers, false)?;
    Ok(Json(
        service
            .document_statuses(request.document_ids)
            .map_err(ApiError::bad_request)?,
    ))
}

async fn replace_document(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    multipart: Multipart,
) -> ApiResult<Json<Document>> {
    require_access(&service, &headers, true)?;
    let (filename, bytes) = multipart_file(multipart).await?;
    Ok(Json(
        service
            .replace_document(id, &filename, bytes)
            .await
            .map_err(ApiError::bad_request)?,
    ))
}

async fn delete_document(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_access(&service, &headers, true)?;
    service.trash_document(id).map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore_document(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    require_access(&service, &headers, true)?;
    service
        .restore_document(id)
        .map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn download_document(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    require_access(&service, &headers, false)?;
    let (document, path) = service.source_file(id).map_err(ApiError::not_found)?;
    let bytes = tokio::fs::read(path).await.map_err(ApiError::internal)?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(
            mime_guess::from_path(&document.name)
                .first_or_octet_stream()
                .as_ref(),
        )
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    response
        .headers_mut()
        .insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    response.headers_mut().insert(
        "x-pinvou-filename-b64",
        HeaderValue::from_str(&URL_SAFE_NO_PAD.encode(document.name.as_bytes()))
            .map_err(ApiError::internal)?,
    );
    Ok(response)
}

async fn search(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> ApiResult<Json<Vec<SearchHit>>> {
    require_access(&service, &headers, false)?;
    if !service.ready() {
        return Err(ApiError::unavailable("embedding 模型未就绪"));
    }
    let permit = service
        .acquire_search_slot()
        .await
        .map_err(ApiError::unavailable)?;
    // ONNX inference and the SQLite vector scan are synchronous CPU/IO work.
    // Keep them off Tokio's request workers so health, auth and admin UI remain
    // responsive while one or more searches are running.
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        service.search(request)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::bad_request)?;
    Ok(Json(result))
}

async fn source_window(
    State(service): State<Arc<KnowledgeService>>,
    headers: HeaderMap,
    Json(request): Json<SourceWindowRequest>,
) -> ApiResult<Json<SourceWindow>> {
    require_access(&service, &headers, false)?;
    Ok(Json(
        service
            .source_window(request)
            .map_err(ApiError::not_found)?,
    ))
}

fn is_private_network_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(address) => {
            if let Some(address) = address.to_ipv4_mapped() {
                return is_private_network_ip(IpAddr::V4(address));
            }
            let first = address.segments()[0];
            address.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

fn has_forwarding_headers(headers: &HeaderMap) -> bool {
    ["forwarded", "x-forwarded-for", "x-real-ip"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

fn is_private_network_host(uri: &Uri, headers: &HeaderMap) -> bool {
    // HTTP/2 carries the target host in `:authority`, which hyper exposes on
    // the request URI rather than as a `Host` header. Prefer it, then fall
    // back to the HTTP/1.1 Host header.
    let Some(authority) = uri.authority().cloned().or_else(|| {
        headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
    }) else {
        return false;
    };
    let host = authority
        .host()
        .trim_matches(['[', ']'])
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        return is_private_network_ip(address);
    }
    [".local", ".lan", ".internal", ".home.arpa", ".ts.net"]
        .iter()
        .any(|suffix| host.ends_with(suffix))
}

fn require_private_direct_connection(
    source: IpAddr,
    uri: &Uri,
    headers: &HeaderMap,
) -> ApiResult<()> {
    if has_forwarding_headers(headers) {
        return Err(ApiError::forbidden(
            "加入共享知识库不接受转发来源，请直接连接服务器地址",
        ));
    }
    if !is_private_network_host(uri, headers) || !is_private_network_ip(source) {
        return Err(ApiError::forbidden(
            "加入共享知识库仅支持局域网或 Tailscale 私有地址",
        ));
    }
    Ok(())
}

fn require_access(
    service: &KnowledgeService,
    headers: &HeaderMap,
    manage: bool,
) -> ApiResult<AccessScope> {
    let grant = require_device_access(service, headers)?;
    if manage && !grant.scope.can_manage() {
        return Err(ApiError::forbidden("该设备只有只读访问权限"));
    }
    Ok(grant.scope)
}

fn require_device_access(
    service: &KnowledgeService,
    headers: &HeaderMap,
) -> ApiResult<DeviceGrant> {
    let token = bearer(headers).ok_or_else(|| ApiError::unauthorized("缺少访问令牌"))?;
    service
        .authorize(token)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::unauthorized("访问令牌无效或已撤销"))
}

fn require_owner_device(service: &KnowledgeService, headers: &HeaderMap) -> ApiResult<DeviceGrant> {
    let grant = require_device_access(service, headers)?;
    if grant.scope.is_owner() {
        Ok(grant)
    } else {
        Err(ApiError::forbidden("该操作仅限共享知识库所有者"))
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

type ApiResult<T> = Result<T, ApiError>;

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error)
    }

    fn unauthorized(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, error)
    }

    fn forbidden(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::FORBIDDEN, error)
    }

    fn not_found(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::NOT_FOUND, error)
    }

    fn conflict(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::CONFLICT, error)
    }

    fn unavailable(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, error)
    }

    fn too_many_requests(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, error)
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error)
    }

    fn new(status: StatusCode, error: impl std::fmt::Display) -> Self {
        Self {
            status,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiMessage {
                message: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::{AUTHORIZATION, HOST};
    use axum::http::Method;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde::Serialize;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_is_public_but_collections_require_authentication() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let app = router(service);
        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let collections = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/collections")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(collections.status(), StatusCode::UNAUTHORIZED);
        let body = collections.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("访问令牌"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn network_api_requires_the_persisted_service_ca() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let tls = tls_config(&service).await.unwrap();
        let server =
            tokio::spawn(axum_server::from_tcp_rustls(listener, tls).unwrap().serve(
                router(service.clone()).into_make_service_with_connect_info::<SocketAddr>(),
            ));
        let endpoint = format!("https://{address}");
        let pinned = crate::client::KnowledgeClient::new_pinned(
            &endpoint,
            "",
            &service.tls_identity().ca_encoded,
            service.server_id(),
        )
        .unwrap();
        let mut info = None;
        let mut last_error = String::new();
        for _ in 0..20 {
            match tokio::time::timeout(std::time::Duration::from_millis(250), pinned.health()).await
            {
                Ok(Ok(value)) => {
                    info = Some(value);
                    break;
                }
                Ok(Err(error)) => last_error = error,
                Err(_) => last_error = "request timed out".to_string(),
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        if info.is_none() {
            server.abort();
        }
        let info = info.unwrap_or_else(|| panic!("pinned TLS client should connect: {last_error}"));
        assert_eq!(info.server_id, service.server_info().unwrap().server_id);
        assert!(matches!(
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                crate::client::KnowledgeClient::new(&endpoint, "")
                    .unwrap()
                    .health()
            )
            .await,
            Ok(Err(_))
        ));
        let wrong_identity = crate::client::KnowledgeClient::new_pinned(
            &endpoint,
            "",
            &service.tls_identity().ca_encoded,
            "different-server-id",
        )
        .unwrap();
        assert!(wrong_identity
            .health()
            .await
            .unwrap_err()
            .contains("服务身份与连接记录不一致"));
        server.abort();
    }

    #[tokio::test]
    async fn share_join_request_owner_approval_and_independent_token_work_end_to_end() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let owner_token = "host-owner-token-that-is-long-enough-0001";
        let owner = service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();
        assert_eq!(owner.scope, AccessScope::Owner);
        let app = router(service.clone());

        let mut create_share = json_request(
            Method::POST,
            "/api/v2/owner/shares",
            &ShareCreateRequest {
                endpoints: vec!["https://knowledge.internal:3210".to_string()],
                auto_approve_read: false,
                expires_in_hours: None,
            },
        );
        add_bearer(&mut create_share, owner_token);
        let created: ShareCreated =
            decode_body(app.clone().oneshot(create_share).await.unwrap()).await;
        let parsed = crate::client::parse_share(&created.share).unwrap();
        assert_eq!(parsed.server_id, service.server_info().unwrap().server_id);
        let remaining = created.expires_at - chrono::Utc::now().timestamp();
        assert!(((24 * 60 * 60 - 2)..=24 * 60 * 60).contains(&remaining));

        let first_credentials = crate::client::new_join_credentials();
        let first_request = join_request(
            "192.168.8.20:4567",
            "Alice",
            Some(&parsed.secret),
            &first_credentials,
        );
        let pending: JoinRequestReceipt =
            decode_body(app.clone().oneshot(first_request).await.unwrap()).await;
        assert_eq!(pending.request.status, JoinRequestStatus::Pending);
        assert_eq!(pending.request.scope, None);

        // A share link is multi-use: another device receives a separate request
        // and never shares Alice's eventual device credential.
        let second_credentials = crate::client::new_join_credentials();
        let second: JoinRequestReceipt = decode_body(
            app.clone()
                .oneshot(join_request(
                    "192.168.8.21:4567",
                    "Bob",
                    Some(&parsed.secret),
                    &second_credentials,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_ne!(pending.request.id, second.request.id);

        let mut approve = json_request(
            Method::POST,
            &format!("/api/v2/owner/join-requests/{}/approve", pending.request.id),
            &ResolveJoinRequest {
                scope: AccessScope::Read,
            },
        );
        add_bearer(&mut approve, owner_token);
        let approved: JoinRequestRecord =
            decode_body(app.clone().oneshot(approve).await.unwrap()).await;
        assert_eq!(approved.status, JoinRequestStatus::Approved);
        assert_eq!(approved.scope, Some(AccessScope::Read));

        let claimed: JoinRequestReceipt = decode_body(
            app.clone()
                .oneshot(join_claim_request(
                    "192.168.8.20:4567",
                    &pending.request.id,
                    &first_credentials.claim_secret,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(claimed.request.device_id, approved.device_id);

        let mut collections = Request::builder()
            .uri("/api/v1/collections")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut collections, &first_credentials.device_token);
        assert_eq!(
            app.clone().oneshot(collections).await.unwrap().status(),
            StatusCode::OK
        );

        let mut owner_only = Request::builder()
            .uri("/api/v2/owner/shares")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut owner_only, &first_credentials.device_token);
        assert_eq!(
            app.clone().oneshot(owner_only).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let mut stop = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/v2/owner/shares/{}", created.share_id))
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut stop, owner_token);
        assert_eq!(
            app.clone().oneshot(stop).await.unwrap().status(),
            StatusCode::OK
        );
        let third_credentials = crate::client::new_join_credentials();
        assert_eq!(
            app.oneshot(join_request(
                "192.168.8.22:4567",
                "Carol",
                Some(&parsed.secret),
                &third_credentials,
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn remote_owner_cannot_change_owner_roles_or_start_host_model_download() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let host_token = "host-owner-token-that-is-long-enough-0002";
        service.ensure_host_owner("Host", host_token).unwrap();
        let credentials = crate::client::new_join_credentials();
        let remote = service
            .submit_join_request(
                "192.168.1.30".parse().unwrap(),
                JoinRequestCreate {
                    device_name: "Remote owner".to_string(),
                    device_token_hash: credentials.device_token_hash.clone(),
                    claim_secret: credentials.claim_secret.clone(),
                    share_secret: None,
                },
            )
            .unwrap();
        let approved = service
            .approve_join_request(&remote.request.id, AccessScope::Manage)
            .unwrap();
        let remote_device_id = approved.device_id.unwrap();
        service.set_owner_device(&remote_device_id, true).unwrap();
        let app = router(service.clone());

        let mut demote = json_request(
            Method::PATCH,
            &format!("/api/v2/owner/devices/{remote_device_id}"),
            &UpdateDeviceRequest {
                name: None,
                scope: Some(AccessScope::Manage),
                revoked: None,
            },
        );
        add_bearer(&mut demote, host_token);
        assert_eq!(
            app.clone().oneshot(demote).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let mut delete = Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/v2/owner/devices/{remote_device_id}"))
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut delete, host_token);
        assert_eq!(
            app.clone().oneshot(delete).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let mut download = Request::builder()
            .method(Method::POST)
            .uri("/api/v2/owner/model")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut download, &credentials.device_token);
        assert_eq!(
            app.oneshot(download).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        assert!(!service.model_downloading());
    }

    #[test]
    fn join_request_source_filter_covers_private_tailnet_and_public_boundaries() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.0",
            "100.127.255.255",
            "::1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
            "::ffff:192.168.1.10",
        ] {
            assert!(is_private_network_ip(address.parse().unwrap()), "{address}");
        }
        for address in [
            "0.0.0.0",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.0",
            "2001:4860:4860::8888",
        ] {
            assert!(
                !is_private_network_ip(address.parse().unwrap()),
                "{address}"
            );
        }
    }

    #[test]
    fn join_request_host_filter_accepts_private_literals_and_internal_domains_only() {
        for host in [
            "192.168.1.20:3210",
            "100.64.12.34:3210",
            "[fd7a:115c:a1e0::1]:3210",
            "knowledge.internal:3210",
            "cube.tailnet-name.ts.net:3210",
            "localhost:3210",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("host", HeaderValue::from_str(host).unwrap());
            assert!(
                is_private_network_host(&Uri::from_static("/"), &headers),
                "{host}"
            );
        }
        for host in ["8.8.8.8:3210", "knowledge.example.com", "ts.net.evil.test"] {
            let mut headers = HeaderMap::new();
            headers.insert("host", HeaderValue::from_str(host).unwrap());
            assert!(
                !is_private_network_host(&Uri::from_static("/"), &headers),
                "{host}"
            );
        }
    }

    #[test]
    fn join_request_host_filter_accepts_http2_authority_without_host_header() {
        let headers = HeaderMap::new();
        let tailnet: Uri = "https://100.64.12.34:3210/api/v2/join-requests"
            .parse()
            .unwrap();
        assert!(is_private_network_host(&tailnet, &headers));

        let public: Uri = "https://8.8.8.8:3210/api/v2/join-requests".parse().unwrap();
        assert!(!is_private_network_host(&public, &headers));
    }

    #[tokio::test]
    async fn join_requests_are_rate_limited_per_source() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let app = router(service);

        for index in 0..6 {
            let credentials = crate::client::new_join_credentials();
            let response = app
                .clone()
                .oneshot(join_request(
                    "192.168.8.20:4567",
                    &format!("device-{index}"),
                    None,
                    &credentials,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        let credentials = crate::client::new_join_credentials();
        let limited = app
            .oneshot(join_request(
                "192.168.8.20:4567",
                "one-too-many",
                None,
                &credentials,
            ))
            .await
            .unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    fn json_request<T: Serialize>(method: Method, uri: &str, value: &T) -> Request<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(value).unwrap()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:45000".parse::<SocketAddr>().unwrap(),
        ));
        request
    }

    fn join_request(
        source: &str,
        device_name: &str,
        share_secret: Option<&str>,
        credentials: &crate::client::NewJoinCredentials,
    ) -> Request<Body> {
        let mut request = json_request(
            Method::POST,
            "/api/v2/join-requests",
            &JoinRequestCreate {
                device_name: device_name.to_string(),
                device_token_hash: credentials.device_token_hash.clone(),
                claim_secret: credentials.claim_secret.clone(),
                share_secret: share_secret.map(str::to_string),
            },
        );
        request
            .extensions_mut()
            .insert(ConnectInfo(source.parse::<SocketAddr>().unwrap()));
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_static("knowledge.internal:3210"));
        request
    }

    fn join_claim_request(source: &str, id: &str, claim_secret: &str) -> Request<Body> {
        let mut request = json_request(
            Method::POST,
            &format!("/api/v2/join-requests/{id}/status"),
            &JoinRequestClaim {
                claim_secret: claim_secret.to_string(),
            },
        );
        request
            .extensions_mut()
            .insert(ConnectInfo(source.parse::<SocketAddr>().unwrap()));
        request
            .headers_mut()
            .insert(HOST, HeaderValue::from_static("knowledge.internal:3210"));
        request
    }

    fn add_bearer(request: &mut Request<Body>, token: &str) {
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }

    async fn decode_body<T: serde::de::DeserializeOwned>(response: Response) -> T {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }
}
