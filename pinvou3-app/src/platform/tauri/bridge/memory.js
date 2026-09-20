/**
 * memory feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["memory"] = function (context) {let pinvouSharedtauriMemoryCache = null;
function pinvouSharedtauriMemory() {
  if (!pinvouSharedtauriMemoryCache) pinvouSharedtauriMemoryCache = window.PinvouBridgeShared.create("tauriMemory", { state, runOnSession, invoke, notify, timeStr, addChatItem, loadMemoryOverview, memoryOverviewSeq: { get value() { return memoryOverviewSeq; }, set value(v) { memoryOverviewSeq = v; } } });
  return pinvouSharedtauriMemoryCache;
}


    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const addSystemItem = context.addSystemItem;
    const patchItemByIdFor = context.patchItemByIdFor;
    const runOnSession = context.runOnSession;
    const addChatItem = context.addChatItem;
    const timeStr = context.timeStr;



function handleMemoryWrite(payload) { return pinvouSharedtauriMemory().handleMemoryWrite(payload); }

  function applyMemoryOverview(overview) {
    const previous = state.memory || {};
    const sourceStates = overview && overview.sources || {};
    // stateKey:后端 source 名与前端 state 字段名通常一致,但 snapshot 源对应
    // state.memory.snapshot_path,两者不同;保留上次值时按 state 字段名查找。
    function sourceValue(source, value, fallback, stateKey) {
      const status = sourceStates[source];
      if (status && status.available === false) {
        const key = stateKey || source;
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already the safe form
        return Object.prototype.hasOwnProperty.call(previous, key) ? previous[key] : fallback;
      }
      return value;
    }
    state.memory = {
      loading: false,
      error: null,
      profile: sourceValue("profile", overview && overview.profile || null, null),
      preferences: sourceValue("preferences", overview && Array.isArray(overview.preferences) ? overview.preferences : [], []),
      work_context: sourceValue("work_context", overview && Array.isArray(overview.work_context) ? overview.work_context : [], []),
      current_focus: sourceValue("current_focus", overview && Array.isArray(overview.current_focus) ? overview.current_focus : [], []),
      recent_activity: sourceValue("recent_activity", overview && Array.isArray(overview.recent_activity) ? overview.recent_activity : [], []),
      pending: sourceValue("pending", overview && Array.isArray(overview.pending) ? overview.pending : [], []),
      runtime: sourceValue("runtime", overview && overview.runtime || null, null),
      snapshot_path: sourceValue("snapshot", overview && overview.snapshot_path || "", "", "snapshot_path"),
      warnings: orderedMemoryWarnings(overview && overview.warnings),
      sources: sourceStates,
    };
  }
function orderedMemoryWarnings(warnings) { return pinvouSharedtauriMemory().orderedMemoryWarnings(warnings); }
function applyMemoryProfileState(result) { return pinvouSharedtauriMemory().applyMemoryProfileState(result); }
function applyMemoryWriteState(result, update) { return pinvouSharedtauriMemory().applyMemoryWriteState(result, update); }
function upsertMemoryValue(items, value, replacedId) { return pinvouSharedtauriMemory().upsertMemoryValue(items, value, replacedId); }

function rehydratePendingMemoryCandidates(overview) { return pinvouSharedtauriMemory().rehydratePendingMemoryCandidates(overview); }
  // 记忆面板混合两类数据：runtime 按 session 分文件，profile/preferences/
  // pending 等为全局单文件(见后端 paths.rs)。加载仍必须带归属+序号校验：
  // await 挂起期间切会话或再次加载，旧响应返回后不得覆盖当前显示(尤其
  // runtime 属于别的会话)，也不得把候选卡 rehydrate 进当前对话流(串台)。
  // 任何新加载都会递增序号使在途读取作废(审计)。
  let memoryOverviewSeq = 0;
  async function loadMemoryOverview(options) {
    if (!invoke) return null;
    options = options || {};
    const sid = state.activeSessionId;
    const seq = ++memoryOverviewSeq;
    state.memory = Object.assign({}, state.memory, { loading: true, error: null });
    notify();
    try {
      // invoke 形状保持原样（协议指纹按文本计算）；发起瞬间 activeSessionId === sid。
      const overview = await invoke("get_memory_overview", { sessionId: state.activeSessionId });
      if (sid !== state.activeSessionId || seq !== memoryOverviewSeq) return discardStaleLoad(seq);
      applyMemoryOverview(overview);
      if (options.rehydratePending) rehydratePendingMemoryCandidates(overview);
      notify();
      return overview;
    } catch (e) {
      if (sid !== state.activeSessionId || seq !== memoryOverviewSeq) return discardStaleLoad(seq);
      state.memory = Object.assign({}, state.memory, { loading: false, error: String(e) });
      notify();
      return null;
    }
  }
  // 守卫命中的善后：序号已被更新加载接管时由它负责收尾 loading；仅会话
  // 变化、无人接管时(如切草稿不续发加载)必须自己清掉 loading，否则面板
  // 永远停在"同步中"(审计补充)。
function discardStaleLoad(seq) { return pinvouSharedtauriMemory().discardStaleLoad(seq); }
  async function saveMemoryProfilePatch(patch) {
    if (!invoke) return null;
    // 入口捕获触发会话：invoke 往返期间切走，A 的写结果/错误不得渲染进
    // B 的面板(与 loadMemoryOverview 同一不变量，审计补充)。
    const sid = state.activeSessionId;
    try {
      const result = await invoke("update_memory_profile", { patch: patch || {}, sessionId: state.activeSessionId });
      if (sid === state.activeSessionId) { applyMemoryProfileState(result); notify(); }
      const overview = await loadMemoryOverview();
      return overview || result;
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function updateMemoryItem(kind, id, patch) {
    if (!id || !invoke) return null;
    const sid = state.activeSessionId; // same as saveMemoryProfilePatch: after switching away, never write to B's panel (audit follow-up)
    try {
      const command = kind === "preference" ? "update_memory_preference"
        : kind === "work_context" ? "update_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "update_timed_memory"
        : null;
      if (!command) return null;
      const args = { id, patch: patch || {}, sessionId: state.activeSessionId };
      if (command === "update_timed_memory") args.kind = kind;
      const res = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, value) {
          if (!value) return;
          const source = kind === "preference" ? "preferences" : kind;
          next[source] = upsertMemoryValue(next[source], value, id);
        });
      }
      await loadMemoryOverview();
      return res && res.value;
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function deleteMemoryItem(kind, id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // same as saveMemoryProfilePatch: after switching away, never write to B's panel (audit follow-up)
    try {
      const command = kind === "preference" ? "delete_memory_preference"
        : kind === "work_context" ? "delete_work_context_memory"
        : (kind === "current_focus" || kind === "recent_activity") ? "delete_timed_memory"
        : null;
      if (!command) return false;
      const args = { id, sessionId: state.activeSessionId };
      if (command === "delete_timed_memory") args.kind = kind;
      const res = await invoke(command, args);
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (!changed) return;
          const source = kind === "preference" ? "preferences" : kind;
          next[source] = (next[source] || []).filter(function (item) { return item.id !== id; });
        });
      }
      await loadMemoryOverview();
      return !!(res && res.value);
    } catch (e) {
      if (sid === state.activeSessionId) {
        state.memory = Object.assign({}, state.memory, { error: String(e) });
        notify();
      }
      throw e;
    }
  }
  async function confirmMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    const sid = state.activeSessionId; // captured at entry: both the candidate-card patch and panel writes route back to the originating session (audit follow-up)
    try {
      const result = await invoke("confirm_pending_memory", { id: memoryId, sessionId: sid });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== memoryId; });
        });
      }
      // patch 必须按发起会话路由(而非当前显示)：切走后写 B 的 chatItems 是
      // no-op，A 的候选卡会永远停留在"可点击未决"态，切回再点会二次提交。
      if (chatItemId) patchItemByIdFor(sid, chatItemId, { resolved: true, statusLabel: "已记住" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt("memoryWriteFailed") + e);
    }
  }
  async function ignoreMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    const sid = state.activeSessionId; // same as confirmMemoryCandidate: route back to the originating session (audit follow-up)
    try {
      const result = await invoke("ignore_pending_memory", { id: memoryId, sessionId: sid });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== memoryId; });
        });
      }
      if (chatItemId) patchItemByIdFor(sid, chatItemId, { resolved: true, statusLabel: "已忽略" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt("memoryIgnoreFailed") + e);
    }
  }
  async function neverMemoryCandidate(memoryId, chatItemId) {
    if (!memoryId) return;
    const sid = state.activeSessionId; // same as confirmMemoryCandidate: route back to the originating session (audit follow-up)
    try {
      const result = await invoke("never_pending_memory", { id: memoryId, reason: "user_selected", sessionId: sid });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(result, function (next) {
          next.pending = (next.pending || []).filter(function (item) { return item.id !== memoryId; });
        });
      }
      if (chatItemId) patchItemByIdFor(sid, chatItemId, { resolved: true, statusLabel: "不再提示" });
      await loadMemoryOverview();
      notify();
    } catch (e) {
      if (sid === state.activeSessionId) addSystemItem(bt("memoryNeverFailed") + e);
    }
  }
  // AI organize memory ("AI 整理记忆"): operates on global memory data and does
  // not depend on the current session; the entry point still captures the
  // session so success and failure are both written back only to the panel of
  // the session that started it (same invariant as saveMemoryProfilePatch:
  // prevents rendering results or errors into another conversation stream if
  // the user switches away during the await). On success, reuse the write
  // flow's runtime/warnings reconciliation (applyMemoryWriteState) and refetch
  // the overview to refresh panel content; the caller reads the report from
  // the return value. On failure, rethrow: memory.error is the dedicated
  // load-failure channel (the settings banner renders it as the generic
  // "加载失败" (load failed) copy), so the organize failure reason is
  // surfaced by the caller's catch and must not pollute that channel.
  async function organizeMemory() {
    if (!invoke) return null;
    const sid = state.activeSessionId; // same as saveMemoryProfilePatch: after switching away, never write to B's panel
    const result = await invoke("organize_memory");
    if (sid === state.activeSessionId && result) applyMemoryWriteState(result);
    // Organizing can merge/delete entries: refetch the overview like the other
    // write flows to refresh panel content; the return value stays the raw
    // organize_memory payload (report/runtime/warnings).
    await loadMemoryOverview();
    return result;
  }
async function loadOrganizeHistory() { return pinvouSharedtauriMemory().loadOrganizeHistory(); }
    return {
      handleMemoryWrite,
      loadMemoryOverview,
      saveMemoryProfilePatch,
      updateMemoryItem,
      deleteMemoryItem,
      confirmMemoryCandidate,
      ignoreMemoryCandidate,
      neverMemoryCandidate,
      organizeMemory,
      loadOrganizeHistory
    };
  };
})(window);
