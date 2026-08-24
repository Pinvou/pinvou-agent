/**
 * memory feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: classic script 直拷产物,严格模式是载荷
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: 直拷载荷的注册表引导,拆分语句会偏离产物原貌
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["memory"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const addSystemItem = context.addSystemItem;
    const patchItemByIdFor = context.patchItemByIdFor;
    const runOnSession = context.runOnSession;
    const addChatItem = context.addChatItem;
    const timeStr = context.timeStr;
  function memoryWriteLabel(event) {
    const text = event && event.text || "";
    if (!text) return "记忆已更新";
    return text;
  }
  function memoryWriteStatusLabel(event) {
    const action = event && event.action || "";
    if (action === "confirmed" || action === "remembered") return "记忆已更新";
    if (action === "archived") return "记忆已归档";
    if (action === "deleted") return "记忆已删除";
    return "记忆已更新";
  }
  function normalizeMemoryCandidateText(text) {
    return String(text || "").replaceAll(/\s+/g, " ").trim().toLowerCase();
  }
  function handleMemoryWrite(payload) {
    const sid = payload && payload.session_id || state.activeSessionId;
    const events = payload && Array.isArray(payload.events) ? payload.events : [];
    if (!sid || !events.length) return;
    runOnSession(sid, function () {
      events.forEach(function (event) {
        if (!event) return;
        if (event.action === "pending") {
          const label = memoryWriteLabel(event);
          const labelKey = normalizeMemoryCandidateText(label);
          const existing = state.chatItems.find(function (it) {
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
        const label = memoryWriteLabel(event);
        const labelKey = normalizeMemoryCandidateText(label);
        const existing = state.chatItems.find(function (it) {
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
    const previous = state.memory || {};
    const sourceStates = overview && overview.sources || {};
    // stateKey:后端 source 名与前端 state 字段名通常一致,但 snapshot 源对应
    // state.memory.snapshot_path,两者不同;保留上次值时按 state 字段名查找。
    function sourceValue(source, value, fallback, stateKey) {
      const status = sourceStates[source];
      if (status && status.available === false) {
        const key = stateKey || source;
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 下限,Object.hasOwn 不可用,本调用已是安全形态
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
      recent_work: sourceValue("recent_work", overview && Array.isArray(overview.recent_work) ? overview.recent_work : [], []),
      pending: sourceValue("pending", overview && Array.isArray(overview.pending) ? overview.pending : [], []),
      never: sourceValue("never", overview && Array.isArray(overview.never) ? overview.never : [], []),
      runtime: sourceValue("runtime", overview && overview.runtime || null, null),
      snapshot_path: sourceValue("snapshot", overview && overview.snapshot_path || "", "", "snapshot_path"),
      warnings: orderedMemoryWarnings(overview && overview.warnings),
      sources: sourceStates,
    };
  }
  function orderedMemoryWarnings(warnings) {
    const items = Array.isArray(warnings) ? warnings : [];
    return [
      ...items.filter(function (warning) {
        return warning && warning.code === "memory_topic_cleanup_required";
      }),
      ...items.filter(function (warning) {
        return !warning || warning.code !== "memory_topic_cleanup_required";
      }),
    ];
  }
  function applyMemoryProfileState(result) {
    if (!result || !result.profile) return;
    state.memory = Object.assign({}, state.memory, {
      loading: false,
      error: null,
      profile: result.profile,
      runtime: result.runtime || null,
      warnings: orderedMemoryWarnings(result.warnings),
    });
  }
  function applyMemoryWriteState(result, update) {
    if (!result) return;
    const next = Object.assign({}, state.memory, {
      loading: false,
      error: null,
      runtime: result.runtime || null,
      warnings: orderedMemoryWarnings(result.warnings),
    });
    if (update) update(next, result.value);
    state.memory = next;
    notify();
  }
  function upsertMemoryValue(items, value, replacedId) {
    if (!value) return items || [];
    const next = (items || []).filter(function (item) {
      return item && item.id !== value.id && item.id !== replacedId;
    });
    next.push(value);
    return next;
  }
  function upsertPendingMemoryCandidate(item) {
    if (!item || item.status !== "pending_confirm") return;
    const label = item.content || item.text || "";
    if (!label) return;
    const labelKey = normalizeMemoryCandidateText(label);
    const existing = state.chatItems.find(function (it) {
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
    const pending = overview && Array.isArray(overview.pending) ? overview.pending : [];
    pending.forEach(upsertPendingMemoryCandidate);
  }
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
  function discardStaleLoad(seq) {
    if (seq === memoryOverviewSeq) {
      state.memory = Object.assign({}, state.memory, { loading: false });
      notify();
    }
    return null;
  }
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
  async function deleteMemoryPreference(id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(审计补充)
    try {
      const res = await invoke("delete_memory_preference", { id, sessionId: state.activeSessionId });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (changed) next.preferences = (next.preferences || []).filter(function (item) { return item.id !== id; });
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
  async function updateMemoryItem(kind, id, patch) {
    if (!id || !invoke) return null;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(审计补充)
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
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(审计补充)
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
  async function archiveRecentWorkMemory(id) {
    if (!id || !invoke) return false;
    const sid = state.activeSessionId; // 同 saveMemoryProfilePatch：切走后不写 B 的面板(审计补充)
    try {
      const res = await invoke("archive_recent_work_memory", { id, sessionId: state.activeSessionId });
      if (sid === state.activeSessionId) {
        applyMemoryWriteState(res, function (next, changed) {
          if (changed) next.recent_work = (next.recent_work || []).filter(function (item) { return item.id !== id; });
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
    const sid = state.activeSessionId; // 入口捕获：候选卡 patch 与面板写入都定向回发起会话(审计补充)
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
    const sid = state.activeSessionId; // 同 confirmMemoryCandidate：定向回发起会话(审计补充)
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
    const sid = state.activeSessionId; // 同 confirmMemoryCandidate：定向回发起会话(审计补充)
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
    return {
      handleMemoryWrite,
      loadMemoryOverview,
      saveMemoryProfilePatch,
      deleteMemoryPreference,
      updateMemoryItem,
      deleteMemoryItem,
      archiveRecentWorkMemory,
      confirmMemoryCandidate,
      ignoreMemoryCandidate,
      neverMemoryCandidate
    };
  };
})(window);
