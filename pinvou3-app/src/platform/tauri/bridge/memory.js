/**
 * memory feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["memory"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var addSystemItem = context.addSystemItem;
    var runSyncOnSession = context.runSyncOnSession;
    var patchItemById = context.patchItemById;
    var runOnSession = context.runOnSession;
    var addChatItem = context.addChatItem;
    var timeStr = context.timeStr;
  function memoryWriteLabel(event) {
    var text = event && event.text || "";
    if (!text) return "记忆已更新";
    return text;
  }
  function memoryWriteStatusLabel(event) {
    var action = event && event.action || "";
    if (action === "confirmed" || action === "remembered") return "记忆已更新";
    if (action === "archived") return "记忆已归档";
    if (action === "deleted") return "记忆已删除";
    return "记忆已更新";
  }
  function normalizeMemoryCandidateText(text) {
    return String(text || "").replace(/\s+/g, " ").trim().toLowerCase();
  }
  function handleMemoryWrite(payload) {
    var sid = payload && payload.session_id || state.activeSessionId;
    var events = payload && Array.isArray(payload.events) ? payload.events : [];
    if (!sid || !events.length) return;
    runOnSession(sid, function () {
      events.forEach(function (event) {
        if (!event) return;
        if (event.action === "pending") {
          var label = memoryWriteLabel(event);
          var labelKey = normalizeMemoryCandidateText(label);
          var existing = state.chatItems.find(function (it) {
            return it.type === "memory_candidate" && !it.resolved && (
              (event.id && it.memoryId === event.id) ||
              (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
            );
          });
          if (existing) {
            existing.memoryId = event.id || existing.memoryId;
            existing.kind = event.kind || existing.kind || "preference";
            existing.text = label;
            existing.time = timeStr();
            return;
          }
          addChatItem({
            type: "memory_candidate",
            memoryId: event.id,
            kind: event.kind || "preference",
            text: label,
            time: timeStr(),
            resolved: false,
          });
          return;
        }
        var label = memoryWriteLabel(event);
        var labelKey = normalizeMemoryCandidateText(label);
        var existing = state.chatItems.find(function (it) {
          return it.type === "memory_candidate" && (
            (event.id && it.memoryId === event.id) ||
            (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
          );
        });
        if (existing) {
          if (event.action === "ignored" || event.action === "never") {
            state.chatItems = state.chatItems.filter(function (it) { return it !== existing; });
            return;
          }
          existing.resolved = true;
          existing.statusLabel = event.action === "ignored" ? "已忽略"
            : event.action === "never" ? "不再提示"
            : event.action === "archived" ? "已归档"
            : event.action === "deleted" ? "已删除"
            : "已记住";
          existing.kind = event.kind || existing.kind || "preference";
          existing.text = label;
          existing.time = timeStr();
          return;
        }
        if (event.action === "ignored" || event.action === "never") {
          return;
        }
        addChatItem({
          type: "memory_notice",
          memoryId: event.id,
          kind: event.kind || "preference",
          text: label,
          statusLabel: memoryWriteStatusLabel(event),
          time: timeStr(),
        });
      });
      notify();
    });
    if (invoke) {
      setTimeout(function () {
        loadMemoryOverview({ rehydratePending: true });
      }, 0);
    }
  }

  function applyMemoryOverview(overview) {
    state.memory = {
      loading: false,
      error: null,
      profile: overview && overview.profile || null,
      preferences: overview && Array.isArray(overview.preferences) ? overview.preferences : [],
      work_context: overview && Array.isArray(overview.work_context) ? overview.work_context : [],
      current_focus: overview && Array.isArray(overview.current_focus) ? overview.current_focus : [],
      recent_activity: overview && Array.isArray(overview.recent_activity) ? overview.recent_activity : [],
      recent_work: overview && Array.isArray(overview.recent_work) ? overview.recent_work : [],
      pending: overview && Array.isArray(overview.pending) ? overview.pending : [],
      never: overview && Array.isArray(overview.never) ? overview.never : [],
      runtime: overview && overview.runtime || null,
      snapshot_path: overview && overview.snapshot_path || "",
    };
  }
  function upsertPendingMemoryCandidate(item) {
    if (!item || item.status !== "pending_confirm") return;
    var label = item.content || item.text || "";
    if (!label) return;
    var labelKey = normalizeMemoryCandidateText(label);
    var existing = state.chatItems.find(function (it) {
      return it.type === "memory_candidate" && !it.resolved && (
        (item.id && it.memoryId === item.id) ||
        (labelKey && normalizeMemoryCandidateText(it.text) === labelKey)
      );
    });
    if (existing) {
      existing.memoryId = item.id || existing.memoryId;
      existing.kind = item.kind || existing.kind || "preference";
      existing.text = label;
      return;
    }
    addChatItem({
      type: "memory_candidate",
      memoryId: item.id,
      kind: item.kind || "preference",
      text: label,
      time: timeStr(),
      resolved: false,
    });
  }
  function rehydratePendingMemoryCandidates(overview) {
    var pending = overview && Array.isArray(overview.pending) ? overview.pending : [];
    pending.forEach(upsertPendingMemoryCandidate);
  }
  async function loadMemoryOverview(options) {
    if (!invoke) return null;
    options = options || {};
    state.memory = Object.assign({}, state.memory, { loading: true, error: null });
    notify();
    try {
      var overview = await invoke("get_memory_overview", { sessionId: state.activeSessionId });
      applyMemoryOverview(overview);
      if (options.rehydratePending) rehydratePendingMemoryCandidates(overview);
      notify();
      return overview;
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { loading: false, error: String(e) });
      notify();
      return null;
    }
  }
  async function saveMemoryProfilePatch(patch) {
    if (!invoke) return null;
    try {
      await invoke("update_memory_profile", { patch: patch || {}, sessionId: state.activeSessionId });
      return await loadMemoryOverview();
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function deleteMemoryPreference(id) {
    if (!id || !invoke) return false;
    try {
      var res = await invoke("delete_memory_preference", { id: id, sessionId: state.activeSessionId });
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function updateMemoryItem(kind, id, patch) {
    if (!id || !invoke) return null;
    try {
      var command = kind === "preference" ? "update_memory_preference"
        : kind === "work_context" ? "update_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "update_timed_memory"
        : null;
      if (!command) return null;
      var args = { id: id, patch: patch || {}, sessionId: state.activeSessionId };
      if (command === "update_timed_memory") args.kind = kind;
      var res = await invoke(command, args);
      await loadMemoryOverview();
      return res && res.value;
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function deleteMemoryItem(kind, id) {
    if (!id || !invoke) return false;
    try {
      var command = kind === "preference" ? "delete_memory_preference"
        : kind === "work_context" ? "delete_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "delete_timed_memory"
        : null;
      if (!command) return false;
      var args = { id: id, sessionId: state.activeSessionId };
      if (command === "delete_timed_memory") args.kind = kind;
      var res = await invoke(command, args);
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function archiveRecentWorkMemory(id) {
    if (!id || !invoke) return false;
    try {
      var res = await invoke("archive_recent_work_memory", { id: id, sessionId: state.activeSessionId });
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      state.memory = Object.assign({}, state.memory, { error: String(e) });
      notify();
      throw e;
    }
  }
  async function confirmMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    var sid = state.activeSessionId;
    try {
      await invoke("confirm_pending_memory", { id: memoryId, sessionId: sid });
      if (chatItemId) patchItemById(chatItemId, { resolved: true, statusLabel: "已记住" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      addSystemItem(bt("memoryWriteFailed") + e);
    }
  }
  async function ignoreMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    var sid = state.activeSessionId;
    try {
      await invoke("ignore_pending_memory", { id: memoryId, sessionId: sid });
      if (chatItemId) patchItemById(chatItemId, { resolved: true, statusLabel: "已忽略" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      addSystemItem(bt("memoryIgnoreFailed") + e);
    }
  }
  async function neverMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    var sid = state.activeSessionId;
    try {
      await invoke("never_pending_memory", { id: memoryId, reason: "user_selected", sessionId: sid });
      if (chatItemId) patchItemById(chatItemId, { resolved: true, statusLabel: "不再提示" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      addSystemItem(bt("memoryNeverFailed") + e);
    }
  }
    return {
      handleMemoryWrite: handleMemoryWrite,
      loadMemoryOverview: loadMemoryOverview,
      saveMemoryProfilePatch: saveMemoryProfilePatch,
      deleteMemoryPreference: deleteMemoryPreference,
      updateMemoryItem: updateMemoryItem,
      deleteMemoryItem: deleteMemoryItem,
      archiveRecentWorkMemory: archiveRecentWorkMemory,
      confirmMemoryCandidate: confirmMemoryCandidate,
      ignoreMemoryCandidate: ignoreMemoryCandidate,
      neverMemoryCandidate: neverMemoryCandidate
    };
  };
})(window);
