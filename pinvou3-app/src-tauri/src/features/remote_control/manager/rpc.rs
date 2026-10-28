//! RPC idempotency admission, response shaping, and the stream/event protocol
//! message builders consumed by the facade.
//!
//! Admission and cache helpers borrow `Inner` (or inspect it through `&Inner`)
//! and never lock the mutex themselves: the facade owns the guard and lends the
//! state, preserving the single-lock concurrency contract. Protocol message
//! builders are pure and stateless.

use std::collections::HashSet;
use std::time::Instant;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::relay_client::RelaySender;
use super::{
    Inner, RpcCacheEntry, RpcCompletion, RpcDispatch, RpcLedger, RpcTombstone, StreamEvent,
};
use crate::features::sessions::SessionStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EventSource {
    Rust,
    Frontend,
}

/// Bridge-generation replay actions. Also returned by
/// `prepare_bridge_generation`, which never needs the `None` variant.
pub(super) enum RpcRequestAction {
    None,
    Respond(RelaySender, Value),
    Dispatch(RpcDispatch),
}

pub(super) enum NewRpcAdmission {
    Rejected(RpcRejection),
    Durable(RpcLedger),
}

/// An admission rejection plus its cache classification. Deterministic
/// rejections depend only on the request's own args and the access policy, so
/// replaying the cached completion forever is always correct. Transient
/// rejections (`too_many_in_flight_requests`, an unavailable Session store or
/// ledger) describe desktop-side state at admission time: they are answered
/// once, never cached, so a retry of the same `client_request_id` re-runs
/// admission instead of replaying the stale rejection until FIFO eviction.
#[derive(Clone)]
pub(super) struct RpcRejection {
    pub(super) completion: RpcCompletion,
    pub(super) cacheable: bool,
}

impl RpcRejection {
    pub(super) fn deterministic(completion: RpcCompletion) -> Self {
        Self {
            completion,
            cacheable: true,
        }
    }

    pub(super) fn transient(completion: RpcCompletion) -> Self {
        Self {
            completion,
            cacheable: false,
        }
    }
}

/// Session-scope validation outcome. `Deterministic` violations follow from
/// the request args or policy alone; `Transient` ones (Session store not
/// ready, Session temporarily unreadable) may resolve on a later retry and
/// must therefore never be cached as terminal completions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WebScopeRejection {
    Deterministic(String),
    Transient(String),
}

impl WebScopeRejection {
    fn message(self) -> String {
        match self {
            Self::Deterministic(message) | Self::Transient(message) => message,
        }
    }

    fn cacheable(&self) -> bool {
        matches!(self, Self::Deterministic(_))
    }

    pub(super) fn into_rpc_rejection(self) -> RpcRejection {
        let cacheable = self.cacheable();
        RpcRejection {
            completion: rpc_error_completion("invalid_session_scope", self.message()),
            cacheable,
        }
    }
}

pub(super) fn rpc_admission_rejection(
    preflight_rejection: Option<&RpcRejection>,
    allowed: bool,
    in_flight_count: usize,
) -> Option<RpcRejection> {
    if let Some(rejection) = preflight_rejection {
        Some(rejection.clone())
    } else if !allowed {
        Some(RpcRejection::deterministic(rpc_error_completion(
            "command_not_allowed",
            "远程控制不允许调用该命令",
        )))
    } else if in_flight_count >= super::MAX_RPC_IN_FLIGHT {
        Some(RpcRejection::transient(rpc_error_completion(
            "too_many_in_flight_requests",
            "正在运行的远程控制请求过多",
        )))
    } else {
        None
    }
}

pub(super) fn prepare_new_rpc_admission(
    ledger: &RpcLedger,
    request_id: &str,
    fingerprint: &str,
    preflight_rejection: Option<&RpcRejection>,
    allowed: bool,
    in_flight_count: usize,
) -> Result<NewRpcAdmission, String> {
    if let Some(rejection) = rpc_admission_rejection(preflight_rejection, allowed, in_flight_count)
    {
        // Deterministic rejections are side-effect free and may be replayed
        // from the bounded in-memory cache; transient ones must not (see
        // `RpcRejection`). Neither kind ever consumes a durable idempotency
        // slot or evicts an acknowledged request tombstone.
        return Ok(NewRpcAdmission::Rejected(rejection));
    }
    let mut next_ledger = ledger.clone();
    next_ledger.remember(request_id, fingerprint)?;
    Ok(NewRpcAdmission::Durable(next_ledger))
}

pub(super) fn prepare_bridge_generation(
    inner: &mut Inner,
    generation: &str,
) -> Vec<RpcRequestAction> {
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
                actions.push(RpcRequestAction::Respond(
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
        actions.push(RpcRequestAction::Dispatch(RpcDispatch {
            request_id,
            command,
            args,
            bridge_generation: generation.to_string(),
        }));
    }
    actions
}

pub(super) fn prune_rpc_cache(inner: &mut Inner) {
    while inner.rpc_cache.len() > super::RPC_CACHE_CAPACITY
        || rpc_cache_bytes(inner) > super::RPC_CACHE_BYTES_CAPACITY
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

pub(super) fn rpc_cache_bytes(inner: &Inner) -> usize {
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
                                .unwrap_or(super::MAX_RPC_REQUEST_BYTES)
                    }
                    RpcCacheEntry::Complete {
                        fingerprint,
                        completion,
                    } => {
                        fingerprint.len()
                            + serde_json::to_vec(&completion.result)
                                .map(|encoded| encoded.len())
                                .unwrap_or(super::MAX_RPC_RESPONSE_BYTES)
                            + completion.error.as_ref().map_or(0, String::len)
                            + completion.error_code.as_ref().map_or(0, String::len)
                    }
                }
        })
        .sum()
}

pub(super) fn rpc_response(
    endpoint_id: &str,
    lease_id: &str,
    request_id: &str,
    completion: &RpcCompletion,
) -> Value {
    json!({
        "v": super::PROTOCOL_VERSION,
        "type": "rpc_response",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "id": request_id,
        "client_request_id": request_id,
        "ok": completion.ok,
        "result": completion.result,
        "error": completion.error,
        "error_code": completion.error_code,
        "error_category": completion.error_category,
    })
}

pub(super) fn rpc_error_completion(
    code: impl Into<String>,
    error: impl Into<String>,
) -> RpcCompletion {
    RpcCompletion {
        ok: false,
        result: Value::Null,
        error: Some(error.into().chars().take(16_384).collect()),
        error_code: Some(code.into()),
        error_category: None,
    }
}

/// `error_code`/`error_category` carry the desktop command's structured
/// `VoiceCommandError` identity to the browser lane, whose RPC proxy only
/// forwards the (Chinese) message text; without them the web lane's
/// `normalizeVoiceError` code mapping is unreachable for Rust errors.
pub(super) fn bounded_rpc_completion(
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
    error_code: Option<String>,
    error_category: Option<String>,
) -> RpcCompletion {
    let result = result.unwrap_or(Value::Null);
    let response_bytes = serde_json::to_vec(&result)
        .map(|encoded| encoded.len())
        .unwrap_or(usize::MAX);
    if response_bytes > super::MAX_RPC_RESPONSE_BYTES {
        return rpc_error_completion(
            "response_too_large",
            format!(
                "远程控制响应超过 {} MiB 上限，请使用分页或分块命令",
                super::MAX_RPC_RESPONSE_BYTES / (1024 * 1024)
            ),
        );
    }
    RpcCompletion {
        ok,
        result,
        error: error.map(|message| message.chars().take(16_384).collect()),
        error_code,
        error_category,
    }
}

pub(super) fn outcome_unknown_completion() -> RpcCompletion {
    rpc_error_completion(
        "outcome_unknown",
        "the desktop may have started this request before its result was durably observed; it will not be run again",
    )
}

pub(super) fn request_conflict_completion() -> RpcCompletion {
    rpc_error_completion(
        "request_id_conflict",
        "client_request_id was reused with different command or args",
    )
}

pub(super) fn tombstone_completion(tombstone: &RpcTombstone, fingerprint: &str) -> RpcCompletion {
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

/// Whether a ledger-tombstone replay completion may enter the bounded response
/// cache. Only the acknowledged outcome for the tombstone's own fingerprint is
/// terminal-and-cacheable. A fingerprint conflict must be recomputed on every
/// attempt: cached under the tombstone fingerprint it would replay the probe's
/// conflict to the legitimate owner after restart, and cached under the
/// incoming fingerprint the `Complete`-mismatch arm would preempt the owner's
/// re-dispatch. The conflict stays deterministic because the tombstone is
/// durable, so recomputing it per attempt is exact.
pub(super) fn tombstone_completion_cacheable(tombstone: &RpcTombstone, fingerprint: &str) -> bool {
    tombstone.fingerprint == fingerprint && tombstone.acknowledged
}

pub(super) fn rpc_fingerprint(command: &str, args: &Value) -> Result<String, String> {
    let canonical = canonicalize_json(args);
    let encoded = serde_json::to_vec(&(command, canonical))
        .map_err(|error| format!("serialize Web RPC fingerprint: {error}"))?;
    Ok(crate::platform::hashing::sha256_hex(&encoded))
}

pub(super) fn canonicalize_json(value: &Value) -> Value {
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
enum WebWorkspaceRpcPolicy {
    HostFileBrowse,
    CreateWithOptionalGrant,
    SessionBoundRead,
}

fn web_workspace_rpc_policy(command: &str) -> Option<WebWorkspaceRpcPolicy> {
    match command {
        "web_access_list_host_files" => Some(WebWorkspaceRpcPolicy::HostFileBrowse),
        "web_access_create_codex_acp_session" => {
            Some(WebWorkspaceRpcPolicy::CreateWithOptionalGrant)
        }
        "web_access_list_codex_workspace"
        | "web_access_search_codex_workspace"
        | "web_access_preview_codex_workspace_file"
        | "web_access_get_codex_workspace_changes"
        | "web_access_get_codex_workspace_diff" => Some(WebWorkspaceRpcPolicy::SessionBoundRead),
        _ => None,
    }
}

fn validate_web_workspace_rpc(command: &str, args: &Value) -> Result<(), String> {
    // Only reached for commands present in the Web access policy, so native
    // workspace commands never get here; the policy gate rejects them first.
    let Some(policy) = web_workspace_rpc_policy(command) else {
        return Ok(());
    };
    if args.get("workspacePath").is_some() || args.get("workspace_path").is_some() {
        return Err(format!("{command} does not accept a native workspace path"));
    }
    if policy == WebWorkspaceRpcPolicy::HostFileBrowse {
        for field in ["issueWorkspaceHandle", "issue_workspace_handle"] {
            if args.get(field).is_some_and(|value| !value.is_boolean()) {
                return Err(format!("{field} must be a boolean"));
            }
        }
        return Ok(());
    }
    if policy == WebWorkspaceRpcPolicy::CreateWithOptionalGrant {
        let Some(handle) = args.get("workspaceHandle") else {
            return Ok(());
        };
        if handle.is_null() {
            return Ok(());
        }
        let Some(handle) = handle.as_str() else {
            return Err("workspaceHandle must be a string or null".to_string());
        };
        super::workspace_grants::validate_handle(handle)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WebSessionScope {
    Required(&'static str),
    Optional(&'static str),
}

/// Web lane path validation for the knowledge-base family. Unlike every
/// other web-reachable file entry point, `kb_collection_add_sources` and
/// `kb_start_scan` take raw host paths that the desktop UI sources from its
/// own picker, but a paired phone supplies them directly — without this
/// gate the token holder could index (and then read out via kb_documents)
/// arbitrary disk trees. Every path must clear the same browsable policy as
/// `web_access_list_host_files`: $HOME subtree minus credential components.
/// 上限防滥用:1 MiB 请求体可塞下数万条路径,而每条都要过一次触盘的
/// canonicalize;逐条准入前先按条数做确定性的形状拒绝。
const MAX_WEB_KB_PATHS: usize = 128;

/// Rejection messages stay path-free; the native detail is log-only. The
/// cache classification follows the module-head criterion: arg-shape
/// violations depend only on the request itself and stay deterministic
/// (cacheable), while `validate_browsable_path` touches the filesystem
/// (`canonicalize` + `metadata`), so a not-yet-existing path or a
/// temporarily unavailable volume is a TRANSIENT rejection — caching it as
/// a terminal completion would brick that `client_request_id` until FIFO
/// eviction, exactly the poisoning class this module exists to prevent.
/// `web_access_codex_acp_prompt` 的 workspace_references 条数上限:每条引用
/// 都要触盘 canonicalize(与 `MAX_WEB_KB_PATHS` 同一成本类),1 MiB 请求体
/// 可塞下数万条;超限按确定性形状拒绝。
const MAX_WEB_WORKSPACE_REFERENCES: usize = 128;

fn validate_web_prompt_references(command: &str, args: &Value) -> Result<(), WebScopeRejection> {
    if command != "web_access_codex_acp_prompt" {
        return Ok(());
    }
    // 手机侧经 Tauri 参数绑定,两个字段拼写都可能出现;预算按两种拼写的
    // 条目总数计——恶意帧可以同时携带两者,独立上限等于把同一请求的
    // canonicalize 预算翻倍。
    let mut reference_count = 0usize;
    for field in ["workspaceReferences", "workspace_references"] {
        let Some(references) = args.get(field).filter(|value| !value.is_null()) else {
            continue;
        };
        let Some(references) = references.as_array() else {
            return Err(WebScopeRejection::Deterministic(format!(
                "{field} must be an array of paths"
            )));
        };
        reference_count += references.len();
        if reference_count > MAX_WEB_WORKSPACE_REFERENCES {
            return Err(WebScopeRejection::Deterministic(format!(
                "web_access_codex_acp_prompt accepts at most {MAX_WEB_WORKSPACE_REFERENCES} workspace references"
            )));
        }
    }
    Ok(())
}

fn validate_web_kb_paths(command: &str, args: &Value) -> Result<(), WebScopeRejection> {
    let paths_field = match command {
        "kb_collection_add_sources" => "paths",
        "kb_start_scan" => "roots",
        _ => return Ok(()),
    };
    let Some(paths) = args.get(paths_field).filter(|value| !value.is_null()) else {
        // kb_start_scan without roots (absent or null — Tauri maps both to
        // Option::None) scans the home default — the same dataset the
        // desktop's own auto-scan covers.
        if command == "kb_start_scan" {
            return Ok(());
        }
        return Err(WebScopeRejection::Deterministic(format!(
            "{command} requires {paths_field}"
        )));
    };
    let Some(paths) = paths.as_array() else {
        return Err(WebScopeRejection::Deterministic(format!(
            "{paths_field} must be an array of paths"
        )));
    };
    if paths.len() > MAX_WEB_KB_PATHS {
        return Err(WebScopeRejection::Deterministic(format!(
            "{command} accepts at most {MAX_WEB_KB_PATHS} {paths_field} entries"
        )));
    }
    for path in paths {
        let Some(path) = path.as_str() else {
            return Err(WebScopeRejection::Deterministic(format!(
                "{paths_field} entries must be strings"
            )));
        };
        // 相对路径与 validate_browsable_path 内部的同语义检查一致地先挡:
        // 它是只依赖请求本身的形状违规,按确定性(可缓存)拒绝,不必为它
        // 触盘,也不会被错误地归入瞬时类反复重跑。
        if !std::path::Path::new(path).is_absolute() {
            return Err(WebScopeRejection::Deterministic(format!(
                "{paths_field} entries must be absolute paths"
            )));
        }
        if crate::features::files::file_ingest::validate_browsable_path(path).is_err() {
            log::warn!("[remote_control] rejected non-browsable kb path over Web");
            return Err(WebScopeRejection::Transient(format!(
                "{command} may only index browsable paths under the home directory"
            )));
        }
    }
    Ok(())
}

pub(super) fn web_session_scope(command: &str) -> Option<WebSessionScope> {
    use WebSessionScope::{Optional, Required};
    let scope = match command {
        // Commands whose Rust API historically falls back to the desktop
        // process-wide active Session must be explicit over WebUI.
        "cancel_generation"
        | "cancel_user_input"
        | "compact_now"
        | "confirm_pending_memory"
        | "delete_memory_preference"
        | "delete_timed_memory"
        | "delete_work_context_memory"
        | "edit_last_turn"
        | "get_memory_overview"
        | "ignore_pending_memory"
        | "never_pending_memory"
        | "submit_user_input"
        | "summon_pinvou"
        | "update_memory_profile"
        | "update_memory_preference"
        | "update_timed_memory"
        | "update_work_context_memory" => Required("sessionId"),

        "accept_plan"
        // `cancel_codex_acp` (desktop-only) is deliberately absent: the Web
        // access policy's command set never contains it (the browser uses
        // `web_access_cancel_codex_acp`), so the RPC admission gate rejects the
        // bare command before this scope table is consulted.
        | "cancel_shell_task"
        | "discard_plan"
        | "equip_persona"
        | "exit_plan_to_yolo"
        | "get_active_persona"
        | "get_mode_state"
        | "get_or_create_aux_session"
        | "reset_aux_session"
        | "get_session_model_id"
        | "get_session_persona_events"
        | "get_session_pinvou_reviews"
        | "get_session_pinvou_scene_events"
        | "get_session_timeline"
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
        | "session_unmount_collection"
        | "set_plan_mode_next"
        | "set_session_model"
        | "unequip_persona"
        | "web_access_artifact_info"
        | "web_access_chat"
        | "web_access_codex_acp_prompt"
        | "web_access_cancel_codex_acp"
        | "web_access_get_codex_acp_pending_elicitations"
        | "web_access_get_codex_acp_pending_permissions"
        | "web_access_respond_codex_acp_permission"
        | "web_access_get_codex_acp_session_info"
        | "web_access_get_codex_acp_timeline"
        | "web_access_respond_codex_acp_elicitation"
        | "web_access_get_codex_workspace_changes"
        | "web_access_get_codex_workspace_diff"
        | "web_access_list_codex_workspace"
        | "web_access_preview_codex_workspace_file"
        | "web_access_read_artifact_chunk"
        | "web_access_read_artifact_image_b64"
        | "web_access_read_artifact_text"
        | "web_access_read_artifact_thumbnail"
        | "web_access_render_artifact_visual"
        | "web_access_read_conversation_attachment_chunk"
        | "web_access_search_codex_workspace"
        | "web_access_set_codex_acp_config_option"
        | "web_access_set_codex_acp_mode"
        | "web_access_set_codex_acp_model"
        | "web_access_transcribe_voice_audio"
        | "web_access_write_artifact_text" => Required("sessionId"),

        "delete_session"
        | "rename_session"
        | "save_session_artifacts"
        | "set_session_archived"
        | "set_session_pinned"
        | "web_access_cancel_session_download"
        | "web_access_load_session_chunk" => Required("id"),

        // Omitting these deliberately uses the global/default behavior without
        // consulting the desktop active pointer.
        "get_effective_model_config" => Optional("sessionId"),
        _ => return None,
    };
    Some(scope)
}

pub(super) fn validate_web_rpc_scope(
    app: &AppHandle,
    command: &str,
    args: &Value,
) -> Result<(), WebScopeRejection> {
    validate_web_workspace_rpc(command, args)
        .map_err(|error| WebScopeRejection::Deterministic(error))?;
    validate_web_prompt_references(command, args)?;
    validate_web_kb_paths(command, args)?;
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
            Err(WebScopeRejection::Deterministic(format!(
                "{command} requires an explicit {field}"
            )))
        } else {
            Ok(())
        };
    };
    if let Err(error) = crate::features::sessions::validate_session_id(session_id) {
        log::warn!("[remote_control] rejected invalid Web Session id: {error:#}");
        return Err(WebScopeRejection::Deterministic(
            "invalid_web_session_id".to_string(),
        ));
    }
    super::validate_multi_agent_session_web_scope(app, command, session_id)
        .map_err(WebScopeRejection::Deterministic)?;
    if command == "web_access_cancel_session_download"
        || (command == "web_access_load_session_chunk"
            && args
                .get("downloadId")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()))
    {
        // The opaque transfer token is already bound to the validated
        // Session id in RemoteControlManager; avoid re-reading a large Session
        // file for every 256 KiB chunk.
        return Ok(());
    }
    let store = app
        .try_state::<SessionStore>()
        .ok_or_else(|| WebScopeRejection::Transient("Session store is not ready".to_string()))?;
    store.load(session_id).map_err(|error| {
        // 读取失败可能是暂时的(存储尚未就绪/文件暂时不可读):拒绝必须能被
        // 同一 client_request_id 重试,所以归类为 Transient 而非确定性非法。
        log::warn!("[remote_control] Web Session scope lookup failed: {error:#}");
        WebScopeRejection::Transient("web_session_unavailable".to_string())
    })?;
    Ok(())
}

pub(super) fn validate_bridge_generation(generation: &str) -> Result<(), String> {
    let generation = generation.trim();
    if !crate::features::files::attachment_upload::validate_opaque_token(generation, 8, 256, None) {
        return Err("invalid WebView bridge generation".to_string());
    }
    Ok(())
}

pub(super) fn validate_rpc_command(command: &str) -> Result<(), String> {
    if command.is_empty() || command.len() > super::MAX_RPC_COMMAND_BYTES {
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

pub(super) fn event_message(
    endpoint_id: &str,
    lease_id: &str,
    stream_epoch: &str,
    event: &StreamEvent,
) -> Value {
    json!({
        "v": super::PROTOCOL_VERSION,
        "type": "event",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "stream_epoch": stream_epoch,
        "seq": event.seq,
        "event": event.event,
        "payload": event.payload,
    })
}

#[derive(Clone, Copy)]
pub(super) struct ReplayMessageContext<'a> {
    pub(super) endpoint_id: &'a str,
    pub(super) lease_id: &'a str,
    pub(super) stream_epoch: &'a str,
    pub(super) capability_commands: &'a [String],
    pub(super) capability_events: &'a [String],
}

pub(super) fn subscription_filtered_replay_messages(
    events: Vec<StreamEvent>,
    subscriptions: &HashSet<String>,
    context: ReplayMessageContext<'_>,
) -> Vec<Value> {
    let mut messages = Vec::with_capacity(events.len());
    let mut skipped_through = None;
    for event in events {
        if subscriptions.contains(&event.event) {
            if let Some(seq) = skipped_through.take() {
                messages.push(snapshot_message(
                    context.endpoint_id,
                    context.lease_id,
                    context.stream_epoch,
                    seq,
                    context.capability_commands,
                    context.capability_events,
                ));
            }
            messages.push(event_message(
                context.endpoint_id,
                context.lease_id,
                context.stream_epoch,
                &event,
            ));
        } else {
            skipped_through = Some(event.seq);
        }
    }
    if let Some(seq) = skipped_through {
        messages.push(snapshot_message(
            context.endpoint_id,
            context.lease_id,
            context.stream_epoch,
            seq,
            context.capability_commands,
            context.capability_events,
        ));
    }
    messages
}

pub(super) fn stream_reset_message(
    endpoint_id: &str,
    lease_id: &str,
    stream_epoch: &str,
    reason: &str,
) -> Value {
    json!({
        "v": super::PROTOCOL_VERSION,
        "type": "stream_reset",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "stream_epoch": stream_epoch,
        "seq": 0,
        "reason": reason,
    })
}

pub(super) fn rpc_in_flight_expired(dispatched_at: Instant, now: Instant) -> bool {
    now.duration_since(dispatched_at) > super::RPC_IN_FLIGHT_TTL
}

pub(super) fn enqueue_stream_reset(sender: RelaySender, message: Value) {
    // RelaySender owns a single bounded waiter and coalesces repeated recovery
    // barriers to the latest lease/epoch while the data channel is saturated.
    if sender.enqueue_stream_reset(message).is_err() {
        eprintln!("[web-access] stream reset could not reach the relay task");
    }
}

pub(super) fn try_enqueue_message_batch(
    messages: Vec<Value>,
    mut enqueue: impl FnMut(Value) -> bool,
) -> bool {
    for message in messages {
        if !enqueue(message) {
            return false;
        }
    }
    true
}

pub(super) fn snapshot_message(
    endpoint_id: &str,
    lease_id: &str,
    epoch: &str,
    seq: u64,
    commands: &[String],
    events: &[String],
) -> Value {
    json!({
        "v": super::PROTOCOL_VERSION,
        "type": "desktop_snapshot",
        "endpoint_id": endpoint_id,
        "lease_id": lease_id,
        "stream_epoch": epoch,
        "seq": seq,
        "snapshot": {
            "desktop_connected": true,
            "server_time": chrono::Utc::now().to_rfc3339(),
            "backend_version": env!("CARGO_PKG_VERSION"),
            "capabilities": {
                "protocol_version": super::PROTOCOL_VERSION,
                "commands": commands,
                "events": events,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::super::MAX_RPC_IN_FLIGHT;
    use super::*;

    #[test]
    fn in_flight_saturation_is_a_transient_uncacheable_rejection() {
        let rejection = rpc_admission_rejection(None, true, MAX_RPC_IN_FLIGHT)
            .expect("a saturated in-flight count must reject");
        assert_eq!(
            rejection.completion.error_code.as_deref(),
            Some("too_many_in_flight_requests")
        );
        assert!(
            !rejection.cacheable,
            "an in-flight rejection depends on desktop congestion, not on the \
             request; caching it would replay the stale rejection for every \
             retry of the same client_request_id"
        );
        // 槽位空出来后,同样的请求必须能重新准入而不是命中缓存拒绝。
        assert!(rpc_admission_rejection(None, true, MAX_RPC_IN_FLIGHT - 1).is_none());
    }

    #[test]
    fn deterministic_rejections_remain_cacheable() {
        let too_large =
            RpcRejection::deterministic(rpc_error_completion("request_too_large", "too large"));
        assert!(too_large.cacheable);

        let not_allowed =
            rpc_admission_rejection(None, false, 0).expect("a disallowed command must reject");
        assert_eq!(
            not_allowed.completion.error_code.as_deref(),
            Some("command_not_allowed")
        );
        assert!(not_allowed.cacheable);
    }

    #[test]
    fn web_prompt_references_are_capped_deterministically() {
        // 129 条引用超出上限:确定性(可缓存)形状拒绝,消息不回显路径。
        let many: Vec<String> = (0..129).map(|index| format!("file-{index}.md")).collect();
        for field in ["workspaceReferences", "workspace_references"] {
            let rejection = validate_web_prompt_references(
                "web_access_codex_acp_prompt",
                &json!({ field: many }),
            )
            .err()
            .expect("over-cap references must be rejected");
            let message = match &rejection {
                WebScopeRejection::Deterministic(message)
                | WebScopeRejection::Transient(message) => message,
            };
            assert!(
                matches!(rejection, WebScopeRejection::Deterministic(_)),
                "shape violations stay deterministic, got {rejection:?}"
            );
            assert!(!message.contains("file-"));
        }
        // 恰在上限内、缺省、null、非本命令一律放行。
        let at_cap: Vec<String> = (0..128).map(|index| format!("file-{index}.md")).collect();
        assert!(
            validate_web_prompt_references(
                "web_access_codex_acp_prompt",
                &json!({ "workspaceReferences": at_cap })
            )
            .is_ok()
        );
        assert!(validate_web_prompt_references("web_access_codex_acp_prompt", &json!({})).is_ok());
        assert!(
            validate_web_prompt_references(
                "web_access_codex_acp_prompt",
                &json!({ "workspaceReferences": null })
            )
            .is_ok()
        );
        assert!(
            validate_web_prompt_references(
                "web_access_chat",
                &json!({ "workspaceReferences": many })
            )
            .is_ok()
        );
        // 非数组条目也是确定性形状拒绝。
        assert!(matches!(
            validate_web_prompt_references(
                "web_access_codex_acp_prompt",
                &json!({ "workspaceReferences": "x" })
            ),
            Err(WebScopeRejection::Deterministic(_))
        ));
    }

    #[test]
    fn web_kb_paths_require_browsable_home_paths() {
        // 无路径字段的知识库命令不受影响;kb_start_scan 缺省 roots 扫描
        // 主目录(桌面自动扫描同一数据集),放行。
        assert!(validate_web_kb_paths("kb_search", &json!({ "query": "x" })).is_ok());
        assert!(validate_web_kb_paths("kb_start_scan", &json!({})).is_ok());
        assert!(validate_web_kb_paths("kb_start_scan", &json!({ "roots": null })).is_ok());

        // add_sources 必须携带 paths,且每个条目都要过可浏览策略。
        assert!(validate_web_kb_paths("kb_collection_add_sources", &json!({})).is_err());
        assert!(
            validate_web_kb_paths("kb_collection_add_sources", &json!({ "paths": "x" })).is_err()
        );
        assert!(
            validate_web_kb_paths("kb_collection_add_sources", &json!({ "paths": [42] })).is_err()
        );
        assert!(
            validate_web_kb_paths("kb_start_scan", &json!({ "roots": ["relative/path"] })).is_err()
        );
        // 绝对但越出 $HOME(且真实存在)的路径必须拒绝;拒绝消息不得回显路径。
        // 触盘校验失败归类 Transient(路径尚不存在/卷暂不可用都可能恢复),
        // 不能缓存成终态——否则同 client_request_id 的重试永远重放这次失败,
        // 正是本模块要消灭的毒化类。两种平台拼写都测:任一平台上,另一平台
        // 的绝对路径都无法落在 $HOME 下;它在本平台按相对形状归类为确定性
        // 形状拒绝(与文件系统无关),本平台的绝对拼写才走触盘的瞬时类。
        for outside in ["/etc/hosts", r"C:\Windows\notepad.exe"] {
            let rejection =
                validate_web_kb_paths("kb_collection_add_sources", &json!({ "paths": [outside] }))
                    .expect_err("a path outside $HOME must not be indexable over Web");
            if std::path::Path::new(outside).is_absolute() {
                assert!(
                    matches!(rejection, WebScopeRejection::Transient(_)),
                    "filesystem-backed rejections must be transient, got {rejection:?}"
                );
            } else {
                assert!(
                    matches!(rejection, WebScopeRejection::Deterministic(_)),
                    "relative-shaped spellings must stay deterministic, got {rejection:?}"
                );
            }
            let message = rejection.message();
            assert!(
                !message.contains(outside),
                "rejection must not echo the path: {message}"
            );
        }

        // 相对路径是纯形状违规:按确定性(可缓存)拒绝,不归入瞬时类。
        let rejection = validate_web_kb_paths(
            "kb_start_scan",
            &json!({ "roots": ["relative/path"] }),
        )
        .expect_err("relative roots must be rejected");
        assert!(
            matches!(rejection, WebScopeRejection::Deterministic(_)),
            "arg-shape violations must stay deterministic, got {rejection:?}"
        );

        // 形状违规只依赖请求参数,保持确定性(可缓存)。
        for shape in [json!({}), json!({ "paths": "x" }), json!({ "paths": [42] })] {
            let rejection = validate_web_kb_paths("kb_collection_add_sources", &shape)
                .expect_err("arg-shape violations must be deterministic");
            assert!(
                matches!(rejection, WebScopeRejection::Deterministic(_)),
                "arg-shape rejections must stay deterministic, got {rejection:?}"
            );
        }

        // 条数上限:确定性拒绝。条目故意用相对路径——触盘校验对它们的拒绝
        // 会归类 Transient,这里的 Deterministic 恰好证明拒绝来自条数上限
        // (上限检查先于逐条触盘)而非路径校验。
        let flood: Vec<String> = (0..MAX_WEB_KB_PATHS + 1)
            .map(|i| format!("docs/{i}.txt"))
            .collect();
        let rejection =
            validate_web_kb_paths("kb_collection_add_sources", &json!({ "paths": flood }))
                .expect_err("over-cap path lists must be rejected");
        assert!(matches!(rejection, WebScopeRejection::Deterministic(_)));
    }

    #[test]
    fn session_scope_rejections_split_deterministic_from_transient() {
        assert!(
            WebScopeRejection::Deterministic(
                "web_access_chat requires an explicit sessionId".into()
            )
            .cacheable()
        );
        assert!(!WebScopeRejection::Transient("Session store is not ready".into()).cacheable());

        let deterministic =
            WebScopeRejection::Deterministic("missing sessionId".into()).into_rpc_rejection();
        assert_eq!(
            deterministic.completion.error_code.as_deref(),
            Some("invalid_session_scope")
        );
        assert!(deterministic.cacheable);

        let transient =
            WebScopeRejection::Transient("web_session_unavailable".into()).into_rpc_rejection();
        assert_eq!(
            transient.completion.error_code.as_deref(),
            Some("invalid_session_scope")
        );
        assert!(
            !transient.cacheable,
            "a store-not-ready rejection must reach the browser but never the \
             idempotency cache"
        );
    }

    #[test]
    fn transient_rejections_never_consume_durable_tombstone_capacity() {
        let ledger = RpcLedger::for_endpoint("ep_test");
        let fingerprint = rpc_fingerprint("chat", &json!({ "sessionId": "s1" })).unwrap();
        let rejection =
            rpc_admission_rejection(None, true, MAX_RPC_IN_FLIGHT).expect("saturated admission");
        let admission = prepare_new_rpc_admission(
            &ledger,
            "request-transient",
            &fingerprint,
            Some(&rejection),
            true,
            MAX_RPC_IN_FLIGHT,
        )
        .expect("classify the saturated admission");
        let NewRpcAdmission::Rejected(rejection) = admission else {
            panic!("a saturated admission must reject");
        };
        assert!(!rejection.cacheable);
        assert!(
            ledger.get("request-transient").is_none(),
            "transient rejections must not consume a durable tombstone"
        );
    }

    #[test]
    fn tombstone_conflicts_are_answered_but_never_cached() {
        let owner_fingerprint = rpc_fingerprint("chat", &json!({ "sessionId": "s1" })).unwrap();
        let probe_fingerprint = rpc_fingerprint("chat", &json!({ "sessionId": "s2" })).unwrap();
        let mut tombstone = RpcTombstone {
            request_id: "request-1".into(),
            fingerprint: owner_fingerprint.clone(),
            acknowledged: false,
        };
        // 指纹一致且未确认 → 走重建 pending 的重派发路径,不是终态完成。
        assert!(!tombstone_completion_cacheable(
            &tombstone,
            &owner_fingerprint
        ));
        // 指纹冲突 → 回复冲突完成,但不可缓存:缓存到墓碑指纹下会在重启后
        // 把探针的冲突重放给合法指纹,缓存到来方指纹下又会经 Complete 分支
        // 抢占合法指纹的重派发。
        assert!(!tombstone_completion_cacheable(
            &tombstone,
            &probe_fingerprint
        ));
        assert_eq!(
            tombstone_completion(&tombstone, &probe_fingerprint)
                .error_code
                .as_deref(),
            Some("request_id_conflict"),
            "the conflict completion stays deterministic because the tombstone is durable"
        );
        tombstone.acknowledged = true;
        assert!(tombstone_completion_cacheable(
            &tombstone,
            &owner_fingerprint
        ));
        assert!(!tombstone_completion_cacheable(
            &tombstone,
            &probe_fingerprint
        ));
    }
}
