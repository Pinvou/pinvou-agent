/**
 * aux-chat feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 *
 * 「辅助对话」桥：每个任务（taskId）挂一条后台辅助会话（id 形如 aux-<ulid>）。
 * 辅助会话被后端从 list_sessions 过滤（不进 state.sessions），因此不能复用
 * chat 域的 sendMessageToSession（它要求 sid ∈ state.sessions）。本域只做薄
 * 封装：会话建立/加载复用 sessions 域的 per-session buffer 路径，回合事件仍由
 * chat-events 按 session_id 路由进 buffer（本域不新增任何事件监听）。
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.auxChat = function (context) {
    const state = context.state;
    const invoke = context.invoke;
    const bt = context.bt;
    const sessionStates = context.sessionStates;
    const ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    const purgeSessionBuffer = context.purgeSessionBuffer || function () {};
    const isBusyFor = context.isBusyFor;

    const AUX_SESSION_ID_PATTERN = /^aux-/;
    // 域内私有索引（非第二份全局状态）：discard 只知 taskId，清本地 buffer
    // 需要 auxId。后端删除触发的 session:deleted 由 sessions 域兜底清，双保险且幂等。
    const auxIdByTask = Object.create(null);

    function isAuxSession(id) {
      return typeof id === "string" && AUX_SESSION_ID_PATTERN.test(id);
    }

    function emptySnapshot() {
      return { chatItems: [], busy: false, queued: [] };
    }

    async function ensure(taskId) {
      const task = String(taskId || "").trim();
      if (!task) throw new Error(bt("targetSessionMissing"));
      const metadata = await invoke("get_or_create_aux_session", { sessionId: task });
      const auxId = metadata && typeof metadata.id === "string" ? metadata.id : "";
      if (!isAuxSession(auxId)) throw new Error(bt("sessionDataInvalid"));
      auxIdByTask[task] = auxId;
      // 辅助会话永不成为 active：走后台 buffer 的 load_session(setActive:false) 路径。
      await ensureSessionBufferLoaded(auxId);
      return auxId;
    }

    async function send(auxId, text) {
      const sid = String(auxId || "").trim();
      const message = String(text || "").trim();
      if (!isAuxSession(sid)) throw new Error(bt("targetSessionMissing"));
      if (!message) throw new Error(bt("replyContentEmpty"));
      await ensureSessionBufferLoaded(sid);
      const buf = sessionStates[sid];
      // 辅助会话不走排队（queue 是用户输入语义）：忙/有排队直接拒绝，调用方自行重试。
      if (isBusyFor(sid) || (buf && Array.isArray(buf.queued) && buf.queued.length > 0)) {
        throw new Error(bt("turnAlreadyInProgress"));
      }
      return invoke("chat", { message, attachments: [], sessionId: sid, restrictTools: true });
    }

    // 同步快照：未加载（无 buffer）返回空结构，不抛错、不触发加载。数组浅拷贝防
    // 调用方改穿内部 buffer。
    function snapshot(auxId) {
      const sid = String(auxId || "").trim();
      if (!sid) return emptySnapshot();
      if (sid === state.activeSessionId) {
        return {
          chatItems: [...(state.chatItems || [])],
          busy: !!state.busy,
          queued: [...(state.queued || [])],
        };
      }
      const buf = sessionStates[sid];
      if (!buf) return emptySnapshot();
      return {
        chatItems: Array.isArray(buf.chatItems) ? [...buf.chatItems] : [],
        busy: !!buf.busy,
        queued: Array.isArray(buf.queued) ? [...buf.queued] : [],
      };
    }

    async function discard(taskId) {
      const task = String(taskId || "").trim();
      if (!task) throw new Error(bt("targetSessionMissing"));
      await invoke("discard_aux_session", { sessionId: task });
      const auxId = auxIdByTask[task];
      delete auxIdByTask[task];
      if (auxId) purgeSessionBuffer(auxId);
    }

    return { ensure, send, snapshot, discard, isAuxSession };
  };
})(typeof window === "undefined" ? globalThis : window);
