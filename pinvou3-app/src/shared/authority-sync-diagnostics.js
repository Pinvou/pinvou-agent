/**
 * One concentrated diagnostic stream for desktop/Web transcript reconciliation.
 *
 * Browser records are kept in a small local queue while disconnected and are
 * flushed into the desktop's `~/.pinvou3/logs/authority-sync.jsonl` after the
 * connection recovers. Callers must provide state metadata only; this module
 * additionally redacts content-like fields before anything leaves the client.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: classic script 直拷产物,严格模式是载荷
  "use strict";

  const COMMAND = "record_authority_sync_diagnostics";
  const STORAGE_KEY = "pinvou.authority_sync.diagnostics.v1";
  const MAX_QUEUE_ENTRIES = 256;
  const MAX_BATCH_ENTRIES = 32;
  const MAX_STRING_CHARS = 2048;
  const ALLOWED_EVENTS = new Set([
    "authority_sync_notice_shown", "browser_network_offline", "browser_network_online",
    "chat_done_classified", "connection_state_changed", "diagnostics_initialized",
    "document_visibility_changed", "local_send_blocked_by_remote_sync",
    "local_turn_admission_failed", "local_turn_admitted", "local_turn_claimed",
    "reconcile_attempt_failed", "reconcile_attempt_rejected", "reconcile_deferred_busy",
    "reconcile_exhausted", "reconcile_joined_inflight", "reconcile_started",
    "reconcile_succeeded", "remote_sync_blocked_action", "remote_turn_marked",
    "session_download_capability_wait_failed", "session_download_cleanup",
    "transcript_committed_event_received",
  ]);
  const IDENTIFIER_FIELDS = new Set(["session_id", "active_session_id", "trace_id", "download_id"]);
  const REVISION_FIELDS = new Set([
    "session_revision", "committed_revision", "expected_committed_revision", "saved_revision", "event_revision",
  ]);
  // Keep in lockstep with Rust's normalize_nonnegative_number cap (10^15) in
  // features/sessions/diagnostics.rs; the contract test enforces the pairing.
  const MAX_NUMBER_VALUE = 1000000000000000;
  const NUMBER_FIELDS = new Set([
    "message_count", "chat_item_count", "queued_count", "expected_assistant_key_length",
    "baseline_message_count", "minimum_terminal_message_count", "attempt", "attempts", "elapsed_ms",
    "saved_message_count", "chunk_count", "bytes_received", "declared_total_bytes",
    "cleanup_requested_count", "cleanup_failed_count", "cleanup_succeeded_count", "restored_queue_count",
  ]);
  const BOOLEAN_FIELDS = new Set([
    "buffer_present", "local_turn_owned", "remote_turn_active", "remote_terminal_seen",
    "loaded_from_disk", "buffer_busy", "ui_busy", "baseline_trusted", "preserve_committed_revision",
    "snapshot_present", "completed_local_turn", "requires_authority_reconcile", "terminal_error_present",
    "terminal_seen_before_event",
    "concurrent_turn", "error_present", "cancellable_lease", "cancel_requested", "cancel_succeeded",
    "desktop_online",
  ]);
  const ENUM_FIELDS = {
    reason: new Set(["assistant_identity_missing", "invalid_snapshot", "load_session_error", "message_count_short", "revision_mismatch"]),
    error_category: new Set([
      "cancel_rpc_failed", "capability_snapshot_timeout", "capability_snapshot_unavailable",
      "command_rejected", "download_id_mismatch", "session_turn_in_progress", "snapshot_load_failed",
    ]),
    operation: new Set(["accept_plan", "edit_last_turn", "send", "send_to_session"]),
    notice: new Set(["desktop_done_sync_pending", "remote_done_unsynced"]),
    terminal_status: new Set(["Cancelled", "Canceled", "Completed", "Failed", "Interrupted", "cancelled", "canceled", "completed", "failed", "interrupted"]),
    transport_kind: new Set(["desktop_invoke", "web_chunked_rpc"]),
    status: new Set(["connected", "connecting", "desktop_offline", "error", "idle", "local", "unknown"]),
    visibility: new Set(["hidden", "prerender", "unknown", "visible"]),
  };
  let queue = [];
  let sequence = 0;
  let flushing = false;
  let retryTimer = null;
  let lastConnectionSignature = "";

  function randomId(prefix) {
    try {
      if (root.crypto && typeof root.crypto.randomUUID === "function") {
        return prefix + "_" + root.crypto.randomUUID(); // safari14-ok: guarded above
      }
    } catch { /* UUID 生成异常时走下方兜底 */ }
    // eslint-disable-next-line sonarjs/pseudo-random -- 非安全用途:诊断去重 ID,时间戳前缀已保证基本唯一
    return prefix + "_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2);
  }

  const clientRunId = randomId("authority_sync_client");

  function isSensitiveKey(key) {
    const normalized = String(key || "").toLowerCase().replaceAll('-', "_");
    // eslint-disable-next-line sonarjs/regex-complexity -- 脱敏字段白名单刻意全量枚举,拆分会降低可审计性
    return /^(access_token|api_key|authorization|body|content|cookie|credential|credentials|cwd|directory|error|file_path|id_token|local_path|message|output|password|path|prompt|refresh_token|request|response|secret|text|token|user_input)$/.test(normalized) ||
      // eslint-disable-next-line sonarjs/regex-complexity -- 脱敏后缀黑名单刻意全量枚举,拆分会降低可审计性
      /(_access_token|_api_key|_authorization|_body|_content|_cookie|_credential|_credentials|_directory|_error|_file_path|_id_token|_local_path|_message|_output|_password|_path|_prompt|_refresh_token|_request|_response|_secret|_user_input)$/.test(normalized);
  }

  function cleanString(value) {
    return String(value == null ? "" : value)
      .replaceAll(/\b(?:Cookie|Set-Cookie)\s*:[^\r\n]*/gi, "Cookie: [REDACTED]")
      .replaceAll(/[\r\n\t]+/g, " ")
      .replaceAll(/\bBearer\s+\S+/gi, "Bearer [REDACTED]")
      .replaceAll(/\bBasic\s+\S+/gi, "Basic [REDACTED]")
      .replaceAll(/([?&](?:access_token|api_key|token|secret|password)=)[^&#\s]+/gi, "$1[REDACTED]")
      .replaceAll(/\b(?:access_token|api_key|authorization|cookie|token|secret|password)\s*=\s*[^\s;,]+/gi, function (match) {
        return match.slice(0, match.indexOf("=") + 1) + "[REDACTED]";
      })
      .replaceAll(/\bfile:\/\/\/?[^\s]+/gi, "[LOCAL_PATH]")
      .replaceAll(/\b[A-Za-z]:[\\/][^\s]+/g, "[LOCAL_PATH]")
      .replaceAll(/\\{2}[^\\\s]+\\[^\s]+/g, "[LOCAL_PATH]")
      .replaceAll(/(^|\s)~[\\/][^\s]+/g, "$1[LOCAL_PATH]")
      .replaceAll(/\/(?:Users|home|tmp|private\/var|var\/folders)\/[^\s]+/g, "[LOCAL_PATH]")
      .slice(0, MAX_STRING_CHARS);
  }

  function sanitize(value, depth) {
    if (depth >= 8) return "[DEPTH_LIMIT]";
    if (value == null || typeof value === "boolean" || typeof value === "number") return value;
    if (typeof value === "string") return cleanString(value);
    if (Array.isArray(value)) {
      return value.slice(0, 64).map(function (entry) { return sanitize(entry, depth + 1); });
    }
    if (typeof value === "object") {
      const result = {};
      Object.keys(value).slice(0, 64).forEach(function (key) {
        result[key] = isSensitiveKey(key) ? "[REDACTED]" : sanitize(value[key], depth + 1);
      });
      return result;
    }
    return cleanString(value);
  }

  function allowedCause(value) {
    if (new Set([
      "accept_plan_concurrent_turn", "chat_done_without_local_owner", "edit_last_turn_concurrent_turn",
      "local_send_concurrent_turn", "remote_user_message_event",
    ]).has(value)) return true;
    return /^event:chat:(delta|reasoning_delta|reasoning_done|reasoning_start|tool_end|tool_start|transient_error|turn_started|user_input_required|user_message)$/.test(value);
  }

  function normalizeDetails(value) {
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const output = {};
    Object.keys(value).forEach(function (key) {
      const candidate = value[key];
      if (key === "transport") {
        output.transport = normalizeDetails(candidate);
      } else if (IDENTIFIER_FIELDS.has(key) && typeof candidate === "string" &&
          candidate.length <= 256 && /^[A-Za-z0-9_.:-]*$/.test(candidate)) {
        output[key] = candidate;
      } else if (REVISION_FIELDS.has(key) && typeof candidate === "string" &&
          (candidate === "" || /^[A-Fa-f0-9]{64}$/.test(candidate))) {
        output[key] = candidate.toLowerCase();
      } else if ((NUMBER_FIELDS.has(key) && (candidate === null ||
          (Number.isSafeInteger(candidate) && candidate >= 0 && candidate <= MAX_NUMBER_VALUE))) ||
          (BOOLEAN_FIELDS.has(key) && typeof candidate === "boolean")) {
        output[key] = candidate;
      } else if (key === "cause" && typeof candidate === "string" && allowedCause(candidate)) {
        output.cause = candidate;
      } else if (ENUM_FIELDS[key] && typeof candidate === "string" && ENUM_FIELDS[key].has(candidate)) {
        output[key] = candidate;
      } else if (key === "saved_roles" && Array.isArray(candidate) && candidate.length <= 12 &&
          candidate.every(function (role) { return /^(assistant|invalid|system|tool|user)$/.test(role); })) {
        output.saved_roles = [...candidate];
      }
    });
    return sanitize(output, 0);
  }

  function normalizeConnection(snapshot) {
    snapshot = snapshot || {};
    const output = {};
    if (/^(desktop|unknown|web)$/.test(snapshot.platform_kind)) output.platform_kind = snapshot.platform_kind;
    if (typeof snapshot.browser_online === "boolean") output.browser_online = snapshot.browser_online;
    if (/^(hidden|prerender|unknown|visible)$/.test(snapshot.visibility)) output.visibility = snapshot.visibility;
    if (/^(connected|connecting|credentials_missing|denied|desktop_offline|error|idle|incompatible_desktop|local|replaced|revoked|unknown)$/.test(snapshot.connection_status)) {
      output.connection_status = snapshot.connection_status;
    }
    if (typeof snapshot.desktop_online === "boolean") output.desktop_online = snapshot.desktop_online;
    if (typeof snapshot.endpoint_id === "string" && snapshot.endpoint_id.length <= 256 &&
        /^[A-Za-z0-9_.:-]*$/.test(snapshot.endpoint_id)) output.endpoint_id = snapshot.endpoint_id;
    return output;
  }

  function connectionSnapshot() {
    const platform = root.PinvouPlatform || {};
    let state = null;
    try {
      if (typeof platform.getConnectionState === "function") state = platform.getConnectionState();
    } catch { /* 连接态读取失败按未知处理 */ }
    return {
      platform_kind: platform.kind || (root.__TAURI__ ? "desktop" : "unknown"),
      browser_online: !root.navigator || root.navigator.onLine !== false,
      visibility: root.document && root.document.visibilityState || "unknown",
      connection_status: state && state.status || (platform.isWeb ? "unknown" : "local"),
      desktop_online: state ? state.desktop_online === true : !platform.isWeb,
      endpoint_id: state && state.endpoint_id || "",
    };
  }

  function loadQueue() {
    try {
      const raw = root.localStorage && root.localStorage.getItem(STORAGE_KEY);
      const parsed = raw ? JSON.parse(raw) : [];
      if (Array.isArray(parsed)) queue = parsed.slice(-MAX_QUEUE_ENTRIES);
    } catch {
      queue = [];
    }
  }

  function saveQueue() {
    try {
      if (!root.localStorage) return;
      if (queue.length) root.localStorage.setItem(STORAGE_KEY, JSON.stringify(queue));
      else root.localStorage.removeItem(STORAGE_KEY);
    } catch { /* localStorage 不可用时队列仅留内存 */ }
  }

  function invokeFunction() {
    return root.__TAURI__ && root.__TAURI__.core && root.__TAURI__.core.invoke;
  }

  function commandAvailable() {
    const platform = root.PinvouPlatform || {};
    if (!platform.isWeb) return true;
    try {
      return typeof platform.canInvoke === "function" && platform.canInvoke(COMMAND) === true;
    } catch {
      return false;
    }
  }

  function scheduleRetry() {
    if (retryTimer != null) return;
    retryTimer = root.setTimeout(function () {
      retryTimer = null;
      flush();
    }, 2000);
  }

  function flush() {
    if (flushing || !queue.length || !commandAvailable()) return Promise.resolve(false);
    const invoke = invokeFunction();
    if (typeof invoke !== "function") return Promise.resolve(false);
    const batch = queue.slice(0, MAX_BATCH_ENTRIES);
    const ids = Object.create(null);
    batch.forEach(function (entry) { ids[entry.event_id] = true; });
    flushing = true;
    return Promise.resolve(invoke(COMMAND, { entries: batch })).then(function () {
      queue = queue.filter(function (entry) { return !ids[entry.event_id]; });
      saveQueue();
      flushing = false;
      if (queue.length) return flush();
      return true;
    }).catch(function () {
      flushing = false;
      saveQueue();
      scheduleRetry();
      return false;
    });
  }

  function record(event, details) {
    event = cleanString(event).replaceAll(/[^A-Za-z0-9_.:-]/g, "").slice(0, 160);
    if (!ALLOWED_EVENTS.has(event)) return "";
    const snapshot = connectionSnapshot();
    const entry = {
      event,
      event_id: clientRunId + "_" + (++sequence),
      connection: normalizeConnection(snapshot),
      details: normalizeDetails(details || {}),
    };
    queue.push(entry);
    if (queue.length > MAX_QUEUE_ENTRIES) queue.splice(0, queue.length - MAX_QUEUE_ENTRIES);
    saveQueue();
    flush();
    return entry.event_id;
  }

  function recordConnection(state) {
    const signature = JSON.stringify({
      status: state && state.status || "unknown",
      desktop_online: !!(state && state.desktop_online),
      browser_online: !root.navigator || root.navigator.onLine !== false,
    });
    if (signature === lastConnectionSignature) {
      flush();
      return;
    }
    const previous = lastConnectionSignature;
    lastConnectionSignature = signature;
    record("connection_state_changed", {
      previous_signature: previous,
      status: state && state.status || "unknown",
      desktop_online: !!(state && state.desktop_online),
    });
  }

  loadQueue();
  const platform = root.PinvouPlatform || {};
  if (typeof platform.onConnectionChange === "function") {
    platform.onConnectionChange(recordConnection);
  }
  if (typeof root.addEventListener === "function") {
    root.addEventListener("online", function () { record("browser_network_online", {}); });
    root.addEventListener("offline", function () { record("browser_network_offline", {}); });
  }
  if (root.document && typeof root.document.addEventListener === "function") {
    root.document.addEventListener("visibilitychange", function () {
      record("document_visibility_changed", {
        visibility: root.document && root.document.visibilityState || "unknown",
      });
    });
  }

  root.PinvouAuthoritySyncDiagnostics = Object.freeze({
    record,
    flush,
    connectionSnapshot,
    pendingCount: function () { return queue.length; },
  });
  // 恢复条数在本次 record 入队前度量，此刻 queue.length 即恢复规模。
  record("diagnostics_initialized", { restored_queue_count: queue.length });
})(window);
