(function () {
  "use strict";

  var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.chat = function (context) {
    var state = context.state;
    var invoke = context.invoke;
    var notify = context.notify;
    var TAURI = context.TAURI;
    var sessionStates = context.sessionStates;
    var turnUsageDirty = context.turnUsageDirty;
    var personaPlaceholderTitles = context.personaPlaceholderTitles;
    var renderMarkdown = context.renderMarkdown;
    var safeConsoleInfo = context.safeConsoleInfo;
    var bt = context.bt;
    var isDefaultChatTitle = context.isDefaultChatTitle;
    var runSyncOnSession = context.runSyncOnSession;
    var startThinking = context.startThinking;
    var stopThinking = context.stopThinking;
    var ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    var ensureSession = context.ensureSession;
    var getBuffer = context.getBuffer;
    var recordPinvouSceneForMessage = context.recordPinvouSceneForMessage || function () {};
    var reconcileRemoteTurn = context.reconcileRemoteTurn;
    var markRemoteTurn = context.markRemoteTurn;
    var clearAttachments = context.clearAttachments;
    var discardManagedAttachment = context.discardManagedAttachment || function () { return Promise.resolve(); };
    var isScheduledRunSession = context.isScheduledRunSession;
    var basename = context.basename;
    var userMessageDisplayText = context.userMessageDisplayText;
    var extractArtifactPaths = context.extractArtifactPaths;
    var parseScheduledTaskDraftFromText = context.parseScheduledTaskDraftFromText;
    var autoCreateScheduledTaskDraft = context.autoCreateScheduledTaskDraft;

  // Composer 草稿是纯前端短期状态：写入时不 notify，避免每次按键都克隆
  // 整个 chat slice 并触发 App 重渲染。会话切换本身会 notify，ChatView 会在
  // activeSessionId 变化后主动读取目标 working set 的草稿。
  function getComposerDraft() {
    return String(state.composerDraft || "");
  }
  function setComposerDraft(value) {
    var text = value == null ? "" : String(value);
    state.composerDraft = text;
    var activeBuffer = state.activeSessionId && sessionStates[state.activeSessionId];
    if (activeBuffer) activeBuffer.composerDraft = text;
    return text;
  }

  // 打断（interrupt）在途标记（按 session）：打断期间禁止 flushQueued 抢先发
  // 排队消息——chat:done handler 在打断消息 doSendFor 之前触发 flushQueued，
  // 若不挡，排队消息会先 reserve 成功、打断消息反而撞 session_turn_in_progress。
  // 打断消息发出（或其失败路径收尾）后清除，排队消息由打断轮的 chat:done 继续。
  var interruptInFlight = {};

  // ── Chat Items (display format for React) ────────────────────────
  function addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
  }
  function messageHasToolBlock(type, toolCallId) {
    if (!toolCallId) return false;
    for (var i = state.messages.length - 1; i >= 0; i--) {
      var blocks = state.messages[i] && state.messages[i].content;
      if (!Array.isArray(blocks)) continue;
      for (var j = blocks.length - 1; j >= 0; j--) {
        var block = blocks[j];
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
    var bn = basename(pathv);
    if (!bn) return false;
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      var it = state.chatItems[i];
      if (it.type === "tool" && context.fileMutationAction(it.name, it.args)) {
        var changedPaths = extractArtifactPaths(it.args);
        if (changedPaths.some(function (ap) { return basename(ap) === bn; })) return false;
      }
      if (it.type === "user") return false;
      if (it.type === "artifact_card" && basename(it.path) === bn) return true;
    }
    return false;
  }
  function addSystemItem(text, meta) {
    var item = { type: "system", text: text, time: timeStr() };
    if (meta) {
      for (var k in meta) item[k] = meta[k];
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
    for (var i = state.chatItems.length - 1; i >= 0; i--) {
      var it = state.chatItems[i];
      if (it.type === "system" && it.compactId === compactId && it.compactPhase === "start") {
        state.chatItems.splice(i, 1);
        return;
      }
    }
  }
  function addOrMergePruneCompaction(compactId) {
    removeCompactionStartItem(compactId);
    var last = state.chatItems[state.chatItems.length - 1];
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
      var assistantText = context.pendingAssistantBlocks
        .filter(function (block) { return block && block.type === "text" && block.text; })
        .map(function (block) { return block.text; })
        .join("\n\n");
      if (state.activeSessionId && state.activeSessionId === state.scheduledTaskCreationSessionId) {
        var scheduledTaskDraft = parseScheduledTaskDraftFromText(assistantText);
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
    var names = (attachments || []).map(function (attachment) {
      return typeof attachment === "string" ? attachment : attachment && attachment.basename;
    }).filter(Boolean).map(String);
    if (!names.length) return String(text || "");
    var attachmentLine = "📎 " + JSON.stringify(names);
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
    } catch (_) { /* 桌宠是纯装饰,广播失败不影响对话 */ }
  }
  function trackSceneBehavior(sid, scene) {
    var raw = String(scene || "");
    if (!sid || !raw) return;
    var parts = raw.split(":");
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
      sid: sid,
      textLen: (text || "").length,
      attachments: attachmentsPayload ? attachmentsPayload.length : 0,
    });
    turnUsageDirty[sid] = false; // 新一轮开始，重置口径保护
    var turnOwnerBuffer = getBuffer(sid);
    var submittedMessage = null;
    var submittedMessagePos = -1;
    var submittedUserItemId = 0;
    var submittedStreamId = 0;
    if (turnOwnerBuffer && turnOwnerBuffer.remoteTurnActive) {
      return Promise.reject(new Error(bt("sessionSyncingTurn")));
    }
    if (turnOwnerBuffer) {
      turnOwnerBuffer.localTurnOwned = true;
      turnOwnerBuffer.remoteTurnActive = false;
      turnOwnerBuffer.remoteTerminalSeen = false;
      turnOwnerBuffer.remoteCommittedRevision = "";
    }
    runSyncOnSession(sid, function () {
      state.chatItems = state.chatItems.filter(function (item) {
        return !item.turnErrorNotice && !item.authoritySyncNotice;
      });
      var uitem = {
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
    publishRemoteUserMessage(sid, displayText, meta && meta.remoteClientMessageId);
    return invoke("chat", { message: text, attachments: attachmentsPayload, sessionId: sid, restrictTools: !!restrictTools })
      .then(function () {
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
          sid: sid,
          error: err && err.toString ? err.toString() : err,
        });
        emitPetEvent("pet:turn_end", sid);
        var errorText = String(err && err.message ? err.message : err || "");
        var concurrentTurn = errorText.indexOf("session_turn_in_progress") >= 0;
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
        if (concurrentTurn && turnOwnerBuffer) markRemoteTurn(sid, turnOwnerBuffer);
        runSyncOnSession(sid, function () {
          // 稳定错误码(如 image_input_unsupported)按码替换为三语指引,而非剥前缀
          // 透传后端硬编码中文——英/日界面不该看到中文结论;文案与 ChatView
          // 前置警告(t.uiAttachments.*)同源。与 web bridge displayTurnError
          // 同一口径(chat.rs IMAGE_INPUT_*_ERROR)。
          var errorText = String(err && err.toString ? err.toString() : err || "");
          if (errorText.indexOf("image_input_unsupported") === 0) {
            errorText = errorText.indexOf("能力未知") >= 0
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
  function publishRemoteUserMessage(sid, content, clientMessageId) {
    if (!sid || !content) return;
    invoke("remote_control_publish_user_message", {
      sessionId: sid,
      content: content,
      clientMessageId: clientMessageId || null,
    }).catch(function () { /* 没开远控时静默跳过 */ });
  }
  // 本轮跑完(或被停止)后,若该 session 不忙且有排队消息 → 严格按 FIFO
  // 只发送队首一条。剩余消息留给后续 turn 的 done 继续逐条触发，避免把用户
  // 连续输入的多个独立任务合并成一个模型请求。
  function flushQueued(sid) {
    // 打断在途：排队消息让路，打断消息优先（否则 flush 先 reserve，
    // 打断消息反而撞 turn_in_progress 丢失）。
    if (interruptInFlight[sid]) return;
    var pendingBuffer = sessionStates[sid];
    if (pendingBuffer && pendingBuffer.remoteTurnActive) {
      reconcileRemoteTurn(sid).then(function (ready) {
        if (ready) flushQueued(sid);
      }).catch(function () {});
      return;
    }
    if (isBusyFor(sid)) return;            // doFinal 等又起了新 turn → 留给那轮的 done 再 flush
    var q = sid === state.activeSessionId ? state.queued : (sessionStates[sid] && sessionStates[sid].queued);
    if (!q || q.length === 0) return;
    // P0-A：队首是已投递引擎的 steer chip（等 chat:steer_committed 转气泡）
    // → 让路不发送。它已在引擎侧排队，重复 doSendFor 会变两条消息；committed
    // 事件移除 chip 后，后续 turn 的 flushQueued 继续发剩余排队消息。
    if (q[0].steered) return;
    var item = q.shift();
    var attachments = item.attachments || [];
    var displayText = item.displayText == null
      ? formatAttachmentDisplayText(item.text, attachments)
      : item.displayText;
    notify();
    doSendFor(sid, item.text, displayText, attachments, item.meta || null, !!item.restrictTools, true)
      .catch(function () {
        var retryQueue = sid === state.activeSessionId
          ? state.queued
          : (sessionStates[sid] && sessionStates[sid].queued);
        if (!retryQueue) return;
        retryQueue.unshift(item);
        notify();
      });
  }

  async function sendMessageToSession(sessionId, text, meta) {
    var sid = String(sessionId || "").trim();
    var content = String(text || "").trim();
    if (!sid) throw new Error(bt("targetSessionMissing"));
    if (!content) throw new Error(bt("replyContentEmpty"));
    var exists = state.sessions.some(function (session) { return String(session.id) === sid; });
    if (!exists) throw new Error(bt("targetSessionMissing"));

    await ensureSessionBufferLoaded(sid);
    var targetBuffer = getBuffer(sid);
    var targetQueue = targetBuffer && targetBuffer.queued;
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
    var completion = doSendFor(sid, content, content, [], meta || null, false, true)
      .then(
        function () { return { ok: true }; },
        function (error) { return { ok: false, error: error }; }
      );
    return { accepted: true, queued: false, completion: completion };
  }

  async function sendMessage(text, meta) {
    text = (text || "").trim();
    var readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return;
    // 还有解析中的附件 → 等
    if (state.attachments.some(function (a) { return a.status === "parsing"; })) {
      addSystemItem(bt("attachStillParsing"));
      return;
    }

    if (!state.activeSessionId) {
      await ensureSession(); // 草稿态首条消息 → 物化 session(命名靠下方 persistSession auto-title)
      if (!state.activeSessionId) {
        // 物化中止（如草稿态多智能体开关落盘失败）：把输入放回输入框，
        // 不静默丢字；错误提示由 ensureSession 内如实给出（复核 P1）。
        prefillComposer(text);
        return;
      }
    }
    var sid = state.activeSessionId;
    var activeTurnBuffer = getBuffer(sid);
    // 展示文本：把附件 chip 名附在用户消息末尾
    var displayText = formatAttachmentDisplayText(text, readyAttachments);
    var attachmentsPayload = readyAttachments.map(function (a) { return a.result; });
    function consumeUiTurnState() {
      var consumed = {
        scheduledTaskPendingGuide: state.scheduledTaskPendingGuide,
        scheduledTaskCreationSessionId: state.scheduledTaskCreationSessionId,
        scheduledTaskDraft: state.scheduledTaskDraft,
        activeSkill: state.activeSkill,
      };
      var requestedPayloadText = meta && meta.pinvouPayloadText
        ? String(meta.pinvouPayloadText || "").trim()
        : "";
      var payloadText = requestedPayloadText || text;
      var restrictTools = false;
      if (state.scheduledTaskPendingGuide) {
        payloadText = state.scheduledTaskPendingGuide + "\n\n" + text;
        if (requestedPayloadText) payloadText = state.scheduledTaskPendingGuide + "\n\n" + requestedPayloadText;
        restrictTools = true;
        state.scheduledTaskPendingGuide = null;
        state.scheduledTaskCreationSessionId = sid;
        state.scheduledTaskDraft = null;
      }
      state.activeSkill = null;
      return { snapshot: consumed, payloadText: payloadText, restrictTools: restrictTools };
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
        displayText: displayText,
        attachments: attachmentsPayload,
        meta: meta || null,
        restrictTools: prepared.restrictTools,
      });
      state.attachments = state.attachments.filter(function (attachment) {
        return readyAttachments.indexOf(attachment) < 0;
      });
      notify();
    }

    // Mid-turn inject: 当前 session 正在生成 → 走底座 steer channel,
    // turn loop 在下次 step 边界自动追加到 session.messages,模型下次思考时看到。
    // 语义对齐 Claude Code 的"主 agent 空闲时插入"(claude-code-best query.ts:1841)。
    //
    // UX 流程:
    // 1. push 到 state.queued → 输入框上方显示排队 chip
    // 2. 后端 steer_chat 成功后,steer() 立即 emit chat:user_message,
    //    applyRemoteUserMessageEvent 检测到 state.queued 有匹配项 → 移除 chip + 渲染气泡
    // 3. 后端 engine turn_loop 在下次 step 边界真正把消息追加到 session.messages
    if (isBusyFor(sid)) {
      var steerPreparation = consumeUiTurnState();
      var steerText = steerPreparation.payloadText;
      // attachments 走 mid-turn inject 时简化处理(只发文本);
      // 附件走另一条线注入是后续工作。
      state.attachments = [];
      // 清空 composer draft(对齐 sendMessage 成功路径)
      state.composerDraft = "";
      // 排队 chip 立即显示。steered=true 标记已投递引擎：flushQueued 跳过它
      // （防重复发送），由 chat:steer_committed / chat:steer_dropped 事件
      // 决定转气泡或取消（P0-A 投递确认协议）。
      var queuedItem = {
        id: ++context.itemIdSeq,
        text: steerText,
        displayText: steerText,
        attachments: [],
        meta: null,
        restrictTools: false,
        queuedAt: Date.now(),
        steered: true,
      };
      state.queued.push(queuedItem);
      notify();
      steer(sid, steerText, queuedItem)
        .catch(function (err) {
          console.warn("[pinvou3][chat-ui] steer failed, falling back to chat", {
            sid: sid, error: err && err.toString ? err.toString() : err,
          });
          // 降级到 chat() 路径:移除 chip,按正常 chat 流程发新 turn
          state.queued = state.queued.filter(function (q) { return q.id !== queuedItem.id; });
          notify();
          return invoke("chat", { message: steerText, attachments: [], sessionId: sid, restrictTools: false })
            .catch(function () {});
        });
      return;
    }
    // 兼容旧行为:state.queued 非空时仍走 flushQueued(跨 session 远控等边缘场景)
    if (state.queued.length > 0) {
      var queuedPreparation = consumeUiTurnState();
      queuePrepared(queuedPreparation);
      flushQueued(sid);
      return;
    }
    if (activeTurnBuffer && activeTurnBuffer.remoteTurnActive &&
        !(await reconcileRemoteTurn(sid))) {
      if (state.activeSessionId !== sid) return;
      addAuthoritySyncNotice(bt("remoteTurnSyncing"));
      return;
    }
    if (state.activeSessionId !== sid) return;
    if (isBusyFor(sid) || state.queued.length > 0) {
      var racedQueuePreparation = consumeUiTurnState();
      queuePrepared(racedQueuePreparation);
      if (!isBusyFor(sid)) flushQueued(sid);
      return;
    }

    var preparation = consumeUiTurnState();
    var accepted = await doSendFor(
      sid,
      preparation.payloadText,
      displayText,
      attachmentsPayload,
      meta,
      preparation.restrictTools,
    );
    if (!accepted) {
      restoreUiTurnState(preparation.snapshot);
      notify();
    } else {
      state.attachments = state.attachments.filter(function (attachment) {
        return readyAttachments.indexOf(attachment) < 0;
      });
      notify();
    }
  }
  // WebUI 草稿首条消息失败时的专用重试入口。桌面端没有远程草稿，
  // 保留同名空实现以维持跨宿主 Bridge API 的稳定形状。
  function retryFirstTurn() {}
  function prefillComposer(text) {
    state.composerPrefill = { id: (state.composerPrefill.id || 0) + 1, text: String(text || "") };
    notify();
  }
  // 撤销一条待发消息(点 chip 的 ✕)。
  function removeQueued(id) {
    var removed = state.queued.find(function (q) { return q.id === id; });
    if (removed && removed.attachments) {
      removed.attachments.forEach(discardManagedAttachment);
    }
    state.queued = state.queued.filter(function (q) { return q.id !== id; });
    notify();
  }

  // ── Pinvou v4 召唤式检阅:Boss 主动呼叫,审当前 session 前面的工作 ──
  // 设计 docs/品悟v4-常驻检阅助手设计.md。纯召唤、不替 Boss 决策。
  // 审查卡进 chatItems(当前会话可见);跨会话持久化(进 messages/独立存储)是 §6 后续增强。
  async function summonPinvou(focus, mode) {
    if (!state.activeSessionId) { addSystemItem(bt("summonNeedsSession")); return; }
    if (state.pinvouSummoning) return;
    state.pinvouSummoning = true;
    var sid = state.activeSessionId; // 召唤发起时的 session;await 返回后校验,防跨 session 串(召唤慢+切走)
    // 检阅结果弹 modal(不进对话流):一次只一个,裁决/跳过直接操作 state.pinvouModal.review、
    // 不靠 pos 定位(根治连续召唤 pos 重复串卡)。
    state.pinvouModal = { loading: true, coverage: mode === "coverage" };
    notify();
    try {
      // focus=产出物 path(品=审产物); mode="coverage"=悟(通盘体检)。
      var review = await invoke("summon_pinvou", { sessionId: sid, focus: focus || null, mode: mode || null });
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
    var pos = state.messages.length;
    state.pinvouReviews.push({ pos: pos, review: review });
    var sid = state.activeSessionId;
    var snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    invoke("save_session_pinvou_reviews", { sessionId: sid, reviews: snapshot }).catch(function () {});
    return pos; // 供卡片记 reviewPos,裁决时按 pos 定位原 state 写 resolution
  }

  // §2 按勾选裁决:resolution 已由前端写回 review 对象(引用→sidecar),这里持久化 +
  // 把勾「让AI改」的条目走 B1 发定向修订指令(只改对应段落、禁全文重写)。Boss 驾驶,非自动。
  async function resolvePinvouReview(resolutions, actions) {
    // 弹窗只一个 review(state.pinvouModal.review),直接在它上面写 resolution——不靠 pos 定位
    // (根治连续召唤 pos 重复串卡)。它和 sidecar entry.review 同引用,写它=写 sidecar。
    var isWu = !!(state.pinvouModal && state.pinvouModal.coverage); // 关窗前取,供转交标品/悟
    var review = state.pinvouModal && state.pinvouModal.review;
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
    var fix = actions.filter(function (a) { return a.t === "fix"; });
    var verify = actions.filter(function (a) { return a.t === "verify"; });
    var adopt = actions.filter(function (a) { return a.t === "adopt"; });
    var ask = actions.filter(function (a) { return a.t === "ask"; });
    var parts = [];
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
    var fill = actions.filter(function (a) { return a.t === "fill"; });
    if (fill.length) {
      if (parts.length) parts.push("");
      parts.push("以下维度产物还缺，请补充进去（保留其余、只增不改）：");
      fill.forEach(function (a) { parts.push("- " + a.dimension + (a.suggestion ? "：" + a.suggestion : "")); });
      parts.push("（涉及外部事实的，先查证再写、标依据，别凭记忆编。）");
    }
    if (parts.length) sendMessage(parts.join("\n"), { pinvouTransfer: isWu ? "悟" : "品" });
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
    var snapshot = JSON.parse(JSON.stringify(state.pinvouReviews));
    return invoke("save_session_pinvou_reviews", { sessionId: state.activeSessionId, reviews: snapshot }).catch(function () {});
  }

  // Mid-turn inject: 投到当前 turn 的下个 step 边界。底座 turn loop 在
  // tool result 处理完后自动追加到 session.messages,模型下次思考时看到。
  // engine 不在场(没起 / 已 evict)时后端静默 Ok,前端无需处理。
  //
  // **chip 滞留 → 自然嵌入**:
  // invoke steer_chat 成功后仅记录"引擎已接受",chip 保留在 state.queued,
  // 不立即渲染 bubble。chat-events.js 在 chat:transcript_committed 收到时
  // 通过 load_session 重载 session.messages 检测新增 user message,把对应
  // chip 转 bubble。**不人为设置延时** —— 等当前 AI 步自然结束即可。
  async function steer(sid, content, queuedItem) {
    safeConsoleInfo("[pinvou3][chat-ui] steer start", { sid: sid, len: (content || "").length });
    await invoke("steer_chat", { sessionId: sid, content: String(content || "") });
    safeConsoleInfo("[pinvou3][chat-ui] steer accepted, awaiting transcript commit", { sid: sid });
  }

  // Mid-turn INTERRUPT: 打断当前 AI 步骤,立刻起新 turn 发送。
  // 与 steer 区别:steer 等下次 step 边界自然嵌入(不打断 tool 调用),
  // interrupt 立刻 cancel 当前 turn,起新 turn,消息进 chat 命令路径。
  //
  // 事件驱动同步:不轮询 state.busy,而是 await chat:done 事件本身。
  // state.busy 在 chat:done handler 内同步置 false,事件触发即代表
  // turn lifecycle 已完成 cancel + cleanup,可以安全 reserve 新 turn。
  // 长 tool chain 场景下 5s 兜底超时(避免 cancel 永久挂起)。
  //
  // generation 匹配(P0-B):chat:done payload 带后端轮次身份(generation),
  // 只对目标轮 resolve —— 迟到的旧轮终态、其他轮的终态都不会提前解锁等待。
  // 旧后端(无 generation 字段)时退化为按 sid 匹配的旧行为。
  function waitForChatDone(sid, generation, timeoutMs) {
    return new Promise(function (resolve) {
      var timer = null;
      var resolved = false;
      var unlisten = null;
      function done() {
        if (resolved) return;
        resolved = true;
        if (timer) { clearTimeout(timer); timer = null; }
        if (unlisten && typeof unlisten === "function") {
          try { unlisten(); } catch (_) {}
          unlisten = null;
        }
        resolve();
      }
      // 通过 TAURI.event.listen 直接订阅 webview 事件,匹配 sid 后 resolve。
      // Tauri 2 的 listen 返回 Promise<UnlistenFn>,on 收到事件即回调。
      if (TAURI && TAURI.event && typeof TAURI.event.listen === "function") {
        var p = TAURI.event.listen("chat:done", function (e) {
          if (!e || !e.payload || e.payload.session_id !== sid) return;
          var payloadGeneration = e.payload.generation;
          if (generation != null && payloadGeneration != null &&
              Number(payloadGeneration) !== Number(generation)) return;
          done();
        });
        if (p && typeof p.then === "function") {
          p.then(function (un) { unlisten = un; }).catch(function () {});
        }
      } else {
        // 兜底:轮询 busy(for 测试环境或 web 模式)
        var deadline = Date.now() + timeoutMs;
        var poll = function () {
          if (resolved) return;
          if (!isBusyFor(sid)) { done(); return; }
          if (Date.now() >= deadline) { done(); return; }
          setTimeout(poll, 50);
        };
        poll();
      }
      timer = setTimeout(done, timeoutMs);
    });
  }

  async function interruptAndSend(sid, text, displayText, attachments, meta, restrictTools) {
    safeConsoleInfo("[pinvou3][chat-ui] interrupt-and-send start", { sid: sid });
    interruptInFlight[sid] = true;
    try {
      // 1) cancel 当前 turn。cancel_generation 返回 CancelOutcome { generation, terminal }：
      //    terminal=true（claim 路径终态已由 cancel 自身确认 / 目标轮已结束 / 空闲）
      //    → 无需等待事件；false → 等待携带目标 generation 的 chat:done（事件驱动）。
      //    这消除了两处确定性竞态：claim 路径的 chat:done 发在 cancel 返回之前
      //    （监听器必然错过）、turn 刚自然结束时 cancel no-op 不再有事件——二者
      //    前端都无法靠等事件收敛，只能由命令返回值确认终态。
      if (state.busy) {
        var outcome = null;
        try {
          outcome = await invoke("cancel_generation", { sessionId: sid });
        } catch (e) {
          console.warn("[pinvou3][chat-ui] cancel failed before interrupt", e);
        }
        var terminal = !!(outcome && outcome.terminal);
        var generation = outcome && outcome.generation;
        if (!terminal) {
          // 事件驱动等待（P0-B）：后端保证 chat:done 到达时 reserve 闸门已重开，
          // 不再需要固定 sleep 补窗口。超时仅作最后兜底，走下方失败恢复路径。
          await waitForChatDone(sid, generation, 5000);
        }
      }
      // 2) 不整体清空 queue：打断只放弃当前轮进度，保留用户排队中的其他消息
      //    （P0-A 后引擎侧丢弃残留 steer 会发 SteerDropped 事件，前端据此提示）。
      // 3) 附件对齐 sendMessage：取当前 ready 附件随打断消息发送。
      var readyAttachments = (state.attachments || []).filter(function (a) {
        return a.status === "ready" && a.result;
      });
      var attachmentPayload = readyAttachments.map(function (a) { return a.result; });
      // 4) 真正发新消息；失败时由调用方（handleInterruptSend）恢复输入框。
      return await doSendFor(sid, text, displayText, attachmentPayload, meta, restrictTools, true);
    } finally {
      interruptInFlight[sid] = false;
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
      try { await invoke("save_session_artifacts", { id: state.activeSessionId, paths: state.artifacts.map(function (a) { return a.path; }) }); } catch (_) {}
      // Auto-title
      var meta = state.sessions.find(function (s) { return s.id === state.activeSessionId; });
      if (meta && (isDefaultChatTitle(meta.title) || personaPlaceholderTitles[state.activeSessionId])) {
        var firstUser = state.messages.find(function (m) { return m.role === "user"; });
        // 自动标题复用展示层过滤：内部信封/子智能体交接不参与命名，避免 XML 痕迹进
        // sidebar。hideInternalEnvelope=true 剥离 turn_meta/system-reminder 元数据块，
        // 否则普通消息的标题会拼入尾随 turn_meta（引擎持久化为独立 text block）。
        var titleText = firstUser ? userMessageDisplayText(firstUser.content || [], true) : "";
        if (titleText) {
          var newTitle = titleText.slice(0, 20);
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
      addChatItem: addChatItem,
      toolCallAlreadyStarted: toolCallAlreadyStarted,
      toolCallAlreadyFinished: toolCallAlreadyFinished,
      hasChatItemForTool: hasChatItemForTool,
      isDuplicateArtifactCard: isDuplicateArtifactCard,
      addSystemItem: addSystemItem,
      addAuthoritySyncNotice: addAuthoritySyncNotice,
      compactPruneRollupText: compactPruneRollupText,
      removeCompactionStartItem: removeCompactionStartItem,
      addOrMergePruneCompaction: addOrMergePruneCompaction,
      timeStr: timeStr,
      flushPendingTextBlock: flushPendingTextBlock,
      flushAssistantMessageToHistory: flushAssistantMessageToHistory,
      resetPendingAssistant: resetPendingAssistant,
      isBusyFor: isBusyFor,
      emitPetEvent: emitPetEvent,
      doSendFor: doSendFor,
      publishRemoteUserMessage: publishRemoteUserMessage,
      flushQueued: flushQueued,
      sendMessageToSession: sendMessageToSession,
      sendMessage: sendMessage,
      getComposerDraft: getComposerDraft,
      setComposerDraft: setComposerDraft,
      retryFirstTurn: retryFirstTurn,
      prefillComposer: prefillComposer,
      removeQueued: removeQueued,
      summonPinvou: summonPinvou,
      inspectPinvou: inspectPinvou,
      recordPinvouReview: recordPinvouReview,
      resolvePinvouReview: resolvePinvouReview,
      dismissPinvouReview: dismissPinvouReview,
      persistPinvouReviews: persistPinvouReviews,
      cancelGeneration: cancelGeneration,
      persistMessages: persistMessages,
      steer: steer,
    };
  };
})();
