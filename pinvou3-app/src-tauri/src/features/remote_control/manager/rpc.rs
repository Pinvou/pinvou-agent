//! RPC idempotency admission, response shaping, and the stream/event protocol
//! message builders consumed by the facade.
//!
//! Admission and cache helpers borrow `Inner` (or inspect it through `&Inner`)
//! and never lock the mutex themselves: the facade owns the guard and lends the
//! state, preserving the single-lock concurrency contract. Protocol message
//! builders are pure and stateless.

use std::collections::HashSet;
use std::time::Instant;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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

pub(super) fn is_event_subscribed(subscriptions: &HashSet<String>, event: &str) -> bool {
    subscriptions.contains(event)
}

pub(super) fn rpc_in_flight_expired(dispatched_at: Instant, now: Instant) -> bool {
    now.duration_since(dispatched_at) > super::RPC_IN_FLIGHT_TTL
}

pub(super) enum RpcRequestAction {
    None,
    Respond(RelaySender, Value),
    Dispatch(RpcDispatch),
}

pub(super) enum NewRpcAdmission {
    Rejected(RpcCompletion),
    Durable(RpcLedger),
}

pub(super) fn rpc_admission_rejection(
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
    } else if in_flight_count >= super::MAX_RPC_IN_FLIGHT {
        Some(rpc_error_completion(
            "too_many_in_flight_requests",
            "正在运行的远程控制请求过多",
        ))
    } else {
        None
    }
}

pub(super) fn prepare_new_rpc_admission(
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

pub(super) enum RpcReadyAction {
    Respond(RelaySender, Value),
    Dispatch(RpcDispatch),
}

pub(super) fn prepare_bridge_generation(
    inner: &mut Inner,
    generation: &str,
) -> Vec<RpcReadyAction> {
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
    }
}

pub(super) fn bounded_rpc_completion(
    ok: bool,
    result: Option<Value>,
    error: Option<String>,
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
        error_code: None,
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

pub(super) fn rpc_fingerprint(command: &str, args: &Value) -> Result<String, String> {
    let canonical = canonicalize_json(args);
    let encoded = serde_json::to_vec(&(command, canonical))
        .map_err(|error| format!("serialize Web RPC fingerprint: {error}"))?;
    let digest = Sha256::digest(encoded);
    Ok(crate::platform::encoding::hex_lower(&digest))
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
pub(super) enum WebSessionScope {
    Required(&'static str),
    Optional(&'static str),
}

pub(super) fn web_session_scope(command: &str) -> Option<WebSessionScope> {
    use WebSessionScope::{Optional, Required};
    let scope = match command {
        // Commands whose Rust API historically falls back to the desktop
        // process-wide active Session must be explicit over WebUI.
        "archive_recent_work_memory"
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
        | "never_pending_memory"
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
        | "unequip_persona"
        | "web_access_artifact_info"
        | "web_access_chat"
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
        | "web_access_cancel_session_download"
        | "web_access_load_session_chunk"
        | "web_access_save_session_messages_chunk" => Required("id"),

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
) -> Result<(), String> {
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
    super::validate_multi_agent_session_web_scope(app, command, session_id)?;
    if command == "web_access_cancel_session_download"
        || (command == "web_access_load_session_chunk"
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

pub(super) fn validate_bridge_generation(generation: &str) -> Result<(), String> {
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
        if is_event_subscribed(subscriptions, &event.event) {
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

pub(super) fn enqueue_stream_reset(sender: RelaySender, message: Value) {
    // RelaySender owns a single bounded waiter and coalesces repeated recovery
    // barriers to the latest lease/epoch while the data channel is saturated.
    if sender.enqueue_stream_reset(message).is_err() {
        eprintln!("[web-access] stream reset could not reach the relay task");
    }
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
