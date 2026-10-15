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
  if (!pinvouSharedtauriMemoryCache) pinvouSharedtauriMemoryCache = window.PinvouBridgeShared.create("tauriMemory", { state, runOnSession, invoke, notify, bt, addSystemItem, candidateCardPatcher: patchItemByIdFor, timeStr, addChatItem, itemIdSeq: { get value() { return context.itemIdSeq; }, set value(v) { context.itemIdSeq = v; } }, memoryOverviewSeq: { get value() { return memoryOverviewSeq; }, set value(v) { memoryOverviewSeq = v; } } });
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

function applyMemoryProfileState(result) { return pinvouSharedtauriMemory().applyMemoryProfileState(result); }
function applyMemoryWriteState(result, update) { return pinvouSharedtauriMemory().applyMemoryWriteState(result, update); }

  // 记忆面板混合两类数据：runtime 按 session 分文件，profile/preferences/
  // pending 等为全局单文件(见后端 paths.rs)。加载仍必须带归属+序号校验：
  // await 挂起期间切会话或再次加载，旧响应返回后不得覆盖当前显示(尤其
  // runtime 属于别的会话)，也不得把候选卡 rehydrate 进当前对话流(串台)。
  // 任何新加载都会递增序号使在途读取作废(审计)。
  let memoryOverviewSeq = 0;
async function loadMemoryOverview(options) { return pinvouSharedtauriMemory().loadMemoryOverview(options); }
  // 守卫命中的善后：序号已被更新加载接管时由它负责收尾 loading；仅会话
  // 变化、无人接管时(如切草稿不续发加载)必须自己清掉 loading，否则面板
  // 永远停在"同步中"(审计补充)。
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
async function updateMemoryItem(kind, id, patch) { return pinvouSharedtauriMemory().updateMemoryItem(kind, id, patch); }
async function deleteMemoryItem(kind, id) { return pinvouSharedtauriMemory().deleteMemoryItem(kind, id); }
function resolveMemoryCandidate(command, args, chatItemId, statusLabel, errorKey) { return pinvouSharedtauriMemory().resolveMemoryCandidate(command, args, chatItemId, statusLabel, errorKey); }
  async function confirmMemoryCandidate(memoryId, chatItemId) {
    return resolveMemoryCandidate("confirm_pending_memory", { id: memoryId, sessionId: state.activeSessionId }, chatItemId, "已记住", "memoryWriteFailed");
  }
  async function ignoreMemoryCandidate(memoryId, chatItemId) {
    return resolveMemoryCandidate("ignore_pending_memory", { id: memoryId, sessionId: state.activeSessionId }, chatItemId, "已忽略", "memoryIgnoreFailed");
  }
  async function neverMemoryCandidate(memoryId, chatItemId) {
    return resolveMemoryCandidate("never_pending_memory", { id: memoryId, reason: "user_selected", sessionId: state.activeSessionId }, chatItemId, "不再提示", "memoryNeverFailed");
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
