use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router, middleware};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;

use crate::MAX_UPLOAD_BYTES;
use crate::model::*;
use crate::service::KnowledgeService;
use crate::store::DeviceMutationError;

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
            .trashed_collections_page(query.limit, query.offset)
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
            .trashed_documents_page(query.limit, query.offset)
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
    // 缺省 limit/offset 时 documents_page 以 LIMIT -1 返回全量,与非分页
    // 清单同一条路径,无需分支。
    let documents = service.documents_page(id, query.include_deleted, query.limit, query.offset);
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

/// Document-write errors keep the upload family's 400 contract, except the
/// stub-build local-embed refusal: that refusal is a platform capability
/// fact (the service can never index on this build), so it maps to 503 like
/// the search path instead of reading as a malformed request.
fn map_document_write_error(error: String) -> ApiError {
    map_document_write_error_for(crate::embedding::is_local_embed_unavailable(&error), error)
}

/// Capability bit as a parameter (same shape as the desktop twin's
/// `local_embed_download_refusal(supported)`): the host executes both
/// branches, while the production bit stays compile-time per target.
fn map_document_write_error_for(unavailable: bool, error: String) -> ApiError {
    if unavailable {
        ApiError::unavailable(error)
    } else {
        ApiError::bad_request(error)
    }
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
            .map_err(map_document_write_error)?,
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
    // 廉价校验先于装载门:空查询直接 200 []、超量知识集选择 400,不触发
    // 568MB 冷装载,也不因模型不可用把应答变成 503(与 boot 同步装载时代的
    // 行为一致)。
    if let Some(hits) =
        KnowledgeService::precheck_search(&request).map_err(ApiError::bad_request)?
    {
        return Ok(Json(hits));
    }
    // 首个检索请求触发模型装载(boot 不再同步加载 568MB ONNX)。装载必须
    // 在获取并发 slot 之前:568MB 冷读可能超过 slot 的 10s 超时,若先占 slot,
    // 排队第 5 个起的请求会在装载完成前拿到 503。先 ensure 则所有并发请求
    // 都在装载门上排队,装载完成后才竞争检索 slot。
    if !service.ready() {
        service
            .ensure_model_loaded()
            .await
            .map_err(ApiError::unavailable)?;
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
    // Join requests validate the directly-connected source, for which the
    // host's own loopback is a legitimate source (allow loopback).
    crate::net_policy::is_private_network_ip(address, true)
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
    crate::net_policy::PRIVATE_HOST_SUFFIXES
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
    use axum::http::Method;
    use axum::http::Request;
    use axum::http::header::{AUTHORIZATION, HOST};
    use http_body_util::BodyExt;
    use serde::Serialize;
    use tower::ServiceExt;

    #[test]
    fn document_write_errors_map_the_stub_refusal_to_unavailable() {
        // The stub refusal is a platform capability fact, not a malformed
        // request: it must reach the client as 503 (like the search path),
        // while ordinary document errors keep the family's 400 contract.
        assert_eq!(
            map_document_write_error_for(false, "boom".to_string()).status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            map_document_write_error_for(true, "stub".to_string()).status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        // Real-impl builds never produce the refusal (compile-time pin lives
        // on the loongarch64 side); the host direction of the predicate.
        assert!(!crate::embedding::is_local_embed_unavailable(
            "any parse error"
        ));
    }

    #[tokio::test]
    async fn info_is_public_but_collections_require_authentication() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let app = router(service);
        // /api/v1/info is the only public endpoint: the CLI --health-check
        // (deploy/install.sh) and the packaged host-helper probe both depend
        // on unauthenticated reachability; this test pins that too.
        let info = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(info.status(), StatusCode::OK);
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

    // 回收站清单 v2 路由必须真实可达:GET /api/v2/owner/trash/{collections,
    // documents} 返回 owner 作用域的清单,文档条目带所属知识集名;未认证请求
    // 保持 401。这与客户端 trashed_* 方法一一对应,防止路由再次被整段移除。
    #[tokio::test]
    async fn owner_trash_listings_serve_collections_and_documents() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let owner_token = "host-owner-token-that-is-long-enough-0004";
        service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();

        let active = service
            .create_collection(CreateCollectionRequest {
                name: "在用资料".to_string(),
                description: None,
            })
            .unwrap();
        let document = service
            .upload_document(active.id, "doc.txt", b"body".to_vec())
            .await
            .unwrap();
        service.trash_document(document.id).unwrap();

        let trashed = service
            .create_collection(CreateCollectionRequest {
                name: "待删集".to_string(),
                description: None,
            })
            .unwrap();
        service.trash_collection(trashed.id).unwrap();
        let app = router(service.clone());

        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v2/owner/trash/collections?limit=10&offset=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let mut collections = Request::builder()
            .method(Method::GET)
            .uri("/api/v2/owner/trash/collections?limit=10&offset=0")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut collections, owner_token);
        let response = app.clone().oneshot(collections).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let listed: Vec<Collection> = decode_body(response).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, trashed.id);

        let mut documents = Request::builder()
            .method(Method::GET)
            .uri("/api/v2/owner/trash/documents?limit=10&offset=0")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut documents, owner_token);
        let response = app.oneshot(documents).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let listed: Vec<TrashedDocument> = decode_body(response).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].document.id, document.id);
        assert_eq!(listed[0].collection_name, "在用资料");
    }

    // 省略 limit 的手动调用必须返回全量清单,而不是回落到某个静默截断的
    // 默认窗——响应没有总数字段,截断对调用方不可见;owner_devices 与
    // owner_join_requests 的约定都是 None=不限,回收站清单保持一致。
    // 201 条跨过旧的 unwrap_or(200) 默认窗:不带 limit → 全量;显式
    // limit=10 → 仍是单窗分页语义。突变对照:恢复 unwrap_or(200) 后,
    // 不带 limit 的断言恰好取到 200 ≠ 201 而落红。
    #[tokio::test]
    async fn trash_listing_without_limit_lists_everything() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let owner_token = "host-owner-token-that-is-long-enough-0008";
        service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();
        for index in 0..201 {
            let created = service
                .create_collection(CreateCollectionRequest {
                    name: format!("限界集 {index:03}"),
                    description: None,
                })
                .unwrap();
            service.trash_collection(created.id).unwrap();
        }
        let app = router(service.clone());

        let mut unbounded = Request::builder()
            .method(Method::GET)
            .uri("/api/v2/owner/trash/collections?offset=0")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut unbounded, owner_token);
        let response = app.clone().oneshot(unbounded).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let listed: Vec<Collection> = decode_body(response).await;
        assert_eq!(listed.len(), 201);

        let mut windowed = Request::builder()
            .method(Method::GET)
            .uri("/api/v2/owner/trash/collections?limit=10&offset=0")
            .body(Body::empty())
            .unwrap();
        add_bearer(&mut windowed, owner_token);
        let response = app.oneshot(windowed).await.unwrap();
        let window: Vec<Collection> = decode_body(response).await;
        assert_eq!(window.len(), 10);
    }

    // PUT /api/v1/collections/{id} 必须真实可达:owner 令牌完成全量替换
    // 改名(响应回读新名称与新描述),未认证保持 401。与回收站 v2 路由
    // 同理——这条 rename 链路曾被整段移除过,不能只靠 service 层测试保护
    // 路由表。突变对照:路由表去掉 .put(update_collection) 后,重命名腿
    // 以 405 落红。
    #[tokio::test]
    async fn update_collection_route_serves_rename() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let owner_token = "host-owner-token-that-is-long-enough-0009";
        service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();
        let collection = service
            .create_collection(CreateCollectionRequest {
                name: "旧名".to_string(),
                description: Some("原始描述".to_string()),
            })
            .unwrap();
        let app = router(service.clone());

        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/collections/{}", collection.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"新名"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let mut rename = Request::builder()
            .method(Method::PUT)
            .uri(format!("/api/v1/collections/{}", collection.id))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"新名","description":"替换描述"}"#))
            .unwrap();
        add_bearer(&mut rename, owner_token);
        let response = app.oneshot(rename).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let renamed: Collection = decode_body(response).await;
        assert_eq!(renamed.name, "新名");
        assert_eq!(renamed.description.as_deref(), Some("替换描述"));
    }

    // 客户端回收站清单必须翻页取全量:服务端按 offset 分页且不返回总数,
    // 首窗硬编码(limit=200)会静默截断,第 201 条起对用户不可见。清单与
    // 文档两条腿各 600 条,都跨过单页上限 500 验证聚合;顺序断言钉住
    // (deleted_at DESC, id DESC)——同一 fixture 里回收时间随 id 单调,
    // 聚合结果必须严格按 id 降序,翻页方向突变(ASC)或跨页重复都会在此
    // 落红。文档腿同时钉住 client.documents() 非分页清单(含/不含已删)。
    // 走真实回环 HTTP,与 KnowledgeClient 全链路一致。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trash_listing_clients_page_through_every_offset() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let owner_token = "host-owner-token-that-is-long-enough-0006";
        service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();

        let active = service
            .create_collection(CreateCollectionRequest {
                name: "在用资料".to_string(),
                description: None,
            })
            .unwrap();
        for index in 0..600 {
            let created = service
                .create_collection(CreateCollectionRequest {
                    name: format!("回收站 {index:03}"),
                    description: None,
                })
                .unwrap();
            service.trash_collection(created.id).unwrap();
        }
        for index in 0..600 {
            let document = service
                .upload_document(active.id, &format!("doc{index}.txt"), b"body".to_vec())
                .await
                .unwrap();
            service.trash_document(document.id).unwrap();
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            axum_server::from_tcp(listener)
                .unwrap()
                .serve(router(service).into_make_service_with_connect_info::<SocketAddr>()),
        );

        let client =
            crate::client::KnowledgeClient::new(&format!("http://{address}"), owner_token).unwrap();
        let listed = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.trashed_collections(),
        )
        .await
        .expect("回收站清单翻页不得挂死")
        .unwrap();
        assert_eq!(listed.len(), 600);
        assert!(
            listed.windows(2).all(|pair| pair[0].id > pair[1].id),
            "回收站清单跨页聚合必须保持 (deleted_at DESC, id DESC) 顺序"
        );
        let documents = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.trashed_documents(),
        )
        .await
        .expect("回收站文档清单翻页不得挂死")
        .unwrap();
        assert_eq!(documents.len(), 600);
        assert!(
            documents
                .windows(2)
                .all(|pair| pair[0].document.id > pair[1].document.id),
            "回收站文档清单跨页聚合必须保持 (deleted_at DESC, id DESC) 顺序"
        );
        let all_documents = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.documents(active.id, true),
        )
        .await
        .expect("非分页文档清单不得挂死")
        .unwrap();
        assert_eq!(all_documents.len(), 600);
        let live_documents = client.documents(active.id, false).await.unwrap();
        assert_eq!(live_documents.len(), 0);
    }

    // 恰好整页(500 = 单页上限)必须多取一页空结果收尾:终止条件退化成
    // `fetched <= 上限` 时永远只取首页(静默截断,由 600 条用例抓红),
    // offset 不按页长推进时窗口重叠或原地重取(突变实证两用例皆红);
    // 旧实现(200 首窗)则只返回 200 不足 500。外层 timeout 把病态
    // 循环兜底成测试失败而非套件挂起。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trash_listing_boundary_page_terminates_cleanly() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        let owner_token = "host-owner-token-that-is-long-enough-0007";
        service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();

        for index in 0..500 {
            let created = service
                .create_collection(CreateCollectionRequest {
                    name: format!("边界集 {index:03}"),
                    description: None,
                })
                .unwrap();
            service.trash_collection(created.id).unwrap();
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            axum_server::from_tcp(listener)
                .unwrap()
                .serve(router(service).into_make_service_with_connect_info::<SocketAddr>()),
        );

        let client =
            crate::client::KnowledgeClient::new(&format!("http://{address}"), owner_token).unwrap();
        let listed = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.trashed_collections(),
        )
        .await
        .expect("整页边界必须终止")
        .unwrap();
        assert_eq!(listed.len(), 500);
    }

    // 对端不按 offset 翻页(恒回整页)时,短页终止永不触发,循环会无界
    // 追加同一批条目直到内存耗尽。参考实现对 offset 有界,已由上面两用例
    // 覆盖;这里用不识别 offset 的最小桩路由(直接回固定整页)钉住页数
    // 上界:上界后的判别页仍是整页时必须快速报错而不是挂死或静默截断。
    // 报错文案对"清单超过约 5 万条上界"与"对端异常"两种可能都保留余地
    // (合法大清单在对端正常时也会走到这里),断言只钉"整页"关键词。
    // 突变对照:放宽 paged_all 的页数上界后,本用例被外层 timeout 兜底为
    // 失败;让判别页无条件收尾(不检查是否整页)则本用例的 unwrap_err
    // 直接落空。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trash_listing_peer_ignoring_offset_errors_at_page_cap() {
        async fn always_full_page() -> Json<Vec<Collection>> {
            let entry = Collection {
                id: 1,
                name: "病态对端".to_string(),
                description: None,
                status: "active".to_string(),
                doc_count: 0,
                chunk_count: 0,
                total_bytes: 0,
                created_at: 0,
                updated_at: 0,
                deleted_at: Some(0),
            };
            Json(vec![entry; crate::model::PAGE_LIMIT_CAP])
        }
        let app = Router::new().route("/api/v2/owner/trash/collections", get(always_full_page));

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            axum_server::from_tcp(listener)
                .unwrap()
                .serve(app.into_make_service_with_connect_info::<SocketAddr>()),
        );

        let client = crate::client::KnowledgeClient::new(
            &format!("http://{address}"),
            "stub-token-not-checked-by-the-fixtures",
        )
        .unwrap();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            client.trashed_collections(),
        )
        .await
        .expect("页数上界必须让病态对端快速失败而非挂死")
        .unwrap_err();
        assert!(error.contains("整页"), "实际错误:{error}");
    }

    // client.devices() 与其它无 _page 后缀的清单方法一样必须逐页取全量:
    // 旧实现硬编码 ?limit=200&offset=0 首窗,设备数超过 200 时其余设备
    // 静默不可见。这里用按 limit/offset 真实切片的桩路由(与真服务端
    // 相同的 clamp 语义)钉住 600 台设备跨页聚合与顺序;突变对照:
    // devices() 退回单窗 ?limit=200 时,本用例恰好取到 200 ≠ 600 而落红。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn devices_client_pages_through_every_offset() {
        #[derive(Deserialize)]
        struct StubPageQuery {
            limit: Option<usize>,
            #[serde(default)]
            offset: usize,
        }
        async fn paging_stub(
            State(items): State<std::sync::Arc<Vec<DeviceGrant>>>,
            Query(query): Query<StubPageQuery>,
        ) -> Json<Vec<DeviceGrant>> {
            let limit = query
                .limit
                .unwrap_or(200)
                .clamp(1, crate::model::PAGE_LIMIT_CAP);
            Json(
                items
                    .iter()
                    .skip(query.offset)
                    .take(limit)
                    .cloned()
                    .collect(),
            )
        }
        let devices: std::sync::Arc<Vec<DeviceGrant>> = std::sync::Arc::new(
            (0..600)
                .map(|index| DeviceGrant {
                    id: format!("device-{index:03}"),
                    name: format!("设备 {index:03}"),
                    scope: AccessScope::Read,
                    created_at: 0,
                    last_seen_at: None,
                    revoked: false,
                })
                .collect(),
        );
        let app = Router::new()
            .route("/api/v2/owner/devices", get(paging_stub))
            .with_state(devices);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(
            axum_server::from_tcp(listener)
                .unwrap()
                .serve(app.into_make_service_with_connect_info::<SocketAddr>()),
        );

        let client = crate::client::KnowledgeClient::new(
            &format!("http://{address}"),
            "stub-token-not-checked-by-the-fixtures",
        )
        .unwrap();
        let listed = tokio::time::timeout(std::time::Duration::from_secs(30), client.devices())
            .await
            .expect("设备清单翻页不得挂死")
            .unwrap();
        assert_eq!(listed.len(), 600);
        assert!(
            listed.windows(2).all(|pair| pair[0].id < pair[1].id),
            "设备清单跨页聚合不得重复或乱序"
        );
    }

    // 懒装载时代的廉价校验回归:空查询/超量知识集选择必须在模型装载门之前
    // 应答(200 [] / 400),冷服务(模型未装载且 model_dir 无模型文件)不得
    // 触发 568MB 冷读,也不得把本应 200/400 的请求变成 503。boot 同步装载
    // 时代这些请求从未碰到装载路径,本测试钉住该行为不被装载门回归。
    #[tokio::test]
    async fn cold_search_validates_request_before_model_load() {
        let root = tempfile::tempdir().unwrap();
        let service = KnowledgeService::boot(root.path().to_path_buf(), None)
            .unwrap()
            .service;
        assert!(!service.ready(), "空 tempdir 引导后模型不应就绪");
        let owner_token = "host-owner-token-that-is-long-enough-0002";
        service
            .ensure_host_owner("Host PINVOU", owner_token)
            .unwrap();
        let app = router(service.clone());

        let mut empty = json_request(
            Method::POST,
            "/api/v1/search",
            &SearchRequest {
                collection_ids: vec![],
                query: "   ".to_string(),
                limit: 10,
            },
        );
        add_bearer(&mut empty, owner_token);
        let response = app.clone().oneshot(empty).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let hits: Vec<SearchHit> = decode_body(response).await;
        assert!(hits.is_empty());
        assert!(
            !service.ready() && service.model_error().is_none(),
            "空查询不得触发模型装载或留下失败负缓存"
        );

        let mut oversized = json_request(
            Method::POST,
            "/api/v1/search",
            &SearchRequest {
                collection_ids: (0..=crate::service::MAX_SEARCH_COLLECTIONS as i64).collect(),
                query: "query".to_string(),
                limit: 10,
            },
        );
        add_bearer(&mut oversized, owner_token);
        let response = app.oneshot(oversized).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("知识集"));
        assert!(
            !service.ready() && service.model_error().is_none(),
            "无效请求不得触发模型装载"
        );
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
        assert!(
            wrong_identity
                .health()
                .await
                .unwrap_err()
                .contains("服务身份与连接记录不一致")
        );
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
