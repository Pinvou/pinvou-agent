(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";

  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.chat = function (context) {
    const state = context.state;
    const invoke = context.invoke;
    const notify = context.notify;
    const TAURI = context.TAURI;
    const sessionStates = context.sessionStates;
    const turnUsageDirty = context.turnUsageDirty;
    const personaPlaceholderTitles = context.personaPlaceholderTitles;
    const safeConsoleInfo = context.safeConsoleInfo;
    const recordAuthoritySyncDiagnostic = context.recordAuthoritySyncDiagnostic || function () {};
    const authoritySyncBufferSnapshot = context.authoritySyncBufferSnapshot || function () { return {}; };
    const bt = context.bt;
    const isDefaultChatTitle = context.isDefaultChatTitle;
    const runSyncOnSession = context.runSyncOnSession;
    const startThinking = context.startThinking;
    const stopThinking = context.stopThinking;
    const ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    const ensureSession = context.ensureSession;
    const getBuffer = context.getBuffer;
    const recordPinvouSceneForMessage = context.recordPinvouSceneForMessage || function () {};
    const reconcileRemoteTurn = context.reconcileRemoteTurn;
    const markRemoteTurn = context.markRemoteTurn;
    const adoptManagedAttachments = context.adoptManagedAttachments || function () { return Promise.resolve(); };
    const discardManagedAttachment = context.discardManagedAttachment || function () { return Promise.resolve(); };
    const isScheduledRunSession = context.isScheduledRunSession;
    const basename = context.basename;
    const userMessageDisplayText = context.userMessageDisplayText;
    const extractArtifactPaths = context.extractArtifactPaths;
    const parseScheduledTaskDraftFromText = context.parseScheduledTaskDraftFromText;
    const autoCreateScheduledTaskDraft = context.autoCreateScheduledTaskDraft;

  // Composer 草稿是纯前端短期状态：写入时不 notify，避免每次按键都克隆
  // 整个 chat slice 并触发 App 重渲染。会话切换本身会 notify，ChatView 会在
  // activeSessionId 变化后主动读取目标 working set 的草稿。
  function getComposerDraft() {
    return String(state.composerDraft || "");
  }
  function setComposerDraft(value) {
    const text = value == null ? "" : String(value);
    state.composerDraft = text;
    const activeBuffer = state.activeSessionId && sessionStates[state.activeSessionId];
    if (activeBuffer) activeBuffer.composerDraft = text;
    return text;
  }

  // 打断（interrupt）在途标记（按 session）：打断期间禁止 flushQueued 抢先发
  // 排队消息——chat:done handler 在打断消息 doSendFor 之前触发 flushQueued，
  // 若不挡，排队消息会先 reserve 成功、打断消息反而撞 session_turn_in_progress。
  // 打断消息发出（或其失败路径收尾）后清除，排队消息由打断轮的 chat:done 继续。
  const interruptInFlight = {};

  // steer_chat invoke 的兜底超时。Rust 侧 steer() await 底座 mpsc send,
  // 引擎任务卡住(不死、不 drain channel)时 invoke 永不结算——输入框已清空、
  // chip 无 steerId 回填,排队区会被这个悬挂 chip 阻塞。25s 与
  // waitForChatDone 的兜底对齐:正常引擎入队是同步量级,25s 只兜真实卡死。
  const STEER_INVOKE_TIMEOUT_MS = 25000;

  // ── Chat Items (display format for React) ────────────────────────
  function addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
  }
  function messageHasToolBlock(type, toolCallId) {
    if (!toolCallId) return false;
    for (let i = state.messages.length - 1; i >= 0; i--) {
      const blocks = state.messages[i] && state.messages[i].content;
      if (!Array.isArray(blocks)) continue;
      for (let j = blocks.length - 1; j >= 0; j--) {
        const block = blocks[j];
        if (!block || block.type !== type) continue;
        if ((type === "tool_use" ? block.id : block.tool_use_id) === toolCallId) return true;
      }
    }
    return false;
  }
  function toolCallAlreadyStarted(toolCallId) {
    if (!toolCallId) return false;
    if ((context.pendingAssistantBlocks || []).some(function (block) {
      return block && block.type === "tool_use" && block.id === toolCallId;
    })) return true;
    if (state.chatItems.some(function (item) {
      return item && item.type === "tool" && item.toolId === toolCallId;
    })) return true;
    return messageHasToolBlock("tool_use", toolCallId);
  }
  function toolCallAlreadyFinished(toolCallId) {
    return messageHasToolBlock("tool_result", toolCallId);
  }
  function hasChatItemForTool(type, toolCallId) {
    return !!toolCallId && state.chatItems.some(function (item) {
      return item && item.type === type && item.toolCallId === toolCallId;
    });
  }
  // 成品卡是否"重复出卡":从 chatItems 末尾往前扫——先遇到该文件的修改工具(write/append/edit)
  // → 不算重复(文件改过了,该出新版卡/续卡,即"二次修改弹新卡");先遇到同名成品卡 → 算重复
  // (同一产物没改又 present 一次,模型常见啰嗦)。判据=「上一张同名卡之后有没有改过这个文件」。
  // 例外:扫到**用户发言**就放行——用户在上一张卡之后又开了口(典型「再推一次」「没看到」),
  // 这次 present 是新请求的响应,不是模型自发啰嗦;再去重 = 用户主动要却看不到任何反馈(实测 bug)。
  function isDuplicateArtifactCard(pathv) {
    const bn = basename(pathv);
    if (!bn) return false;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type === "tool" && context.fileMutationAction(it.name, it.args)) {
        const changedPaths = extractArtifactPaths(it.args);
        if (changedPaths.some(function (ap) { return basename(ap) === bn; })) return false;
      }
      if (it.type === "user") return false;
      if (it.type === "artifact_card" && basename(it.path) === bn) return true;
    }
    return false;
  }
  function addSystemItem(text, meta) {
    const item = { type: "system", text, time: timeStr() };
    if (meta) {
      for (const k in meta) item[k] = meta[k];
    }
    addChatItem(item);
    notify();
  }
  function addAuthoritySyncNotice(text) {
    if (state.chatItems.some(function (item) {
      return item && item.authoritySyncNotice;
    })) return;
    addSystemItem(text, { authoritySyncNotice: true });
  }
  function compactPruneRollupText(count) {
    return bt("compactDone") + bt("compactAuto") + " " +
      bt("compactPruneMerged") + " ×" + count;
  }
  function removeCompactionStartItem(compactId) {
    if (!compactId) return;
    for (let i = state.chatItems.length - 1; i >= 0; i--) {
      const it = state.chatItems[i];
      if (it.type === "system" && it.compactId === compactId && it.compactPhase === "start") {
        state.chatItems.splice(i, 1);
        return;
      }
    }
  }
  function addOrMergePruneCompaction(compactId) {
    removeCompactionStartItem(compactId);
    const last = state.chatItems[state.chatItems.length - 1];
    if (last && last.type === "system" && last.compactPruneRollup) {
      last.compactPruneCount = (last.compactPruneCount || 1) + 1;
      last.text = compactPruneRollupText(last.compactPruneCount);
      last.time = timeStr();
      notify();
      return;
    }
    addChatItem({
      type: "system",
      text: compactPruneRollupText(1),
      time: timeStr(),
      compactPruneRollup: true,
      compactPruneCount: 1,
    });
    notify();
  }
  function timeStr() {
    return new Date().toTimeString().slice(0, 5);
  }

  // ── Flush helpers (same as main.js) ──────────────────────────────
  function flushPendingTextBlock() {
    if (context.pendingAssistantText) {
      context.pendingAssistantBlocks.push({ type: "text", text: context.pendingAssistantText });
      context.pendingAssistantText = "";
    }
  }
  function flushAssistantMessageToHistory() {
    flushPendingTextBlock();
    if (context.pendingAssistantBlocks.length) {
      const assistantText = context.pendingAssistantBlocks
        .filter(function (block) { return block && block.type === "text" && block.text; })
        .map(function (block) { return block.text; })
        .join("\n\n");
      if (state.activeSessionId && state.activeSessionId === state.scheduledTaskCreationSessionId) {
        const scheduledTaskDraft = parseScheduledTaskDraftFromText(assistantText);
        if (scheduledTaskDraft) {
          autoCreateScheduledTaskDraft(scheduledTaskDraft, state.activeSessionId);
        }
      }
      state.messages.push({ role: "assistant", content: context.pendingAssistantBlocks });
      context.pendingAssistantBlocks = [];
    }
  }
  function resetPendingAssistant() {
    context.pendingAssistantText = "";
    context.pendingAssistantBlocks = [];
    context.currentStreamText = "";
    context.currentStreamId = 0;
  }


  // ── Send message ─────────────────────────────────────────────────
  // 指定 session 是否正在生成(active 看工作集 busy,后台看其 buffer)。
  function isBusyFor(sid) {
    return sid === state.activeSessionId ? state.busy : !!(sessionStates[sid] && sessionStates[sid].busy);
  }
  function formatAttachmentDisplayText(text, attachments) {
    const names = (attachments || []).map(function (attachment) {
      return typeof attachment === "string" ? attachment : attachment && attachment.basename;
    }).filter(Boolean).map(String);
    if (!names.length) return String(text || "");
    const attachmentLine = "📎 " + JSON.stringify(names);
    return String(text || "").trim()
      ? String(text) + "\n\n" + attachmentLine
      : attachmentLine;
  }
  // 桌宠窗口靠全局事件感知回合起止。turn_start 补齐"发送 → 首 token"的空窗
  // (chat:delta 之前引擎在思考,宠物不该干站着);turn_end 只兜 invoke 直接失败
  // 这种不会有 chat:done 的路径。JS emit 是全局广播,宠物窗口 listen 收得到。
  function emitPetEvent(name, sid) {
    try {
      if (TAURI && TAURI.event && TAURI.event.emit) TAURI.event.emit(name, { session_id: sid });
    } catch { /* 桌宠是纯装饰,广播失败不影响对话 */ }
  }
  function trackSceneBehavior(sid, scene) {
    const raw = String(scene || "");
    if (!sid || !raw) return;
    const parts = raw.split(":");
    invoke("track_behavior_event", {
      request: {
        eventName: "scene_triggered",
        sessionId: sid,
        sceneL1: parts[0] || "unknown",
        sceneL2: parts.slice(1).join(":") || parts[0] || "unknown",
      },
    }).catch(function () {});
  }

  // 真正发送:在 sid 的工作集上加 user 气泡 + 流式占位 + busy,然后 invoke chat。
  // active/后台通用(后台走 runSyncOnSession 临时切工作集)。
  function doSendFor(sid, text, displayText, attachmentsPayload, meta, restrictTools, surfaceFailure) {
    safeConsoleInfo("[pinvou3][chat-ui] send start", {
      sid,
      textLen: (text || "").length,
      attachments: attachmentsPayload ? attachmentsPayload.length : 0,
    });
    turnUsageDirty[sid] = false; // 新一轮开始，重置口径保护
    const turnOwnerBuffer = getBuffer(sid);
    let submittedMessage = null;
    let submittedMessagePos = -1;
    let submittedUserItemId = 0;
    let submittedStreamId = 0;
    if (turnOwnerBuffer && turnOwnerBuffer.remoteTurnActive) {
      recordAuthoritySyncDiagnostic("local_send_blocked_by_remote_sync", authoritySyncBufferSnapshot(sid, turnOwnerBuffer));
      return Promise.reject(new Error(bt("sessionSyncingTurn")));
    }
    if (turnOwnerBuffer) {
      turnOwnerBuffer.localTurnOwned = true;
      turnOwnerBuffer.remoteTurnActive = false;
      turnOwnerBuffer.remoteTerminalSeen = false;
      turnOwnerBuffer.remoteCommittedRevision = "";
      recordAuthoritySyncDiagnostic("local_turn_claimed", Object.assign({
        operation: "send",
      }, authoritySyncBufferSnapshot(sid, turnOwnerBuffer)));
    }
    runSyncOnSession(sid, function () {
      state.chatItems = state.chatItems.filter(function (item) {
        return !item.turnErrorNotice && !item.authoritySyncNotice;
      });
      const uitem = {
        type: "user",
        text: displayText,
        time: timeStr(),
        messageIndex: state.messages.length,
      };
      if (meta && meta.pinvouTransfer) uitem.pinvouTransfer = meta.pinvouTransfer; // 仅展示层,不进 messages/LLM
      if (meta && meta.pinvouScene) uitem.pinvouScene = meta.pinvouScene; // 仅展示层,不进 messages/LLM
      addChatItem(uitem);
      submittedUserItemId = uitem.id;
      submittedMessage = { role: "user", content: [{ type: "text", text: displayText }] };
      submittedMessagePos = state.messages.length;
      state.messages.push(submittedMessage);
      state.busy = true;
      startThinking();
      context.currentStreamText = "";
      context.currentStreamId = ++context.itemIdSeq;
      submittedStreamId = context.currentStreamId;
      state.chatItems.push({ id: context.currentStreamId, type: "assistant", text: "", html: "", time: timeStr(), streaming: true });
    });
    notify();
    emitPetEvent("pet:turn_start", sid);
    return invoke("chat", { message: text, attachments: attachmentsPayload, sessionId: sid, restrictTools: !!restrictTools })
      .then(function () {
        // 新一轮已被后端受理：会话中未提交的「打开」（pending enable）自此进入
        // 上下文并锁死（ComposerToolMenu 监听）。bridge 层不反向依赖 features，
        // 与 chat-events.js 的 pinvou:tools-changed 一样内联派发。
        try { window.dispatchEvent(new CustomEvent("pinvou:chat-round-committed", { detail: { scope: "plain" } })); } catch { /* dispatch may fail in non-DOM test harness */ }
        recordAuthoritySyncDiagnostic("local_turn_admitted", Object.assign({
          operation: "send",
        }, authoritySyncBufferSnapshot(sid, turnOwnerBuffer)));
        if (turnOwnerBuffer) turnOwnerBuffer.deferredRemoteUserEvent = null;
        if (meta && meta.pinvouScene) {
          runSyncOnSession(sid, function () {
            recordPinvouSceneForMessage(sid, submittedMessagePos, meta.pinvouScene);
          });
          trackSceneBehavior(sid, meta.pinvouScene);
        }
        return true;
      })
      .catch(function (err) {
        console.warn("[pinvou3][chat-ui] send failed", {
          sid,
          error: err && err.toString ? err.toString() : err,
        });
        emitPetEvent("pet:turn_end", sid);
        const errorText = String(err && err.message ? err.message : err || "");
        const concurrentTurn = errorText.includes("session_turn_in_progress");
        recordAuthoritySyncDiagnostic("local_turn_admission_failed", Object.assign({
          operation: "send",
          concurrent_turn: concurrentTurn,
          error_category: concurrentTurn ? "session_turn_in_progress" : "command_rejected",
          error_present: true,
        }, authoritySyncBufferSnapshot(sid, turnOwnerBuffer)));
        if (turnOwnerBuffer) turnOwnerBuffer.localTurnOwned = false;
        runSyncOnSession(sid, function () {
          state.messages = state.messages.filter(function (message) { return message !== submittedMessage; });
          state.chatItems = state.chatItems.filter(function (item) {
            return item.id !== submittedUserItemId && item.id !== submittedStreamId;
          });
          resetPendingAssistant();
          state.busy = false;
          stopThinking();
        });
        if (concurrentTurn && turnOwnerBuffer) {
          markRemoteTurn(sid, turnOwnerBuffer, false, "local_send_concurrent_turn");
        }
        runSyncOnSession(sid, function () {
          // 稳定错误码(如 image_input_unsupported)按码替换为三语指引,而非剥前缀
          // 透传后端硬编码中文——英/日界面不该看到中文结论;文案与 ChatView
          // 前置警告(t.uiAttachments.*)同源。与 web bridge displayTurnError
          // 同一口径(chat.rs IMAGE_INPUT_*_ERROR)。
          let errorText = String(err && err.toString ? err.toString() : err || "");
          if (errorText.indexOf("image_input_unsupported") === 0) {
            errorText = errorText.includes("能力未知")
              ? bt("imageUnknown")
              : bt("imageUnsupported");
          }
          addSystemItem(concurrentTurn
            ? bt("turnAlreadyInProgress")
            : "⚠️ " + errorText, {
            turnErrorNotice: true,
          });
        });
        notify();
        if (surfaceFailure) throw err;
        return false;
      });
  }
  // 远端用户消息不再由前端单独 invoke 发布:turn admission 时 Engine 侧统一
  // emit + 转发 chat:user_message(engine.rs emit_turn_admission),前端重复
  // 发布会造成远端双份气泡(旧 remote_control_publish_user_message 命令名
  // 在 Rust 侧从未注册,属 v1 遗留死调用,已删除)。
  // 本轮跑完(或被停止)后,若该 session 不忙且有排队消息 → 严格按 FIFO
  // 只发送队首一条。剩余消息留给后续 turn 的 done 继续逐条触发，避免把用户
  // 连续输入的多个独立任务合并成一个模型请求。
  function flushQueued(sid) {
    // 打断在途：排队消息让路，打断消息优先（否则 flush 先 reserve，
    // 打断消息反而撞 turn_in_progress 丢失）。
    if (interruptInFlight[sid]) return;
    const pendingBuffer = sessionStates[sid];
    if (pendingBuffer && pendingBuffer.remoteTurnActive) {
      reconcileRemoteTurn(sid).then(function (ready) {
        if (ready) flushQueued(sid);
      }).catch(function () {});
      return;
    }
    if (isBusyFor(sid)) return;            // doFinal 等又起了新 turn → 留给那轮的 done 再 flush
    const q = sid === state.activeSessionId ? state.queued : (sessionStates[sid] && sessionStates[sid].queued);
    if (!q || q.length === 0) return;
    // 队首是已投递引擎的 steer chip（等 chat:steer_committed 转气泡 / dropped
    // 移除）→ 让路不发送。它已在引擎侧排队，重复 doSendFor 会变两条消息。
    if (q[0].steered) return;
    const item = q.shift();
    const attachments = item.attachments || [];
    const displayText = item.displayText == null
      ? formatAttachmentDisplayText(item.text, attachments)
      : item.displayText;
    notify();
    doSendFor(sid, item.text, displayText, attachments, item.meta || null, !!item.restrictTools, true)
      .catch(function () {
        const retryQueue = sid === state.activeSessionId
          ? state.queued
          : (sessionStates[sid] && sessionStates[sid].queued);
        if (!retryQueue) return;
        retryQueue.unshift(item);
        notify();
      });
  }

  async function sendMessageToSession(sessionId, text, meta) {
    const sid = String(sessionId || "").trim();
    const content = String(text || "").trim();
    if (!sid) throw new Error(bt("targetSessionMissing"));
    if (!content) throw new Error(bt("replyContentEmpty"));
    const exists = state.sessions.some(function (session) { return String(session.id) === sid; });
    if (!exists) throw new Error(bt("targetSessionMissing"));

    await ensureSessionBufferLoaded(sid);
    let targetBuffer = getBuffer(sid);
    const targetQueue = targetBuffer && targetBuffer.queued;
    if (isBusyFor(sid) || (targetQueue && targetQueue.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push({
          id: ++context.itemIdSeq,
          text: content,
          displayText: content,
          attachments: [],
          meta: meta || null,
          restrictTools: false,
        });
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return { accepted: true, queued: true };
    }
    if (targetBuffer && targetBuffer.remoteTurnActive && !(await reconcileRemoteTurn(sid))) {
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "send_to_session",
      }, authoritySyncBufferSnapshot(sid, targetBuffer)));
      throw new Error(bt("targetSessionSyncing"));
    }
    targetBuffer = getBuffer(sid);
    if (isBusyFor(sid) || (targetBuffer.queued && targetBuffer.queued.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push({
          id: ++context.itemIdSeq,
          text: content,
          displayText: content,
          attachments: [],
          meta: meta || null,
          restrictTools: false,
        });
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return { accepted: true, queued: true };
    }
    const completion = doSendFor(sid, content, content, [], meta || null, false, true)
      .then(
        function () { return { ok: true }; },
        function (error) { return { ok: false, error }; }
      );
    return { accepted: true, queued: false, completion };
  }

  // steer invoke 返回 steer_id 后的回填:结算早到的 committed/dropped 事件;
  // chip 在回填前被 ×/⚡ 接走(cancelled)时补发撤回。
  function onSteerBackfill(queuedItem, sid, steerId) {
    // 旧后端(无返回值)时 chip 保持无 id,由 transcript_committed
    // 计数兜底结算;新后端回填 id 后立即结算可能早到的 committed/dropped。
    queuedItem.steerId = steerId || null;
    if (!queuedItem.steerId) return;
    if (queuedItem.cancelled) {
      withdrawSteerChip(sid, queuedItem);
      return;
    }
    settlePendingSteerEvent(sid, queuedItem);
    // 未被暂存事件立即结算的 chip 挂结算看门狗（引擎回收/悬挂兜底）。
    if (findSteerChipIndex(sid, queuedItem.steerId) >= 0) {
      armSteerSettleWatchdog(sid, queuedItem);
    }
  }

  // steer 失败(invoke reject/25s 超时;session 不存在/引擎没起):绝不静默
  // 降级 invoke("chat") —— busy 下必撞 session_turn_in_progress,文本会
  // 无痕蒸发。移除 chip、把文本恢复到输入框/草稿并提示,处置权还给用户。
  // 队列按 sid 路由:steer 最长 25s 才结算,期间用户可能已切走会话,
  // state.queued 已指向别会话工作集,必须按引用从目标会话队列取 chip
  // (同 steeredQueueFor 语义),否则 chip(steered:true, steerId:null)
  // 永久卡在后台会话队头,flushQueued 让路、该会话排队消息全部饿死。
  function onSteerFailure(queuedItem, sid, steerPreparation, steerInputText, err) {
    console.warn("[pinvou3][chat-ui] steer failed, restoring draft", {
      sid, error: err && err.toString ? err.toString() : err,
    });
    const failureQueue = steeredQueueFor(sid);
    const failureIndex = failureQueue ? failureQueue.indexOf(queuedItem) : -1;
    // 快照恢复带 sid 守卫(会话已切走时不覆盖新会话的 UI turn 状态)——
    // 与 sendMessage 内 restoreUiTurnState 同口径,这里显式展开(嵌套函数
    // 不可达)。
    const snap = steerPreparation.snapshot;
    if (snap && state.activeSessionId === sid) {
      state.scheduledTaskPendingGuide = snap.scheduledTaskPendingGuide;
      state.scheduledTaskCreationSessionId = snap.scheduledTaskCreationSessionId;
      state.scheduledTaskDraft = snap.scheduledTaskDraft;
      state.activeSkill = snap.activeSkill;
    }
    if (failureIndex >= 0 && state.activeSessionId === sid &&
        String(state.composerDraft || "").trim() === "") {
      // 仅在本回调真正接管 chip 且输入框为空时回填:chip 已被 ×/⚡ 接走
      // 时文本由那条路径处置,再回填会复活已放弃的文字或与 ⚡ 发送中的
      // 同文重复;输入框已有新内容时覆盖会砸掉用户等待期间打的字。
      failureQueue.splice(failureIndex, 1);
      setComposerDraft(steerInputText);
      prefillComposer(steerInputText);
    } else if (failureIndex >= 0) {
      // 输入框被占用或会话已切走:chip 原地降级为纯本地排队(同 ⚡ 失败
      // 降级语义),文本保留、由 flushQueued 在轮末发送,×/⚡ 仍可用。
      queuedItem.steered = false;
      queuedItem.steerId = null;
    }
    // 提示按 sid 路由:steer 最长 25s 才结算,期间用户可能已切走会话,
    // 无条件 addSystemItem 会把警告写进当前活跃会话的时间线。
    runSyncOnSession(sid, function () {
      addSystemItem("⚠️ " + bt("steerFailed"));
    });
    notify();
  }

  async function sendMessage(text, meta) {
    text = (text || "").trim();
    const readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return;
    // 还有解析中的附件 → 等
    if (state.attachments.some(function (a) { return a.status === "parsing"; })) {
      addSystemItem(bt("attachStillParsing"));
      return;
    }

    if (!state.activeSessionId) {
      // 草稿态首条消息 → 物化 session(命名靠下方 persistSession auto-title)。
      // 必须用返回值判空：切走场景 ensureSession 返回 null 但 activeSessionId
      // 非空（用户已切到别的会话），按 activeSessionId 继续会把本条消息发进
      // 错误会话（审计 #257）。
      const materialized = await ensureSession();
      if (!materialized) {
        // 物化中止（如草稿态多智能体开关落盘失败 / await 期间切走）：把输入放回
        // 输入框，不静默丢字；错误提示由 ensureSession 内如实给出（复核 P1）。
        prefillComposer(text);
        return;
      }
    }
    const sid = state.activeSessionId;
    function abandonPreparedAttachments() {
      state.attachments = state.attachments.filter(function (attachment) {
        return !readyAttachments.includes(attachment);
      });
      readyAttachments.forEach(function (attachment) {
        if (attachment && attachment.result) discardManagedAttachment(attachment.result);
      });
      notify();
    }
    try {
      await adoptManagedAttachments(readyAttachments, sid);
    } catch (error) {
      if (state.activeSessionId !== sid) {
        abandonPreparedAttachments();
        return;
      }
      addSystemItem(bt("deviceUploadFailed") + String(error && error.message ? error.message : error));
      return;
    }
    if (state.activeSessionId !== sid) {
      abandonPreparedAttachments();
      return;
    }
    const activeTurnBuffer = getBuffer(sid);
    // 展示文本：把附件 chip 名附在用户消息末尾
    const displayText = formatAttachmentDisplayText(text, readyAttachments);
    const attachmentsPayload = readyAttachments.map(function (a) { return a.result; });
    function consumeUiTurnState() {
      const consumed = {
        scheduledTaskPendingGuide: state.scheduledTaskPendingGuide,
        scheduledTaskCreationSessionId: state.scheduledTaskCreationSessionId,
        scheduledTaskDraft: state.scheduledTaskDraft,
        activeSkill: state.activeSkill,
      };
      const requestedPayloadText = meta && meta.pinvouPayloadText
        ? String(meta.pinvouPayloadText || "").trim()
        : "";
      let payloadText = requestedPayloadText || text;
      let restrictTools = false;
      if (state.scheduledTaskPendingGuide) {
        payloadText = state.scheduledTaskPendingGuide + "\n\n" + text;
        if (requestedPayloadText) payloadText = state.scheduledTaskPendingGuide + "\n\n" + requestedPayloadText;
        restrictTools = true;
        state.scheduledTaskPendingGuide = null;
        state.scheduledTaskCreationSessionId = sid;
        state.scheduledTaskDraft = null;
      }
      state.activeSkill = null;
      return { snapshot: consumed, payloadText, restrictTools };
    }
    function restoreUiTurnState(consumed) {
      if (!consumed || state.activeSessionId !== sid) return;
      state.scheduledTaskPendingGuide = consumed.scheduledTaskPendingGuide;
      state.scheduledTaskCreationSessionId = consumed.scheduledTaskCreationSessionId;
      state.scheduledTaskDraft = consumed.scheduledTaskDraft;
      state.activeSkill = consumed.activeSkill;
    }
    function queuePrepared(prepared) {
      state.queued.push({
        id: ++context.itemIdSeq,
        text: prepared.payloadText,
        displayText,
        attachments: attachmentsPayload,
        meta: meta || null,
        restrictTools: prepared.restrictTools,
      });
      state.attachments = state.attachments.filter(function (attachment) {
        return !readyAttachments.includes(attachment);
      });
      notify();
    }

    // Mid-turn inject(steer):busy 时发送文本 → chip 进排队浮层 + 立即
    // steer_chat 注入引擎,turn loop 在下次 step 边界自动嵌入(步骤间隙插入)。
    // chip 由 chat:steer_committed(转气泡)/ chat:steer_dropped(移除+提示)
    // 按 steer_id 结算;× 取消走 withdraw_steer 真撤回(见 removeQueued)。
    // 附件:steer 通道只载文本,带附件退回纯本地排队(queuePrepared,
    // 本轮 chat:done 后由 flushQueued 发送)。
    // busy 分支整体提取为嵌套函数(闭包可达 consumeUiTurnState 等局部
    // helper),把 sendMessage 认知复杂度压回 lint 阈值内。
    const busySteerBranch = function () {
      if (isBusyFor(sid)) {
        if (readyAttachments.length > 0) {
          const busyQueuePreparation = consumeUiTurnState();
          queuePrepared(busyQueuePreparation);
          return;
        }
        const steerPreparation = consumeUiTurnState();
        const steerText = steerPreparation.payloadText;
        const steerInputText = text;
        // 清空 composer draft(对齐 sendMessage 成功路径)。走 setComposerDraft
        // 同步会话 buffer,切走再切回不会复活已发送文字;后台会话的 steer
        // (远控等)不得清掉当前活跃会话的输入框。
        if (state.activeSessionId === sid) setComposerDraft("");
        // 排队 chip 立即显示。steered=true 标记已投递引擎：flushQueued 跳过它
        // （防重复发送）。steerId 在 invoke 返回后回填;事件可能早于 invoke
        // 返回,未决事件按 session 暂存(见下方 pendingSteerEvents)。
        const queuedItem = {
          id: ++context.itemIdSeq,
          text: steerText,
          displayText: steerText,
          attachments: [],
          meta: null,
          // steer 通道无 restrictTools 形参(steer_chat 仅携带文本),恒 false 是
          // 有意取舍:busy 下 scheduled-task guide 等受限场景降级为普通注入,
          // 不携带工具限制;非 busy 的排队路径仍保留 prepared.restrictTools。
          restrictTools: false,
          queuedAt: Date.now(),
          steered: true,
          steerId: null,
          cancelled: false,
        };
        state.queued.push(queuedItem);
        notify();
        // 回填/失败恢复提取为模块级函数(显式传参),也把 sendMessage 的
        // 认知复杂度压回 lint 阈值内。
        steer(sid, steerText)
          .then(function (steerId) { onSteerBackfill(queuedItem, sid, steerId); })
          .catch(function (err) { onSteerFailure(queuedItem, sid, steerPreparation, steerInputText, err); });
      }
    };

    if (isBusyFor(sid)) {
      busySteerBranch();
      return;
    }
    // 兼容旧行为:state.queued 非空时仍走 flushQueued(跨 session 远控等边缘场景)
    if (state.queued.length > 0) {
      const queuedPreparation = consumeUiTurnState();
      queuePrepared(queuedPreparation);
      flushQueued(sid);
      return;
    }
    if (activeTurnBuffer && activeTurnBuffer.remoteTurnActive &&
        !(await reconcileRemoteTurn(sid))) {
      if (state.activeSessionId !== sid) {
        abandonPreparedAttachments();
        return;
      }
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "send",
      }, authoritySyncBufferSnapshot(sid, activeTurnBuffer)));
      addAuthoritySyncNotice(bt("remoteTurnSyncing"));
      return;
    }
    if (state.activeSessionId !== sid) {
      abandonPreparedAttachments();
      return;
    }
    if (isBusyFor(sid) || state.queued.length > 0) {
      const racedQueuePreparation = consumeUiTurnState();
      queuePrepared(racedQueuePreparation);
      if (!isBusyFor(sid)) flushQueued(sid);
      return;
    }

    const preparation = consumeUiTurnState();
    // surfaceFailure=true：主路径失败（reserve 冲突/命令拒绝等）必须抛给
    // ChatView——它已清空输入框并负责恢复文字/prefill 追加；bridge 吞错
    // 返回会让「消息绝不静默丢失」的承诺落空（第五轮评审遗留项）。
    // surfaceFailure 下 doSendFor 只会 resolve(true) 或 throw，无 false 分支。
    try {
      await doSendFor(
        sid,
        preparation.payloadText,
        displayText,
        attachmentsPayload,
        meta,
        preparation.restrictTools,
        true,
      );
    } catch (err) {
      if (state.activeSessionId === sid) {
        restoreUiTurnState(preparation.snapshot);
        notify();
      } else {
        abandonPreparedAttachments();
      }
      throw err;
    }
    state.attachments = state.attachments.filter(function (attachment) {
      return !readyAttachments.includes(attachment);
    });
    notify();
  }
  // WebUI 草稿首条消息失败时的专用重试入口。桌面端没有远程草稿，
  // 保留同名空实现以维持跨宿主 Bridge API 的稳定形状。
  function retryFirstTurn() {}
  function prefillComposer(text) {
    state.composerPrefill = { id: (state.composerPrefill.id || 0) + 1, text: String(text || "") };
    notify();
  }
  // 撤销一条待发消息(点 chip 的 ×)。纯排队 chip(含附件)= 纯本地移除 +
  // discard 附件,零引擎调用;steered chip = 乐观移除 + withdraw_steer 真撤回
  // (撤回结果由 chat:steer_dropped 确认,撤回太迟则 committed 补气泡,
  // 见 settleSteerCommitted 的竞态注释)。
  function removeQueued(id) {
    const removed = state.queued.find(function (q) { return q.id === id; });
    if (removed && removed.attachments) {
      removed.attachments.forEach(discardManagedAttachment);
    }
    state.queued = state.queued.filter(function (q) { return q.id !== id; });
    if (removed && removed.steered) withdrawSteerChip(state.activeSessionId, removed);
    notify();
  }

  // ── Pinvou v4 召唤式检阅:Boss 主动呼叫,审当前 session 前面的工作 ──
  // 设计 docs/品悟v4-常驻检阅助手设计.md。纯召唤、不替 Boss 决策。
  // 审查卡进 chatItems(当前会话可见);跨会话持久化(进 messages/独立存储)是 §6 后续增强。
  async function summonPinvou(focus, mode) {
    if (!state.activeSessionId) { addSystemItem(bt("summonNeedsSession")); return; }
    if (state.pinvouSummoning) return;
    state.pinvouSummoning = true;
    const sid = state.activeSessionId; // 召唤发起时的 session;await 返回后校验,防跨 session 串(召唤慢+切走)
    // 检阅结果弹 modal(不进对话流):一次只一个,裁决/跳过直接操作 state.pinvouModal.review、
    // 不靠 pos 定位(根治连续召唤 pos 重复串卡)。
    state.pinvouModal = { loading: true, coverage: mode === "coverage" };
    notify();
    try {
      // focus=产出物 path(品=审产物); mode="coverage"=悟(通盘体检)。
      const review = await invoke("summon_pinvou", { sessionId: sid, focus: focus || null, mode: mode || null });
      if (state.activeSessionId !== sid) return; // 召唤期间切了 session → 丢弃,绝不 record/写进别的 session
      recordPinvouReview(review); // 存 sidecar(供核账读上轮账目);modal.review 同引用,裁决写它=写 sidecar
      if (state.pinvouModal) { state.pinvouModal.loading = false; state.pinvouModal.review = review; }
    } catch (e) {
      if (state.activeSessionId === sid && state.pinvouModal) { state.pinvouModal.loading = false; state.pinvouModal.error = String(e && e.message ? e.message : e); }
    } finally {
      state.pinvouSummoning = false;
      notify();
    }
  }

  // 通盘体检(覆盖镜头):查产物"全不全"=缺哪些完整性维度。独立入口,走 mode=coverage。
  function inspectPinvou(focus) {
    return summonPinvou(focus, "coverage");
  }

  // B2: 审查卡进 sidecar 时间线(pos=当前 messages 数),落盘。同 recordPersonaEvent
  // 范式,**不进 messages/LLM**;rerenderFromMessages 按 pos 插回,切会话/重载不丢。
  function recordPinvouReview(review) {
    if (!state.activeSessionId || !review) return null;
    const pos = state.messages.length;
    state.pinvouReviews.push({ pos, review });
    const sid = state.activeSessionId;
    const snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    invoke("save_session_pinvou_reviews", { sessionId: sid, reviews: snapshot }).catch(function () {});
    return pos; // 供卡片记 reviewPos,裁决时按 pos 定位原 state 写 resolution
  }

  // §2 按勾选裁决:resolution 已由前端写回 review 对象(引用→sidecar),这里持久化 +
  // 把勾「让AI改」的条目走 B1 发定向修订指令(只改对应段落、禁全文重写)。Boss 驾驶,非自动。
  async function resolvePinvouReview(resolutions, actions) {
    // 检阅发生的会话归属捕获：persist 挂起期间用户可能切走，修订指令必须发回
    // 检阅会话，不得漂进当前 active 会话（审计）。
    const reviewSid = state.activeSessionId;
    // 弹窗只一个 review(state.pinvouModal.review),直接在它上面写 resolution——不靠 pos 定位
    // (根治连续召唤 pos 重复串卡)。它和 sidecar entry.review 同引用,写它=写 sidecar。
    const isWu = !!(state.pinvouModal && state.pinvouModal.coverage); // 关窗前取,供转交标品/悟
    const review = state.pinvouModal && state.pinvouModal.review;
    if (review && resolutions) {
      (review.recommendations || []).forEach(function (r, k) { if (resolutions.recs && resolutions.recs[k]) r.resolution = resolutions.recs[k]; });
      (review.issues || []).forEach(function (x, k) { if (resolutions.issues && resolutions.issues[k]) x.resolution = resolutions.issues[k]; });
      (review.coverage || []).forEach(function (g, k) { if (resolutions.coverage && resolutions.coverage[k]) g.resolution = resolutions.coverage[k]; });
    }
    await persistPinvouReviews(); // 落盘,配合后端 preserve_resolutions 防覆盖
    state.pinvouModal = null; // 裁决完关窗
    notify();
    if (!actions || !actions.length) return;
    // 按动作类型分组,组装一条 Boss 消息发给主 AI(Boss 驾驶,非自动回传):
    //   fix/verify=产物缺陷定向修订(verify 先核实);adopt=Boss 已定的决策;ask=让 AI 正式问。
    const fix = actions.filter(function (a) { return a.t === "fix"; });
    const verify = actions.filter(function (a) { return a.t === "verify"; });
    const adopt = actions.filter(function (a) { return a.t === "adopt"; });
    const ask = actions.filter(function (a) { return a.t === "ask"; });
    const parts = [];
    if (fix.length) {
      parts.push("请按下面的检阅意见，**只定向修改对应段落，不要全文重写**：");
      fix.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (verify.length) {
      if (parts.length) parts.push("");
      parts.push("以下几条涉及外部事实，**先查证再改、标明依据，别凭记忆直接改**：");
      verify.forEach(function (a) { parts.push("- " + a.text); });
    }
    if (adopt.length) {
      if (parts.length) parts.push("");
      parts.push("以下事项我已拍板，按此更新产物：");
      adopt.forEach(function (a) { parts.push("- " + (a.topic ? a.topic + "：" : "") + a.pick); });
    }
    if (ask.length) {
      if (parts.length) parts.push("");
      parts.push("以下待定项请用 request_user_input 正式问我，别自己猜：");
      ask.forEach(function (a) { parts.push("- " + a.topic); });
    }
    const fill = actions.filter(function (a) { return a.t === "fill"; });
    if (fill.length) {
      if (parts.length) parts.push("");
      parts.push("以下维度产物还缺，请补充进去（保留其余、只增不改）：");
      fill.forEach(function (a) { parts.push("- " + a.dimension + (a.suggestion ? "：" + a.suggestion : "")); });
      parts.push("（涉及外部事实的，先查证再写、标依据，别凭记忆编。）");
    }
    // 已切走则放弃发指令（修订指令属于检阅会话，漂进别的会话会误导其上下文）。
    if (parts.length && reviewSid && state.activeSessionId === reviewSid) sendMessage(parts.join("\n"), { pinvouTransfer: isWu ? "悟" : "品" });
  }

  // 整卡跳过:Boss 看了不处理这次检阅 → 直接关窗(sidecar entry 留着、无 resolution,无害)。
  function dismissPinvouReview() {
    // 关窗即解召唤守卫:否则若在 await 期间被关(切 session 等路径),会留下"窗没了但
    // pinvouSummoning 仍 held"的死区——重复点品/悟在守卫处(summonPinvou 开头)被吞,要等
    // 整个直连 vLLM 调用(≤30s)返回才解锁。in-flight 结果靠 summonPinvou 内 `if (state.pinvouModal)` 守卫自然丢弃。
    state.pinvouModal = null;
    state.pinvouSummoning = false;
    notify();
  }
  // 把当前 session 的审查时间线(含勾选写回的 resolution)重新落盘。返回 promise 供 await。
  function persistPinvouReviews() {
    if (!state.activeSessionId) return Promise.resolve();
    const snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    return invoke("save_session_pinvou_reviews", { sessionId: state.activeSessionId, reviews: snapshot }).catch(function () {});
  }

  // Mid-turn inject 通道(steer_chat 命令的薄封装)。steer_id 契约:
  // steer_chat 成功 resolve opaque steer_id(如 "steer-3"),session 不存在/
  // 引擎没起时 reject。busy 时的 plain send 走这里(见 sendMessage);
  // 远控/其他宿主也可用。
  async function steer(sid, content) {
    safeConsoleInfo("[pinvou3][chat-ui] steer start", { sid, len: (content || "").length });
    // invoke 无传输层超时:引擎卡住(不死不 drain)时 steer_chat 永不结算,
    // chip(steered:true, steerId:null)会卡住队头并阻塞 flushQueued。超时
    // 走 catch 的失败恢复(移除 chip、恢复输入框)。
    const invokePromise = invoke("steer_chat", { sessionId: sid, content: String(content || "") });
    let timeoutId = null;
    let timedOut = false;
    const timeout = new Promise(function (_, reject) {
      timeoutId = setTimeout(function () {
        timedOut = true;
        reject(new Error("steer_chat timed out"));
      }, STEER_INVOKE_TIMEOUT_MS);
    });
    let steerId;
    // 评审 P1-2：超时后 invoke 晚到成功——引擎已接受原消息并可能在后续
    // step 边界注入，与用户基于恢复文字的重发形成重复投递。晚到成功一律
    // 补偿撤回：未注入则永不注入（底座 withdrawn 标记幂等、恰好一条
    // SteerDropped）；已 committed 时撤回 no-op，迟到的 committed 事件经
    // withdrawn 登记补气泡（与重发同文时去重已有处理）。必须在 race 之前
    // 注册——race 因超时 reject 后本函数已 throw，之后的代码不会执行。
    invokePromise.then(function (lateId) {
      if (!timedOut || lateId == null) return;
      rememberWithdrawn(sid, String(lateId), String(content || ""));
      invoke("withdraw_steer", { sessionId: sid, steerId: String(lateId) })
        .catch(function () { /* 引擎不在场 = 不会再注入 */ });
    });
    try {
      steerId = await Promise.race([invokePromise, timeout]);
    } finally {
      clearTimeout(timeoutId);
    }
    invokePromise.catch(function () { /* 超时后正常 reject 吞掉,避免 unhandledrejection */ });
    safeConsoleInfo("[pinvou3][chat-ui] steer accepted", { sid, steerId });
    return steerId == null ? null : String(steerId);
  }

  // steer chip 结算看门狗（引擎回收/悬挂兜底，第五轮评审）：steerId 回填后
  // 若引擎被回收（30min 空闲/切模型/多智能体开关，forwarder 先 abort 导致
  // SteerDropped 丢失）或卡在「最后一次 drain 之后、finish 之前」的窗口，
  // committed/dropped 永远不到达，steered 队头会让 flushQueued 永久让路——
  // 该会话再也发不出消息。看门狗到时后先 best-effort 撤回（引擎活着则撤回
  // 生效、后续不会再注入，不重复投递；已回收则 Err/no-op），再把 chip 降级
  // 为普通排队消息由 flushQueued 消费——消息不丢、不悬挂。
  // 60s 而非对称 25s：长工具调用（分钟级 shell）期间 steer 合法地等待下个
  // step 边界，过短的看门狗会把健康引擎的 steer 误降级为下轮独立发送。
  const STEER_SETTLE_WATCHDOG_MS = 60000;
  const steerSettleWatchdogs = {}; // sid -> { steerId: timerId }
  function clearSteerSettleWatchdog(sid, steerId) {
    const byId = steerSettleWatchdogs[sid];
    if (!byId || !byId[steerId]) return;
    clearTimeout(byId[steerId]);
    delete byId[steerId];
  }
  function armSteerSettleWatchdog(sid, item) {
    const steerId = item.steerId;
    if (!steerId) return;
    let byId = steerSettleWatchdogs[sid];
    if (!byId) {
      byId = Object.create(null);
      steerSettleWatchdogs[sid] = byId;
    }
    byId[steerId] = setTimeout(function () {
      delete byId[steerId];
      const q = steeredQueueFor(sid);
      if (!q || !q.includes(item) || !item.steered || item.steerId !== steerId) return;
      // best-effort 撤回（底座幂等；已回收时 Err），随后降级为纯本地排队。
      rememberWithdrawn(sid, steerId, item.text);
      invoke("withdraw_steer", { sessionId: sid, steerId })
        .catch(function () { /* 引擎已回收 = 不会再注入 */ });
      item.steered = false;
      item.steerId = null;
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
    }, STEER_SETTLE_WATCHDOG_MS);
  }

  // ⚡ not_pending 对账看门狗（底座契约：NotPending 不是已投递证明——宿主
  // 必须等对账终态事件）。committed/dropped 因引擎回收丢失时，60s 后把
  // 文字恢复到输入框并提示，不把不确定的消息当作投递成功。事件正常到达
  // 时 settle 函数清掉看门狗（committed 渲染气泡 / dropped 静默），
  // 看门狗随后空转 no-op。
  const outcomeReconcileWatchdogs = {}; // sid -> { steerId: timerId }
  function clearOutcomeReconcileWatchdog(sid, steerId) {
    const byId = outcomeReconcileWatchdogs[sid];
    if (!byId || !byId[steerId]) return;
    clearTimeout(byId[steerId]);
    delete byId[steerId];
  }
  function armOutcomeReconcileWatchdog(sid, item) {
    const steerId = item.steerId;
    if (!steerId) return;
    let byId = outcomeReconcileWatchdogs[sid];
    if (!byId) {
      byId = Object.create(null);
      outcomeReconcileWatchdogs[sid] = byId;
    }
    byId[steerId] = setTimeout(function () {
      delete byId[steerId];
      const text = takeWithdrawn(sid, steerId);
      if (text === undefined) return; // 已对账（settle 消费过登记）
      runSyncOnSession(sid, function () {
        if (String(state.composerDraft || "").trim() === "") setComposerDraft(text);
        else prefillComposer(text);
        addSystemItem("⚠️ " + bt("steerFailed"));
      });
      notify();
    }, STEER_SETTLE_WATCHDOG_MS);
  }

  // steer 事件未决暂存:chat:steer_committed / chat:steer_dropped 可能早于
  // invoke("steer_chat") 返回到达(引擎极快 committed 时事件先派发),此时
  // chip 还没回填 steerId,无法匹配。按 session 暂存 steer_id → 结果,
  // chip 回填 id 时立即结算(sendMessage 的 steer().then)。
  // chip 在回填前被用户 × 移除时暂存项会残留,量极小且 steer_id 唯一,可接受。
  const pendingSteerEvents = {};
  function stashSteerEvent(sid, steerId, kind) {
    let byId = pendingSteerEvents[sid];
    if (!byId) {
      byId = Object.create(null);
      pendingSteerEvents[sid] = byId;
    }
    byId[steerId] = kind;
  }
  function takeSteerEvent(sid, steerId) {
    const byId = pendingSteerEvents[sid];
    if (!byId || !byId[steerId]) return null;
    const kind = byId[steerId];
    delete byId[steerId];
    return kind;
  }
  // 已主动撤回的 steer(chip 已乐观移除):此后 dropped 到达 = 撤回生效,
  // 静默(不重复提示);committed 到达 = 撤回太迟(引擎已注入),以引擎为准
  // 用暂存文本补气泡。key = steer_id,值 = 消息文本。
  const withdrawnSteers = {};
  // 会话删除/驱逐时清掉本会话的 steer 中间状态:暂存事件/撤回文本(含用户
  // 消息文本,不应驻留已删会话)/打断在途标记。引擎侧残留由底座
  // SyncSession/Shutdown 的 SteerDropped 兜底,前端无需等事件。
  function purgeSteerState(sid) {
    delete pendingSteerEvents[sid];
    delete withdrawnSteers[sid];
    delete interruptInFlight[sid];
    const watchdogs = steerSettleWatchdogs[sid];
    if (watchdogs) {
      for (const steerId of Object.keys(watchdogs)) clearTimeout(watchdogs[steerId]);
      delete steerSettleWatchdogs[sid];
    }
    const reconcileWatchdogs = outcomeReconcileWatchdogs[sid];
    if (reconcileWatchdogs) {
      for (const steerId of Object.keys(reconcileWatchdogs)) clearTimeout(reconcileWatchdogs[steerId]);
      delete outcomeReconcileWatchdogs[sid];
    }
  }

  function rememberWithdrawn(sid, steerId, text) {
    let byId = withdrawnSteers[sid];
    if (!byId) {
      byId = Object.create(null);
      withdrawnSteers[sid] = byId;
    }
    byId[steerId] = String(text || "");
  }
  function takeWithdrawn(sid, steerId) {
    const byId = withdrawnSteers[sid];
    if (!byId || !Object.prototype.hasOwnProperty.call(byId, steerId)) return;
    const text = byId[steerId];
    delete byId[steerId];
    return text;
  }
  function steeredQueueFor(sid) {
    return sid === state.activeSessionId
      ? state.queued
      : (sessionStates[sid] && sessionStates[sid].queued);
  }
  function findSteerChipIndex(sid, steerId) {
    const q = steeredQueueFor(sid);
    if (!q) return -1;
    for (let i = 0; i < q.length; i++) {
      if (q[i] && q[i].steered && q[i].steerId && q[i].steerId === steerId) return i;
    }
    return -1;
  }
  // 撤回一条 steered chip:有 id → 立即 invoke withdraw_steer 并登记
  // (引擎不在场时 Err = 消息根本没进引擎,纯本地移除即可);id 未回填
  // (invoke 在途)→ 打取消标记,sendMessage 的回填回调里补发撤回。
  function withdrawSteerChip(sid, item) {
    if (!item || !item.steered) return;
    if (!item.steerId) {
      item.cancelled = true;
      return;
    }
    if (!sid) return;
    clearSteerSettleWatchdog(sid, item.steerId);
    rememberWithdrawn(sid, item.steerId, item.text);
    invoke("withdraw_steer", { sessionId: sid, steerId: item.steerId })
      .catch(function () { /* 引擎不在场 = 消息未进引擎,无需后续动作 */ });
  }
  // 事件先到、chip 后到(回填 steerId)时的立即结算。
  function settlePendingSteerEvent(sid, item) {
    if (!item || !item.steerId) return;
    const kind = takeSteerEvent(sid, item.steerId);
    if (!kind) return;
    // chip 可能已被用户 × 移除:队列里找不到就丢弃该事件结果。
    if (findSteerChipIndex(sid, item.steerId) < 0) return;
    if (kind === "committed") settleSteerCommitted(sid, item.steerId);
    else settleSteerDropped(sid, item.steerId);
  }
  // chat:steer_committed 结算:chip 转用户气泡。找不到 chip 时:若是我们
  // 主动撤回的(撤回太迟,引擎已注入)→ 以引擎为准补气泡;否则暂存,
  // 等回填 id 后由 settlePendingSteerEvent 结算。
  function settleSteerCommitted(sid, steerId) {
    clearSteerSettleWatchdog(sid, steerId);
    clearOutcomeReconcileWatchdog(sid, steerId);
    if (findSteerChipIndex(sid, steerId) < 0) {
      const withdrawnText = takeWithdrawn(sid, steerId);
      if (withdrawnText !== undefined) {
        runSyncOnSession(sid, function () {
          // ⚡ 成功路径的 doSendFor 已渲染同文气泡 → 跳过防重(竞态窗口极小)。
          let lastUser = null;
          for (let i = state.chatItems.length - 1; i >= 0; i--) {
            if (state.chatItems[i] && state.chatItems[i].type === "user") { lastUser = state.chatItems[i]; break; }
          }
          if (lastUser && lastUser.text === withdrawnText) return;
          addChatItem({ type: "user", text: withdrawnText, time: timeStr() });
        });
        notify();
        return;
      }
      stashSteerEvent(sid, steerId, "committed");
      return;
    }
    runSyncOnSession(sid, function () {
      const q = steeredQueueFor(sid);
      const index = findSteerChipIndex(sid, steerId);
      if (!q || index < 0) return;
      const item = q[index];
      q.splice(index, 1);
      // 消息已进引擎 transcript；气泡用 chip 文本渲染，transcript_committed
      // 稍后会把 state.messages 同步为权威版本。
      state.chatItems = state.chatItems.filter(function (ci) {
        return !ci.turnErrorNotice && !ci.authoritySyncNotice;
      });
      addChatItem({ type: "user", text: item.text, time: timeStr() });
    });
    notify();
  }
  // chat:steer_dropped 结算:移除 chip + 提示。我们主动撤回的(chip 已乐观
  // 移除)静默,不重复提示。
  function settleSteerDropped(sid, steerId) {
    clearSteerSettleWatchdog(sid, steerId);
    clearOutcomeReconcileWatchdog(sid, steerId);
    if (findSteerChipIndex(sid, steerId) < 0) {
      if (takeWithdrawn(sid, steerId) !== undefined) return;
      stashSteerEvent(sid, steerId, "dropped");
      return;
    }
    runSyncOnSession(sid, function () {
      const q = steeredQueueFor(sid);
      const index = findSteerChipIndex(sid, steerId);
      if (!q || index < 0) return;
      q.splice(index, 1);
      addSystemItem("⚠️ " + bt("steerDropped"));
    });
    notify();
  }
  // Mid-turn INTERRUPT: 打断当前 AI 步骤,立刻起新 turn 发送。
  // 与 steer 区别:steer 等下次 step 边界自然嵌入(不打断 tool 调用),
  // interrupt 立刻 cancel 当前 turn,起新 turn,消息进 chat 命令路径。
  //
  // 事件驱动同步:不轮询 state.busy,而是 await chat:done 事件本身。
  // state.busy 在 chat:done handler 内同步置 false,事件触发即代表
  // turn lifecycle 已完成 cancel + cleanup,可以安全 reserve 新 turn。
  // 长 tool chain 场景下 25s 兜底超时(避免 cancel 永久挂起;5s 对长
  // 工具链收尾偏短,会过早走失败恢复路径)。
  //
  // generation 匹配(P0-B):chat:done payload 带后端轮次身份(generation),
  // 只对目标轮 resolve —— 迟到的旧轮终态、其他轮的终态都不会提前解锁等待。
  // 旧后端(无 generation 字段)时退化为按 sid 匹配的旧行为。
  // 返回值：true = 确实观察到目标轮终态（chat:done / busy 清除）；false =
  // 兜底超时。调用方据此前置区分「可以安全 reserve」与「取消 unwind 未完结」。
  // chat:done 观察器（评审 P2）：打断时在 cancel **之前**注册监听并缓冲
  // 事件——旧实现在 cancel 返回后才 listen，事件落在「cancel 返回 →
  // 注册生效」缝隙里会被错过，terminal=false 时假等 25s 兜底。watcher
  // 缓冲该会话全部 done（含 generation），waitFor 先查缓冲再等未来事件；
  // cancel() 退订并清场。无事件通道（测试/web 模式）时返回 null，调用方
  // 退回 waitForChatDone 的 busy 轮询。
  function createChatDoneWatcher(sid) {
    if (!TAURI || !TAURI.event || typeof TAURI.event.listen !== "function") return null;
    const buffered = [];
    const waiters = [];
    let unlisten = null;
    let closed = false;
    // 与 waitForChatDone 同口径：任一端无 generation（旧后端）退化为按 sid 匹配。
    function matches(generation, observed) {
      if (generation == null || observed == null) return true;
      return Number(observed) === Number(generation);
    }
    function onEvent(e) {
      if (!e || !e.payload || e.payload.session_id !== sid) return;
      const gen = e.payload.generation == null ? null : Number(e.payload.generation);
      buffered.push(gen);
      for (let i = waiters.length - 1; i >= 0; i--) {
        if (matches(waiters[i].generation, gen)) {
          waiters.splice(i, 1)[0].finish(true);
        }
      }
    }
    const listenPromise = TAURI.event.listen("chat:done", onEvent);
    if (listenPromise && typeof listenPromise.then === "function") {
      listenPromise.then(function (un) {
        if (closed) { try { un(); } catch { /* already gone */ } return; }
        unlisten = un;
      }).catch(function () {});
    }
    return {
      // 监听就绪（注册 Promise 结算）后才算真正闭合缝隙；调用方在 cancel 前 await。
      ready: listenPromise && typeof listenPromise.then === "function"
        ? listenPromise.then(function () {}).catch(function () {})
        : Promise.resolve(),
      waitFor: function (generation, timeoutMs) {
        if (closed) return Promise.resolve(false);
        if (buffered.some(function (gen) { return matches(generation, gen); })) {
          return Promise.resolve(true);
        }
        return new Promise(function (resolve) {
          const w = {
            generation,
            timer: null,
            finish: function (observed) {
              if (w.timer) { clearTimeout(w.timer); w.timer = null; }
              resolve(observed);
            },
          };
          w.timer = setTimeout(function () {
            const idx = waiters.indexOf(w);
            if (idx >= 0) waiters.splice(idx, 1);
            w.finish(false);
          }, timeoutMs);
          waiters.push(w);
        });
      },
      cancel: function () {
        closed = true;
        if (unlisten) { try { unlisten(); } catch { /* already gone */ } unlisten = null; }
        for (const w of waiters.splice(0)) w.finish(false);
      },
    };
  }

  function waitForChatDone(sid, generation, timeoutMs) {
    return new Promise(function (resolve) {
      let timer = null;
      let resolved = false;
      let unlisten = null;
      function done(observed) {
        if (resolved) return;
        resolved = true;
        if (timer) { clearTimeout(timer); timer = null; }
        if (unlisten && typeof unlisten === "function") {
          try { unlisten(); } catch { /* unlisten may already be gone */ }
          unlisten = null;
        }
        resolve(observed === true);
      }
      // 通过 TAURI.event.listen 直接订阅 webview 事件,匹配 sid 后 resolve。
      // Tauri 2 的 listen 返回 Promise<UnlistenFn>,on 收到事件即回调。
      if (TAURI && TAURI.event && typeof TAURI.event.listen === "function") {
        const p = TAURI.event.listen("chat:done", function (e) {
          if (!e || !e.payload || e.payload.session_id !== sid) {
            return;
          }
          const payloadGeneration = e.payload.generation;
          if (generation != null && payloadGeneration != null &&
              Number(payloadGeneration) !== Number(generation)) {
            return;
          }
          done(true);
        });
        if (p && typeof p.then === "function") {
          p.then(function (un) {
            // listen 的 Promise 可能晚于超时/事件 resolve:此时立即退订,
            // 否则监听器泄漏一个(有 resolved 守卫无功能危害,但会累积)。
            if (resolved) { try { un(); } catch { /* already unlistened */ } return; }
            unlisten = un;
          }).catch(function () {});
        }
      } else {
        // 兜底:轮询 busy(for 测试环境或 web 模式)
        const deadline = Date.now() + timeoutMs;
        const poll = function () {
          if (resolved) return;
          if (!isBusyFor(sid)) { done(true); return; }
          if (Date.now() >= deadline) { done(false); return; }
          setTimeout(poll, 50);
        };
        poll();
      }
      timer = setTimeout(function () {
        done(false);
      }, timeoutMs);
    });
  }

  async function interruptAndSend(sid, text, displayText, attachments, meta, restrictTools) {
    safeConsoleInfo("[pinvou3][chat-ui] interrupt-and-send start", { sid });
    interruptInFlight[sid] = true;
    try {
      // 1) cancel 当前 turn。cancel_generation 返回 CancelOutcome { generation, terminal }：
      //    terminal=true（claim 路径终态已由 cancel 自身确认 / 目标轮已结束 / 空闲）
      //    → 无需等待事件；false → 等待携带目标 generation 的 chat:done（事件驱动）。
      //    这消除了两处确定性竞态：claim 路径的 chat:done 发在 cancel 返回之前
      //    （监听器必然错过）、turn 刚自然结束时 cancel no-op 不再有事件——二者
      //    前端都无法靠等事件收敛，只能由命令返回值确认终态。
      // 按 sid 判断 busy(await 期间用户可能切换会话,state.busy 会张冠李戴)。
      if (isBusyFor(sid)) {
        // 评审 P2：先注册并等监听就绪再 cancel，闭合「cancel 返回 → listen
        // 注册」缝隙里 chat:done 丢失导致的 25s 假等待；watcher 为 null
        // （无事件通道）时退回 busy 轮询。
        const watcher = createChatDoneWatcher(sid);
        if (watcher) await watcher.ready;
        let outcome = null;
        try {
          // keepInbox=true（打断语义）：未注入的 steer 保留给下一轮，排队
          // chip 不被静默取消；停止按钮（cancelGeneration）不传此参数，
          // 后端按 false 清空未注入 steer 并发 chat:steer_dropped。
          outcome = await invoke("cancel_generation", { sessionId: sid, keepInbox: true });
        } catch (e) {
          console.warn("[pinvou3][chat-ui] cancel failed before interrupt", e);
        }
        const terminal = !!(outcome && outcome.terminal);
        const generation = outcome && outcome.generation;
        if (terminal) {
          if (watcher) watcher.cancel();
        } else {
          // 事件驱动等待（P0-B）：后端保证 chat:done 到达时 reserve 闸门已重开，
          // 不再需要固定 sleep 补窗口。超时仅作最后兜底。
          const observed = watcher
            ? await watcher.waitFor(generation, 25000)
            : await waitForChatDone(sid, generation, 25000);
          if (watcher) watcher.cancel();
          if (!observed && isBusyFor(sid)) {
            // 25s 兜底超时且会话仍 busy：取消 unwind 未完结，此时 doSendFor
            // 会稳定撞 session_turn_in_progress。bridge.chat.interruptAndSend
            // 是公开 API（远控/其他宿主），调用方可能不接异常——这里显式
            // 失败、不尝试发送，消息由调用方恢复（UI 的 ⚡/chip 路径均有
            // catch 恢复），绝不静默丢消息。会话已不 busy（监听器错过事件
            // 但 turn 实际结束）时继续走发送。
            throw new Error(
              "interrupt-and-send: cancel did not reach terminal within 25s and session is still busy; message not sent"
            );
          }
        }
      }
      // 2) 不整体清空 queue：打断只放弃当前轮进度，保留用户排队中的其他消息
      //    （远控注入的 steer 由引擎侧 keepInbox 语义保留给下一轮）。
      // 3) 附件由调用方随消息传入（排队 chip 的 attachments 已是 payload 形态）。
      const attachmentPayload = attachments || [];
      // 4) 真正发新消息；失败时由调用方负责恢复（chip ⚡ 恢复回排队区）。
      const result = await doSendFor(sid, text, displayText, attachmentPayload, meta, restrictTools, true);
      return result;
    } finally {
      interruptInFlight[sid] = false;
    }
  }

  // 排队 chip 的 ⚡ 瞬发:把该 chip 从队列移除(其附件随消息发出,不 discard),
  // 走 interruptAndSend 的 cancel + doSendFor 链路。busy 时打断当前生成;
  // 非 busy(队列残留未被 flushQueued 消费)时直接发送、不 cancel。
  // steered chip 先撤回引擎里那份并 await 明确 outcome(评审 P1-1):
  // retired = 引擎副本已标记撤回、永不注入,安全重发;not_pending =
  // 已 committed(注入完成/进行中),不得重发——气泡由迟到或暂存的
  // steer_committed 渲染(以引擎为准,同文去重已有处理)。
  // 失败时把消息按原位恢复到排队区(不是输入框)并提示
  // ——恢复的 chip 降级为非 steered:撤回已发出,引擎只在下一轮 drain 时才
  // 结算,保持 steered 会让 flushQueued 让路且下一轮永远不来(排队区卡死);
  // 引擎侧残留由迟到的 steer_committed 补气泡,去重已有处理。
  async function interruptAndSendQueued(sid, queuedId) {
    const queueOf = function () {
      return sid === state.activeSessionId
        ? state.queued
        : (sessionStates[sid] && sessionStates[sid].queued);
    };
    const q = queueOf();
    if (!q) return false;
    let index = -1;
    for (let i = 0; i < q.length; i++) {
      if (q[i] && q[i].id === queuedId) { index = i; break; }
    }
    if (index < 0) return false; // chip 已不在(重复点击/已取消)——天然 single-flight
    const item = q[index];
    q.splice(index, 1);
    let skipResend = false;
    if (item.steered && item.steerId && sid) {
      clearSteerSettleWatchdog(sid, item.steerId);
      rememberWithdrawn(sid, item.steerId, item.text);
      let outcome;
      try {
        outcome = await invoke("withdraw_steer", { sessionId: sid, steerId: item.steerId });
      } catch {
        // 引擎不在场 = 消息根本没进引擎(或已随回收销毁),正常重发。
        outcome = "retired";
      }
      skipResend = outcome === "not_pending";
    } else {
      // steerId 未回填(invoke 在途)或非 steered:打取消标记,回填回调补撤回。
      withdrawSteerChip(sid, item);
    }
    notify();
    if (skipResend) {
      // 已注入,不重复发送。事件早于回填到达而被暂存的 committed 立即补气泡;
      // 迟到的 committed 由 withdrawn 登记路径渲染。NotPending 不是已投递
      // 证明（底座契约）：等对账事件，事件丢失（引擎回收）时看门狗把文字
      // 恢复到输入框并提示，不把不确定的消息当作投递成功。
      if (takeSteerEvent(sid, item.steerId) === "committed") {
        settleSteerCommitted(sid, item.steerId);
      } else {
        armOutcomeReconcileWatchdog(sid, item);
      }
      return true;
    }
    try {
      return await interruptAndSend(
        sid, item.text, item.displayText, item.attachments || [], item.meta || null, !!item.restrictTools
      );
    } catch (e) {
      console.warn("[pinvou3][chat-ui] interrupt-queued failed, restoring chip", {
        sid, error: e && e.toString ? e.toString() : e,
      });
      // 恢复的 chip 降级为非 steered(纯本地排队):撤回已发给引擎,引擎只在
      // 下一轮 drain 时才发 steer_dropped 结算;而 chat 发送已失败,若保留
      // steered 标记,flushQueued 会因 steered 队头让路且下一轮永远不来,
      // 排队区卡死。降级后该 chip 由 flushQueued 正常消费;引擎侧残留(若
      // 撤回未及生效)由迟到的 steer_committed 补气泡,去重已有处理。
      if (item.steered) {
        item.steered = false;
        item.steerId = null;
        item.cancelled = false;
      }
      const retryQueue = queueOf();
      if (retryQueue) retryQueue.splice(Math.min(index, retryQueue.length), 0, item);
      runSyncOnSession(sid, function () {
        addSystemItem("⚠️ " + bt("interruptQueuedFailed"));
      });
      notify();
      return false;
    }
  }

  async function cancelGeneration() {
    safeConsoleInfo("[pinvou3][chat-ui] cancel clicked", {
      sid: state.activeSessionId,
      busy: state.busy,
    });
    if (!state.busy) return;
    try {
      safeConsoleInfo("[pinvou3][chat-ui] cancel invoke start", { sid: state.activeSessionId });
      await invoke("cancel_generation", { sessionId: state.activeSessionId });
      safeConsoleInfo("[pinvou3][chat-ui] cancel invoke ok", { sid: state.activeSessionId });
    } catch (e) {
      console.warn("[pinvou3][chat-ui] cancel invoke failed", {
        sid: state.activeSessionId,
        error: e && e.toString ? e.toString() : e,
      });
      console.warn("cancel failed", e);
    }
  }


  // ── Persist messages ─────────────────────────────────────────────
  async function persistMessages() {
    if (!state.activeSessionId) return;
    if (isScheduledRunSession(state.activeSessionId)) return;
    try {
      await invoke("save_session_messages", { id: state.activeSessionId, messages: state.messages });
      // artifacts 一起落盘，重启/切换 session 后能恢复
      try { await invoke("save_session_artifacts", { id: state.activeSessionId, paths: state.artifacts.map(function (a) { return a.path; }) }); } catch { /* artifacts persist is best-effort */ }
      // Auto-title
      const meta = state.sessions.find(function (s) { return s.id === state.activeSessionId; });
      if (meta && (isDefaultChatTitle(meta.title) || personaPlaceholderTitles[state.activeSessionId])) {
        const firstUser = state.messages.find(function (m) { return m.role === "user"; });
        // 自动标题复用展示层过滤：内部信封/子智能体交接不参与命名，避免 XML 痕迹进
        // sidebar。hideInternalEnvelope=true 剥离 turn_meta/system-reminder 元数据块，
        // 否则普通消息的标题会拼入尾随 turn_meta（引擎持久化为独立 text block）。
        const titleText = firstUser ? userMessageDisplayText(firstUser.content || [], true) : "";
        if (titleText) {
          const newTitle = titleText.slice(0, 20);
          await invoke("rename_session", { id: state.activeSessionId, title: newTitle });
          meta.title = newTitle;
          delete personaPlaceholderTitles[state.activeSessionId]; // 已被对话内容命名,卸下占位标记
        }
      }
    } catch (e) {
      console.warn("persist failed", e);
    }
  }


    return {
      addChatItem,
      toolCallAlreadyStarted,
      toolCallAlreadyFinished,
      hasChatItemForTool,
      isDuplicateArtifactCard,
      addSystemItem,
      addAuthoritySyncNotice,
      compactPruneRollupText,
      removeCompactionStartItem,
      addOrMergePruneCompaction,
      timeStr,
      flushPendingTextBlock,
      flushAssistantMessageToHistory,
      resetPendingAssistant,
      isBusyFor,
      emitPetEvent,
      doSendFor,
      flushQueued,
      sendMessageToSession,
      sendMessage,
      getComposerDraft,
      setComposerDraft,
      retryFirstTurn,
      prefillComposer,
      removeQueued,
      summonPinvou,
      inspectPinvou,
      recordPinvouReview,
      resolvePinvouReview,
      dismissPinvouReview,
      persistPinvouReviews,
      cancelGeneration,
      interruptAndSend,
      interruptAndSendQueued,
      persistMessages,
      steer,
      settleSteerCommitted,
      settleSteerDropped,
      purgeSteerState,
    };
  };
})();
