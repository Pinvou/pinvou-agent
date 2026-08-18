/**
 * One concentrated diagnostic stream for desktop/Web transcript reconciliation.
 *
 * Browser records are kept in a small local queue while disconnected and are
 * flushed into the desktop's `~/.pinvou3/logs/authority-sync.jsonl` after the
 * connection recovers. Callers must provide state metadata only; this module
 * additionally redacts content-like fields before anything leaves the client.
 */
(function (root) {
  "use strict";

  var COMMAND = "record_authority_sync_diagnostics";
  var STORAGE_KEY = "pinvou.authority_sync.diagnostics.v1";
  var MAX_QUEUE_ENTRIES = 256;
  var MAX_BATCH_ENTRIES = 32;
  var MAX_STRING_CHARS = 2048;
  var queue = [];
  var sequence = 0;
  var flushing = false;
  var retryTimer = null;
  var lastConnectionSignature = "";

  function randomId(prefix) {
    try {
      if (root.crypto && typeof root.crypto.randomUUID === "function") {
        return prefix + "_" + root.crypto.randomUUID();
      }
    } catch (_) {}
    return prefix + "_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2);
  }

  var clientRunId = randomId("authority_sync_client");

  function isSensitiveKey(key) {
    var normalized = String(key || "").toLowerCase();
    return /^(access_token|authorization|body|content|cookie|message|output|password|prompt|secret|text|token)$/.test(normalized) ||
      /(_secret|_password|_access_token)$/.test(normalized);
  }

  function cleanString(value) {
    return String(value == null ? "" : value)
      .replace(/[\r\n\t]+/g, " ")
      .replace(/\bBearer\s+\S+/gi, "Bearer [REDACTED]")
      .replace(/([?&](?:access_token|token|secret|password)=)[^&#\s]+/gi, "$1[REDACTED]")
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
      var result = {};
      Object.keys(value).slice(0, 64).forEach(function (key) {
        result[key] = isSensitiveKey(key) ? "[REDACTED]" : sanitize(value[key], depth + 1);
      });
      return result;
    }
    return cleanString(value);
  }

  function connectionSnapshot() {
    var platform = root.PinvouPlatform || {};
    var state = null;
    try {
      if (typeof platform.getConnectionState === "function") state = platform.getConnectionState();
    } catch (_) {}
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
      var raw = root.localStorage && root.localStorage.getItem(STORAGE_KEY);
      var parsed = raw ? JSON.parse(raw) : [];
      if (Array.isArray(parsed)) queue = parsed.slice(-MAX_QUEUE_ENTRIES);
    } catch (_) {
      queue = [];
    }
  }

  function saveQueue() {
    try {
      if (!root.localStorage) return;
      if (queue.length) root.localStorage.setItem(STORAGE_KEY, JSON.stringify(queue));
      else root.localStorage.removeItem(STORAGE_KEY);
    } catch (_) {}
  }

  function invokeFunction() {
    return root.__TAURI__ && root.__TAURI__.core && root.__TAURI__.core.invoke;
  }

  function commandAvailable() {
    var platform = root.PinvouPlatform || {};
    if (!platform.isWeb) return true;
    try {
      return typeof platform.canInvoke === "function" && platform.canInvoke(COMMAND) === true;
    } catch (_) {
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
    var invoke = invokeFunction();
    if (typeof invoke !== "function") return Promise.resolve(false);
    var batch = queue.slice(0, MAX_BATCH_ENTRIES);
    var ids = Object.create(null);
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
    var snapshot = connectionSnapshot();
    var entry = {
      schema_version: 1,
      occurred_at: new Date().toISOString(),
      source: snapshot.platform_kind === "web" ? "web-ui" : "desktop-ui",
      event: cleanString(event).replace(/[^A-Za-z0-9_.:-]/g, "").slice(0, 160),
      event_id: clientRunId + "_" + (++sequence),
      client_run_id: clientRunId,
      connection: snapshot,
      details: sanitize(details || {}, 0),
    };
    queue.push(entry);
    if (queue.length > MAX_QUEUE_ENTRIES) queue.splice(0, queue.length - MAX_QUEUE_ENTRIES);
    saveQueue();
    flush();
    return entry.event_id;
  }

  function recordConnection(state) {
    var signature = JSON.stringify({
      status: state && state.status || "unknown",
      desktop_online: !!(state && state.desktop_online),
      browser_online: !root.navigator || root.navigator.onLine !== false,
    });
    if (signature === lastConnectionSignature) {
      flush();
      return;
    }
    var previous = lastConnectionSignature;
    lastConnectionSignature = signature;
    record("connection_state_changed", {
      previous_signature: previous,
      status: state && state.status || "unknown",
      desktop_online: !!(state && state.desktop_online),
    });
  }

  loadQueue();
  var platform = root.PinvouPlatform || {};
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
    record: record,
    flush: flush,
    connectionSnapshot: connectionSnapshot,
    pendingCount: function () { return queue.length; },
  });
  record("diagnostics_initialized", { restored_queue_count: Math.max(0, queue.length - 1) });
})(window);
