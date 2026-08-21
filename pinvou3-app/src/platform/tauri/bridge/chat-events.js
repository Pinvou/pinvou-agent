(function () {
  "use strict";

  var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["chat-events"] = function (context) {
    var state = context.state;
    var recordAuthoritySyncDiagnostic = context.recordAuthoritySyncDiagnostic || function () {};
    var authoritySyncBufferSnapshot = context.authoritySyncBufferSnapshot || function () { return {}; };
    var listen = context.listen;
    var notify = context.notify;
    var invoke = context.invoke;
    var turnUsageDirty = context.turnUsageDirty;
    var sessionStates = context.sessionStates;
    var renderMarkdown = context.renderMarkdown;
    var bt = context.bt;
    var onSessionEvent = context.onSessionEvent;
    var runSyncOnSession = context.runSyncOnSession;
    // 权威 modeState 写回收敛点（bridge.js 共享，评审 P1）：事件负载携带的
    // modeState 更新也必须 bump epoch，否则在途 syncModeState 旧读会覆盖
    // 事件写回的权威值。
    var applyAuthoritativeModeState = context.applyAuthoritativeModeState;
    var addChatItem = context.addChatItem;
    var toolCallAlreadyStarted = context.toolCallAlreadyStarted;
    var toolCallAlreadyFinished = context.toolCallAlreadyFinished;
    var hasChatItemForTool = context.hasChatItemForTool;
    var addSystemItem = context.addSystemItem;
    var addAuthoritySyncNotice = context.addAuthoritySyncNotice;
    var timeStr = context.timeStr;
    var flushPendingTextBlock = context.flushPendingTextBlock;
    var flushAssistantMessageToHistory = context.flushAssistantMessageToHistory;
    var resetPendingAssistant = context.resetPendingAssistant;
    var flushQueued = context.flushQueued;
    var isBusyFor = context.isBusyFor;
    var doSendFor = context.doSendFor;
    var ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    var getBuffer = context.getBuffer;
    var markRemoteTurn = context.markRemoteTurn;
    var reconcileRemoteTurn = context.reconcileRemoteTurn;
    var saveWorkingSetTo = context.saveWorkingSetTo;
    var hydratedMessageKey = context.hydratedMessageKey;
    var thinkingTool = context.thinkingTool;
    var thinkingIdle = context.thinkingIdle;
    var startThinking = context.startThinking;
    var stopThinking = context.stopThinking;
    var userMessageDisplayText = context.userMessageDisplayText;
    var scheduleScheduledRunRefresh = context.scheduleScheduledRunRefresh;
    var handleMemoryWrite = context.handleMemoryWrite;
    var isPresentArtifactTool = context.isPresentArtifactTool;
    var artifactPathFromToolOutput = context.artifactPathFromToolOutput;
    var shouldUseToolOutputAsArtifact = context.shouldUseToolOutputAsArtifact;
    var presentArtifactAbsPath = context.presentArtifactAbsPath;
    var extractArtifactPaths = context.extractArtifactPaths;
    var fileMutationAction = context.fileMutationAction;

    function refreshEffectiveModelConfigAfterAuthError(error) {
      if (!error || !/\b401\b|unauthorized|authentication/i.test(String(error))) return;
      var requestedSessionId = state.activeSessionId || null;
      invoke("get_effective_model_config", { sessionId: requestedSessionId })
        .then(function (config) {
          if (requestedSessionId !== (state.activeSessionId || null)) return;
          state.effectiveModelConfig = config;
          notify();
        })
        .catch(function () {});
    }

    function visibleUserTurnIndex() {
      var count = state.chatItems.filter(function (item) { return item && item.type === "user"; }).length;
      return Math.max(0, count - 1);
    }

    function latestOpenTimelineStart() {
      var events = state.turnTimeline || [];
      var completed = Object.create(null);
      events.forEach(function (event) {
        if (event && event.event === "assistant_done") completed[event.turn_id] = true;
      });
      for (var index = events.length - 1; index >= 0; index--) {
        var event = events[index];
        if (event && event.event === "user_start" && !completed[event.turn_id]) return event;
      }
      return null;
    }

    function recordTurnStarted() {
      if (state.activeTurnTimelineId) return;
      var timestamp = Date.now();
      var turnIndex = visibleUserTurnIndex();
      var existing = latestOpenTimelineStart();
      if (existing && Math.abs(timestamp - Number(existing.timestamp || 0)) < 60000) {
        existing.ui_turn_index = turnIndex;
        state.activeTurnTimelineId = existing.turn_id;
        return;
      }
      var turnId = "ui_" + String(state.activeSessionId || "session") + "_" + timestamp + "_" + turnIndex;
      state.activeTurnTimelineId = turnId;
      state.turnTimeline = (state.turnTimeline || []).concat([{
        turn_id: turnId,
        event: "user_start",
        timestamp: timestamp,
        ts: new Date(timestamp).toISOString(),
        ui_turn_index: turnIndex,
      }]);
    }

    function recordTurnCompleted(payload) {
      var openStart = latestOpenTimelineStart();
      var turnId = state.activeTurnTimelineId || (openStart && openStart.turn_id);
      if (!turnId) return;
      var timestamp = Date.now();
      var start = openStart || (state.turnTimeline || []).find(function (event) {
        return event && event.turn_id === turnId && event.event === "user_start";
      });
      state.turnTimeline = (state.turnTimeline || []).concat([{
        turn_id: turnId,
        event: "assistant_done",
        timestamp: timestamp,
        ts: new Date(timestamp).toISOString(),
        status: payload && payload.status || (payload && payload.error ? "Failed" : "Completed"),
        error: payload && payload.error || null,
        ui_turn_index: start && start.ui_turn_index,
      }]);
      state.activeTurnTimelineId = null;
    }

    function latestTimelineCompletion(events) {
      var latest = 0;
      (Array.isArray(events) ? events : []).forEach(function (event) {
        if (!event || event.event !== "assistant_done") return;
        var timestamp = Number(event.timestamp || 0);
        if (Number.isFinite(timestamp)) latest = Math.max(latest, timestamp);
      });
      return latest;
    }

    function authoritativeTimelineMissesKnownCompletion(local, authoritative) {
      var authoritativeStarts = Object.create(null);
      var authoritativeCompletions = Object.create(null);
      (Array.isArray(authoritative) ? authoritative : []).forEach(function (event) {
        if (!event || !event.turn_id) return;
        if (event.event === "user_start") authoritativeStarts[event.turn_id] = true;
        if (event.event === "assistant_done") authoritativeCompletions[event.turn_id] = true;
      });
      return (Array.isArray(local) ? local : []).some(function (event) {
        return event && event.event === "assistant_done" && event.turn_id &&
          authoritativeStarts[event.turn_id] && !authoritativeCompletions[event.turn_id];
      });
    }

    // chat:done 可能来自后台、页面恢复或切换后的会话，本地 buffer 不一定见过
    // 对应的 turn_started。后端保证先落 timing_events 再发终态；这里重新读取
    // 权威时间线，避免明明已完成却漏掉状态徽标与耗时。读取失败时保留本地投影。
    function refreshAuthoritativeTurnTimeline(sessionId) {
      if (!sessionId) return Promise.resolve(false);
      return invoke("get_session_timeline", { sessionId: sessionId })
        .then(function (authoritative) {
          if (!Array.isArray(authoritative) || authoritative.length === 0) return false;
          var changed = false;
          runSyncOnSession(sessionId, function () {
            // 不允许一次短暂的旧磁盘快照把刚收到的本地终态倒退回“执行中”。
            // 正常后端已保证先落盘再发事件；这层同时保护旧版本和异常 I/O 时序。
            if (authoritativeTimelineMissesKnownCompletion(state.turnTimeline, authoritative)) return;
            var localLatest = latestTimelineCompletion(state.turnTimeline);
            var authoritativeLatest = latestTimelineCompletion(authoritative);
            // timing sidecar 是 best-effort；若权威快照明显旧于刚收到的本地终态，
            // 不用旧数据覆盖当前可见状态。
            if (localLatest && authoritativeLatest + 5000 < localLatest) return;
            state.turnTimeline = authoritative;
            changed = true;
          });
          if (changed) notify();
          return changed;
        })
        .catch(function (error) {
          safeConsoleInfo("[pinvou3][chat-ui] timeline refresh skipped", {
            sid: sessionId,
            error: String(error || ""),
          });
          return false;
        });
    }

    function preserveInterruptedAssistantPresentation() {
      var userItemIndex = -1;
      var afterMessageIndex = -1;
      var afterUserOrdinal = -1;
      for (var index = 0; index < state.chatItems.length; index++) {
        var candidate = state.chatItems[index];
        if (!candidate || candidate.type !== "user") continue;
        afterUserOrdinal += 1;
        userItemIndex = index;
        afterMessageIndex = -1;
        if (Number.isFinite(Number(candidate.messageIndex))) {
          afterMessageIndex = Number(candidate.messageIndex);
        }
      }
      // 没有任何 user 气泡时无法锚定轮次;若把全部历史 assistant 项都标记为
      // 仅展示,下次权威重载会在末尾追加整段历史的重复副本。此时放弃保留,
      // 退化为修复前行为(重载后消失),但必须清空 pending 以免污染下一轮。
      if (userItemIndex < 0) {
        context.pendingAssistantText = "";
        context.pendingAssistantBlocks = [];
        return;
      }
      for (var itemIndex = userItemIndex + 1; itemIndex < state.chatItems.length; itemIndex++) {
        var item = state.chatItems[itemIndex];
        if (!item || item.type !== "assistant" || !item.html) continue;
        item.interruptedDisplayOnly = true;
        item.afterMessageIndex = afterMessageIndex;
        item.afterUserOrdinal = afterUserOrdinal;
      }
      context.pendingAssistantText = "";
      context.pendingAssistantBlocks = [];
    }
    var markTurnDirtyArtifact = context.markTurnDirtyArtifact;
    var trackArtifact = context.trackArtifact;
    var untrackArtifact = context.untrackArtifact;
    var findPresentedArtifact = context.findPresentedArtifact;
    var isDeliverable = context.isDeliverable;
    var noteArtifactChange = context.noteArtifactChange;
    var publishRemoteLiveSnapshot = context.publishRemoteLiveSnapshot;
    var persistMessagesFor = context.persistMessagesFor;
    var composePlanMarkdown = context.composePlanMarkdown;
    var refreshHistoryList = context.refreshHistoryList;
    var isShellExecutionTool = context.isShellExecutionTool;
    var scheduleShellPoll = context.scheduleShellPoll;
    var appendToolItemOutput = context.appendToolItemOutput;
    var scheduleShellNotify = context.scheduleShellNotify;
    var markBackgroundToolItem = context.markBackgroundToolItem;
    var patchLastItem = context.patchLastItem;
    var isDuplicateArtifactCard = context.isDuplicateArtifactCard;
    var updateToolItem = context.updateToolItem;
    var basename = context.basename;
    var hasUnresolvedItem = context.hasUnresolvedItem;
    var finishBackgroundToolItem = context.finishBackgroundToolItem;
    var safeConsoleInfo = context.safeConsoleInfo;
    var isScheduledRunSession = context.isScheduledRunSession;
    var markScheduledInitialTurnTerminal = context.markScheduledInitialTurnTerminal;
    var isAbsPath = context.isAbsPath;
    var addOrMergePruneCompaction = context.addOrMergePruneCompaction;

  // ── Event listeners ──────────────────────────────────────────────
  // 所有 chat:* 事件都带 session_id(后端 spawn_event_forwarder 打的 tag)。
  // onSessionEvent 按 session_id 把同步逻辑路由到对应 session 的工作集:active 直接跑,
  // 后台临时切工作集跑完再切回。下面每个监听器的 body 与旧单 session 版逐字一致,
  // 只是包了一层路由,所以 active session 行为零变化。
  var isInternalRuntimeUserMessage = context.isInternalRuntimeUserMessage;

  function applyRemoteUserMessageEvent(e, force) {
    var payload = e && e.payload || {};
    var sid = payload.session_id || state.activeSessionId;
    if (!sid) return false;
    var userBuffer = getBuffer(sid);
    if (!userBuffer) return false;
    if (userBuffer.localTurnOwned && !force) {
      userBuffer.deferredRemoteUserEvent = e;
      return false;
    }
    var content = String(payload.content || "");
    var hideInternalRuntimeMessage = isInternalRuntimeUserMessage(content);
    var operation = String(payload.operation || "append");
    var action = String(payload.action || "");
    var actionPlanId = String(payload.plan_id || payload.planId || "").trim();
    var baseRevision = String(payload.base_transcript_revision || "");
    var admissionKey = baseRevision
      ? operation + ":" + baseRevision
      : (e && e.id ? "event:" + e.id : "");
    if (admissionKey && userBuffer.remoteAdmissionKeys.indexOf(admissionKey) >= 0) return false;
    if (admissionKey) {
      userBuffer.remoteAdmissionKeys.push(admissionKey);
      if (userBuffer.remoteAdmissionKeys.length > 32) userBuffer.remoteAdmissionKeys.shift();
    }
    var lastUserText = "";
    for (var messageIndex = userBuffer.messages.length - 1; messageIndex >= 0; messageIndex--) {
      var candidate = userBuffer.messages[messageIndex];
      if (candidate && candidate.role === "user") {
        lastUserText = userMessageDisplayText(candidate.content || [], false);
        break;
      }
    }
    var snapshotAlreadyCoversTurn = !!(
      userBuffer.loadedFromDisk && baseRevision && userBuffer.sessionRevision &&
      userBuffer.sessionRevision !== baseRevision && lastUserText === content
    );
    markRemoteTurn(sid, userBuffer, false, "remote_user_message_event");
    runSyncOnSession(sid, function () {
      if (action === "accept_plan") {
        state.chatItems.forEach(function (item) {
          if (item && item.type === "plan_card" && item.cardState === "active" && !item.resolved &&
              (!actionPlanId || String(item.planId || "") === actionPlanId)) {
            item.cardState = "approved";
            item.resolved = true;
            item.statusLabel = bt("approved");
          }
        });
        var acceptedMode = payload.mode_state || payload.modeState;
        // 事件按 sid 定向写回 + bump epoch（此回调在 runSyncOnSession(sid) 内，
        // sid 即触发会话；不能用 active 兜底，await 竞态下两者可能不同）。
        if (acceptedMode) applyAuthoritativeModeState(sid, acceptedMode);
      }
      state.chatItems = state.chatItems.filter(function (item) { return !item.turnErrorNotice; });
      if (!snapshotAlreadyCoversTurn && !hideInternalRuntimeMessage) {
        if (operation === "edit_last") {
          for (var index = state.chatItems.length - 1; index >= 0; index--) {
            if (state.chatItems[index] && state.chatItems[index].type === "user") {
              state.chatItems.splice(index);
              break;
            }
          }
          resetPendingAssistant();
        }
        addChatItem({ type: "user", text: content, time: timeStr() });
      }
      state.busy = true;
      if (!state.thinking.active) startThinking();
      context.currentStreamText = "";
      context.currentStreamId = 0;
    });
    notify();
    return true;
  }

  listen("chat:user_message", async function (e) {
    var payload = e && e.payload || {};
    var sid = payload.session_id || state.activeSessionId;
    if (sid && sid !== state.activeSessionId) {
      try { await ensureSessionBufferLoaded(sid); }
      catch (err) {
        console.warn("chat session hydrate failed", err);
        return;
      }
    }
    applyRemoteUserMessageEvent(e, false);
  });

  listen("chat:transcript_committed", function (e) {
    var payload = e && e.payload || {};
    var sid = payload.session_id || state.activeSessionId;
    if (!sid) return;
    var committedBuffer = getBuffer(sid);
    if (!committedBuffer) return;
    var revision = String(payload.transcript_revision || payload.transcriptRevision || "");
    if (revision) {
      committedBuffer.sessionRevision = revision;
      committedBuffer.remoteCommittedRevision = revision;
    }
    recordAuthoritySyncDiagnostic("transcript_committed_event_received", Object.assign({
      event_revision: revision,
      terminal_seen_before_event: !!committedBuffer.remoteTerminalSeen,
    }, authoritySyncBufferSnapshot(sid, committedBuffer)));
    if (committedBuffer.remoteTerminalSeen && !isBusyFor(sid)) {
      reconcileRemoteTurn(sid).then(function (ready) {
        if (ready) flushQueued(sid);
      }).catch(function () {});
    }
  });

  listen("chat:turn_started", function (e) { onSessionEvent(e, function () {
    state.busy = true;
    if (!state.thinking.active) startThinking();
    recordTurnStarted();
    notify();
  }); });

  function reasoningEventIndex(e) {
    var value = e && e.payload && e.payload.index;
    if (value === undefined || value === null || value === "") return null;
    var parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : String(value);
  }

  function streamingReasoningItem(index) {
    for (var itemIndex = state.chatItems.length - 1; itemIndex >= 0; itemIndex--) {
      var item = state.chatItems[itemIndex];
      if (!item || item.type !== "reasoning" || !item.streaming) continue;
      if (index === undefined || index === null || item.reasoningIndex === index) return item;
    }
    return null;
  }

  function finalizeStreamingReasoning(index) {
    var completedAt = Date.now();
    for (var itemIndex = state.chatItems.length - 1; itemIndex >= 0; itemIndex--) {
      var item = state.chatItems[itemIndex];
      if (!item || item.type !== "reasoning" || !item.streaming) continue;
      if (index !== undefined && index !== null && item.reasoningIndex !== index) continue;
      item.streaming = false;
      item.completedAt = completedAt;
    }
  }

  function finalizeAssistantStreamBeforeReasoning() {
    flushPendingTextBlock();
    var item = state.chatItems.find(function (it) { return it.id === context.currentStreamId; });
    if (item) {
      if (item.html) item.streaming = false;
      else state.chatItems = state.chatItems.filter(function (it) { return it !== item; });
    }
    context.currentStreamText = "";
    context.currentStreamId = 0;
  }

  function startReasoningBlock(index) {
    var existing = streamingReasoningItem(index);
    if (existing) return existing;
    finalizeStreamingReasoning();
    finalizeAssistantStreamBeforeReasoning();
    var item = {
      type: "reasoning",
      text: "",
      time: timeStr(),
      streaming: true,
      startedAt: Date.now(),
      completedAt: null,
      reasoningIndex: index,
    };
    addChatItem(item);
    // 用空 thinking block 记录明确的 Started 边界，使两个相邻的
    // thinking content block 不会在持久化时被误合并。
    context.pendingAssistantBlocks.push({ type: "thinking", thinking: "" });
    return item;
  }

  function appendReasoningBlock(text) {
    var blocks = context.pendingAssistantBlocks;
    var last = blocks[blocks.length - 1];
    if (last && last.type === "thinking") last.thinking += text;
    else blocks.push({ type: "thinking", thinking: text });
  }

  listen("chat:reasoning_start", function (e) { onSessionEvent(e, function () {
    startReasoningBlock(reasoningEventIndex(e));
    notify();
  }); });

  listen("chat:reasoning_delta", function (e) { onSessionEvent(e, function () {
    var text = String(e.payload && e.payload.text || "");
    if (!text) return;
    var index = reasoningEventIndex(e);
    var item = streamingReasoningItem(index);
    if (!item) {
      item = startReasoningBlock(index);
    }
    item.text += text;
    appendReasoningBlock(text);
    notify();
  }); });

  listen("chat:reasoning_done", function (e) { onSessionEvent(e, function () {
    var index = reasoningEventIndex(e);
    var item = streamingReasoningItem(index);
    finalizeStreamingReasoning(index);
    if (item && !item.text) {
      state.chatItems = state.chatItems.filter(function (candidate) { return candidate !== item; });
      var last = context.pendingAssistantBlocks[context.pendingAssistantBlocks.length - 1];
      if (last && last.type === "thinking" && !last.thinking) context.pendingAssistantBlocks.pop();
    }
    notify();
  }); });

  listen("chat:delta", function (e) { onSessionEvent(e, function () {
    finalizeStreamingReasoning();
    var text = e.payload && e.payload.text || "";
    context.pendingAssistantText += text;
    context.currentStreamText += text;
    // Update the streaming chat item
    var item = state.chatItems.find(function (it) { return it.id === context.currentStreamId; });
    if (item) {
      item.text = context.currentStreamText;
      item.html = renderMarkdown(context.currentStreamText);
      item.streaming = true;
    } else {
      // New bubble needed (after tool card)
      context.currentStreamId = ++context.itemIdSeq;
      state.chatItems.push({
        id: context.currentStreamId,
        type: "assistant",
        text: context.currentStreamText,
        html: renderMarkdown(context.currentStreamText),
        time: timeStr(),
        streaming: true,
      });
    }
    notify();
  }); });

  listen("scheduled_task:run_updated", function (e) {
    scheduleScheduledRunRefresh();
  });

  listen("chat:memory_write", function (e) {
    handleMemoryWrite(e && e.payload);
  });

  // present_artifact MCP 工具名匹配:兼容底座 MCP adapter 可能加的 server 前缀
  // (实测透传名若带前缀仍命中)。命中则渲染成品卡而非灰色工具卡。

  listen("chat:tool_start", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (toolCallAlreadyStarted(p.id) || toolCallAlreadyFinished(p.id)) return;
    if (p.session_id) turnUsageDirty[p.session_id] = true; // 多请求轮，usage 累加值不可当占用
    context.toolMeta[p.id] = { name: p.name, args: p.args };
    finalizeStreamingReasoning();
    thinkingTool(p.name);
    flushPendingTextBlock();
    context.pendingAssistantBlocks.push({ type: "tool_use", id: p.id, name: p.name, input: p.args || {} });

    // Finalize current streaming bubble
    var streamItem = state.chatItems.find(function (it) { return it.id === context.currentStreamId; });
    if (streamItem) {
      streamItem.streaming = false;
    }
    context.currentStreamText = "";
    context.currentStreamId = 0;

    // request_user_input：不渲染默认工具卡，等 chat:user_input_required 单独渲染选择卡片
    if (p.name === "request_user_input") { notify(); return; }

    // present_artifact：不渲染灰色工具卡，等 tool_end 成功时渲染成品卡
    if (isPresentArtifactTool(p.name)) { notify(); return; }

    // load_skill：模型加载技能 → 点亮 composer 技能标（内置自动技能"正在使用"指示）。
    if (p.name === "load_skill") {
      var skArg = ((p.args && (p.args.name || p.args.skill)) || "").toString();
      var skLower = skArg.toLowerCase();
      if (skArg.indexOf("视觉设计") >= 0 || skLower.indexOf("visual-design") >= 0) state.activeSkill = "visual-design";
      else if (skArg.indexOf("公文写作") >= 0 || skLower.indexOf("government-writing") >= 0) state.activeSkill = "government-writing";
      else if (skArg.indexOf("PPT") >= 0 || skArg.indexOf("幻灯片") >= 0 || skLower.indexOf("pptx") >= 0) state.activeSkill = "pptx";
      else if (skArg.indexOf("数据分析可视化") >= 0 || skArg.indexOf("数据可视化") >= 0 || skLower.indexOf("visualizer") >= 0) state.activeSkill = "visualizer";
      // 不 return：照常出工具卡。卡内容在 tool_end / rerender 处脱敏成占位，
      // 展开看不到 SKILL.md 全文（防设计系统泄露），但保留"加载了技能"的痕迹。
    }

    // Add tool card
    addChatItem({
      type: "tool", toolId: p.id, name: p.name, args: p.args,
      output: null, success: null, state: "running",
      sessionId: p.session_id || state.activeSessionId,
    });
    if (isShellExecutionTool(p.name)) {
      scheduleShellPoll(p.session_id || state.activeSessionId, true);
    }
    notify();
  }); });

  listen("chat:tool_delta", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    appendToolItemOutput(p.id, p.content, p.stream);
    scheduleShellNotify();
  }); });

  listen("chat:tool_end", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (toolCallAlreadyFinished(p.id)) return;
    var meta = context.toolMeta[p.id];
    thinkingIdle();
    var resultContent = typeof p.output === "string" ? p.output : JSON.stringify(p.output);
    flushAssistantMessageToHistory();
    var trBlock = { type: "tool_result", tool_use_id: p.id, content: resultContent };
    if (!p.success) trBlock.is_error = true;
    state.messages.push({ role: "user", content: [trBlock] });

    var backgroundTaskId = p.metadata && p.metadata.backgrounded === true &&
      p.metadata.status === "Running" && p.metadata.task_id;
    if (meta && (meta.name === "exec_shell" || meta.name === "Bash") && backgroundTaskId) {
      markBackgroundToolItem(p.id, p.session_id, backgroundTaskId, p.output);
      delete context.toolMeta[p.id];
      context.currentStreamText = ""; context.currentStreamId = 0;
      notify();
      return;
    }

    // request_user_input 结束：把选择卡片标记为已提交/取消，不渲染工具卡
    if (meta && meta.name === "request_user_input") {
      patchLastItem(
        function (it) { return it.type === "user_input" && it.toolCallId === p.id && !it.resolved; },
        { resolved: true, cardState: p.success ? "submitted" : "cancelled" }
      );
      delete context.toolMeta[p.id];
      context.currentStreamText = ""; context.currentStreamId = 0;
      notify();
      return;
    }

    // present_artifact 结束：成功 → 弹成品卡(点击打开);失败 → 落普通工具卡显错误,
    // 让 AI 从 tool_result 看到错误自行重试。成品卡是真工具调用,tool_use 已进
    // messages(tool_start line 784),rerenderFromMessages 按 name 还原,切会话不丢。
    if (meta && isPresentArtifactTool(meta.name)) {
      if (p.success) {
        // 用 server 解析好的绝对路径(present_artifact_server.py 的 abs_path),而非模型可能
        // 给的相对 args.path → 卡片 path 绝对,点 Open 不再报「path must be absolute」。
        var presentedPath = presentArtifactAbsPath(p.output, meta.args && meta.args.path);
        // 同一产物没改又 present 一次 → 跳过出卡(防模型啰嗦重复);改完再 present/续卡会保留。
        if (!isDuplicateArtifactCard(presentedPath)) {
          addChatItem({
            type: "artifact_card",
            path: presentedPath,
            title: (meta.args && meta.args.title) || "",
            description: (meta.args && meta.args.description) || "",
            time: timeStr(),
            sessionId: p.session_id || state.activeSessionId,
          });
        }
        if (presentedPath) state.turnPresentedArtifacts.push(presentedPath); // 本 turn 已出成品卡,chat:done 不再兜底补
        // 同步进产物面板:present_artifact 出卡的产物也算「产出物」。修「自己生成文件、
        // 不走 write_file 的工具(如 make_pptx)→ 卡有、面板无」。trackArtifact 已去重。
        if (presentedPath) trackArtifact(presentedPath);
        delete context.toolMeta[p.id];
        context.currentStreamText = ""; context.currentStreamId = 0;
        notify();
        return;
      }
      // 失败:补一张工具卡承载错误输出(tool_start 时跳过了灰卡)
      addChatItem({
        type: "tool", toolId: p.id, name: meta.name, args: meta.args,
        output: p.output, success: false, state: "done",
      });
      delete context.toolMeta[p.id];
      context.currentStreamText = ""; context.currentStreamId = 0;
      notify();
      return;
    }

    // 通用工具产物兜底：PPT / 公文等 MCP 工具会先返回 {path: "..."}，
    // 随后模型按约定再调 present_artifact。若模型漏调，仍把该成品归到当前
    // tool_end 所属 session，并在 chat:done 统一补一张成品卡。
    if (p.success && meta && shouldUseToolOutputAsArtifact(meta.name)) {
      var producedPath = artifactPathFromToolOutput(p.output);
      if (producedPath && isDeliverable(producedPath)) {
        trackArtifact(producedPath);
        markTurnDirtyArtifact(producedPath);
      }
    }

    // load_skill：卡照出，但不把返回的 SKILL.md 全文写进卡，展开只见占位（防设计系统泄露）。
    var outForCard = (meta && meta.name === "load_skill")
      ? bt("skillContentHidden")
      : context.toolResultDisplayContent(p.output);
    var updatedToolItem = updateToolItem(p.id, outForCard, p.success);
    var shellTaskId = p.metadata && (p.metadata.task_id || p.metadata.taskId);
    if (updatedToolItem && shellTaskId) {
      var syntheticShellItem = state.chatItems.find(function (it) {
        return it !== updatedToolItem && it.shellSnapshot === true && it.taskId === shellTaskId;
      });
      if (syntheticShellItem) {
        ["shellStatus", "exitCode", "elapsedMs", "output", "state", "success", "shellSnapshotKey"]
          .forEach(function (key) {
            if (syntheticShellItem[key] !== undefined) updatedToolItem[key] = syntheticShellItem[key];
          });
        var syntheticIndex = state.chatItems.indexOf(syntheticShellItem);
        if (syntheticIndex >= 0) state.chatItems.splice(syntheticIndex, 1);
      }
      updatedToolItem.taskId = shellTaskId;
      updatedToolItem.sessionId = p.session_id || state.activeSessionId;
      var shellStatus = String((p.metadata && p.metadata.status) || "").toLowerCase();
      if (shellStatus === "running" || /running|background/i.test(String(p.output || ""))) {
        updatedToolItem.state = "running";
        updatedToolItem.success = null;
      }
      scheduleShellPoll(updatedToolItem.sessionId, true);
    }

    // Careful hook：CodeWhale shell.rs 拦截 Dangerous → 红色拦截卡
    var md = p.metadata;
    if (md && md.safety_level === "dangerous" && md.blocked) {
      addChatItem({ type: "careful_blocked", args: meta && meta.args, metadata: md, time: timeStr() });
    }

    // File.write/File.edit/File.patch 改了产物 → 记账,turn 结束(chat:done)统一补成品卡。
    // 改成记账+去重:AI 一个 turn 会 edit 多次,实时续会刷出一堆卡;且 edit
    // 之前不触发续卡 → 改完没新卡片 → 没法对改后产物再召唤 pinvou(核账闭环断裂)。
    var mutationAction = meta && fileMutationAction(meta.name, meta.args);
    if (p.success && mutationAction) {
      extractArtifactPaths(meta.args).forEach(function (ap) {
        // 面板只收「成品」:成品型扩展名(自动当成品)或之前 present_artifact 过的文件;
        // 中间草稿(content_p1.txt / *_params.json 等)不进面板。edit_file 只改已有不新建。
        if (mutationAction !== "edit" && (isDeliverable(ap) || findPresentedArtifact(ap))) trackArtifact(ap);
        // 产物(present 过的成品 或 write/append 写进产物列表的)被写/改 → turn 结束补卡。
        // 不再要求 present 过:AI 经常写完产物忘了 present_artifact → 没成品卡 = 没召唤入口。
        // 按 basename 比对:disk watcher(artifact:disk)写盘后抢先用**绝对**路径 trackArtifact
        // 占了名额,而这里 ap 是 write_file 的**相对**参数 —— 用 a.path===ap 比绝对≠相对永远落空,
        // turnDirty 收不到 → 实时不补成品卡(只能靠重启 rerender 才出)。basename 比对消除该竞态。
        var _apbn = basename(ap);
        var isArtifact = !!findPresentedArtifact(ap) || state.artifacts.some(function (a) { return basename(a.path) === _apbn; });
        if (isArtifact) markTurnDirtyArtifact(ap);
      });
    }

    // 兜底：Plan 模式下 AI 调了被白名单/sandbox 拦的工具 → 弹兜底卡，给两条出路
    if (!p.success && state.modeState.mode === "plan" && typeof p.output === "string" &&
        (p.output.includes("not available in the current tool catalog") ||
         p.output.includes("unavailable in Plan mode") ||
         p.output.includes("PermissionDenied"))) {
      if (!hasUnresolvedItem("plan_stuck")) {
        addChatItem({ type: "plan_stuck", toolName: meta && meta.name, resolved: false, time: timeStr() });
      }
    }

    delete context.toolMeta[p.id];
    context.currentStreamText = "";
    context.currentStreamId = 0;
    notify();
  }); });

  listen("chat:shell_task_status", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    finishBackgroundToolItem(p.tool_id, p);
    notify();
  }); });

  // chat:done 特殊:同步收尾(flush/busy=false/mode 复位)走 runSyncOnSession
  // 路由到对应 session;异步收尾(discard_plan/落盘/刷新列表)按显式 sid 路由,
  // 不依赖工作集 —— 这样后台 session 跑完也能正确落盘。
  listen("chat:done", function (e) {
    var sid = (e.payload && e.payload.session_id) || state.activeSessionId;
    var knownDoneSession = !!sid && state.sessions.some(function (session) { return session.id === sid; });
    var scheduledDoneSession = isScheduledRunSession(sid);
    if (sid && sid !== state.activeSessionId && !sessionStates[sid] &&
        !knownDoneSession && !scheduledDoneSession) {
      return;
    }
    safeConsoleInfo("[pinvou3][chat-ui] chat done event", {
      sid: sid,
      error: e.payload && e.payload.error || null,
    });
    var doneBuffer = sid ? getBuffer(sid) : null;
    var requiresAuthorityReconcile = !isScheduledRunSession(sid);
    var completedLocalTurn = !!(
      requiresAuthorityReconcile && doneBuffer && doneBuffer.localTurnOwned
    );
    recordAuthoritySyncDiagnostic("chat_done_classified", Object.assign({
      completed_local_turn: completedLocalTurn,
      requires_authority_reconcile: requiresAuthorityReconcile,
      terminal_status: String(e.payload && e.payload.status || ""),
      terminal_error_present: !!(e.payload && e.payload.error),
    }, authoritySyncBufferSnapshot(sid, doneBuffer)));
    if (requiresAuthorityReconcile && doneBuffer && !doneBuffer.localTurnOwned) {
      // transcript_committed is emitted before chat:done. A client that joins
      // at the terminal tail may not have seen an earlier turn event, so keep
      // the already received revision while initializing remote-turn state.
      markRemoteTurn(sid, doneBuffer, true, "chat_done_without_local_owner");
    }
    if (!requiresAuthorityReconcile) markScheduledInitialTurnTerminal(sid);
    runSyncOnSession(sid, function () {
      var error = e.payload && e.payload.error;
      recordTurnCompleted(e.payload || {});
      refreshEffectiveModelConfigAfterAuthError(error);
      if (error) {
        var finalNotice = "⚠️ " + error;
        var finalNoticeItem = state.chatItems.find(function (item) {
          return item && item.turnErrorNotice && item.text === finalNotice;
        });
        if (finalNoticeItem) {
          finalNoticeItem.legacyConversationOnly = true;
        } else {
          addSystemItem(finalNotice, {
            turnErrorNotice: true,
            legacyConversationOnly: true,
          });
        }
      }
      window.PinvouBridgeMessages.showShellCleanupFailure(e.payload, state, addSystemItem);
      var terminalStatus = String(e.payload && e.payload.status || "").toLowerCase();
      var interrupted = terminalStatus === "interrupted" ||
        terminalStatus === "cancelled" || terminalStatus === "canceled";
      if (interrupted) preserveInterruptedAssistantPresentation();
      else flushAssistantMessageToHistory();
      // 本 turn 写/改过的产物 → 末尾补一张成品卡(带召唤图标),让 Boss 就近召唤 pinvou。
      // present 过的复用其 title/desc;AI 没 present 的兜底用文件名补首卡(否则没召唤入口=这次的 bug)。
      // 本 turn 刚 present_artifact 出过卡的跳过,不重复。edit/append 改多次也只补一张。
      (state.turnDirtyArtifacts || []).forEach(function (ap) {
        // 按 basename 比对:present 存 server 绝对路径、turnDirty 存 write 相对路径,
        // 直接 indexOf 比不中 → present 过的文件会被兜底再补一张(重复)。
        var _apbn = basename(ap);
        if ((state.turnPresentedArtifacts || []).some(function (pp) { return basename(pp) === _apbn; })) return;
        var prev = findPresentedArtifact(ap);
        // 补卡 path 优先用 disk watcher 落进产物列表的同名**绝对**路径(open 可靠、跨 session 稳);
        // 没有再退回 write_file 的相对 ap(由 sessionId 兜底解析)。
        var tracked = state.artifacts.find(function (a) { return basename(a.path) === _apbn && isAbsPath(a.path); });
        var cardPath = (tracked && tracked.path) || ap;
        if (prev) addChatItem({ type: "artifact_card", path: prev.path, title: prev.title, description: prev.description, time: timeStr(), sessionId: sid });
        else addChatItem({ type: "artifact_card", path: cardPath, title: basename(ap), description: "", time: timeStr(), sessionId: sid });
      });
      state.turnDirtyArtifacts = [];
      state.turnPresentedArtifacts = [];
      finalizeStreamingReasoning();
      // Finalize streaming bubble
      var streamItem = state.chatItems.find(function (it) { return it.id === context.currentStreamId; });
      if (streamItem) streamItem.streaming = false;
      // Remove empty assistant bubbles
      state.chatItems = state.chatItems.filter(function (it) {
        return !(it.type === "assistant" && !it.html);
      });
      state.busy = false;
      stopThinking();
      context.currentStreamText = "";
      context.currentStreamId = 0;
    });
    if (requiresAuthorityReconcile && doneBuffer && !completedLocalTurn) {
      var finalAssistantMessage = null;
      for (var doneMessageIndex = doneBuffer.messages.length - 1; doneMessageIndex >= 0; doneMessageIndex--) {
        if (doneBuffer.messages[doneMessageIndex] && doneBuffer.messages[doneMessageIndex].role === "assistant") {
          finalAssistantMessage = doneBuffer.messages[doneMessageIndex];
          break;
        }
      }
      doneBuffer.remoteExpectedAssistantKey = finalAssistantMessage
        ? hydratedMessageKey(finalAssistantMessage, isScheduledRunSession(sid))
        : "";
      if (doneBuffer.localTurnOwned) doneBuffer.deferredRemoteUserEvent = null;
      doneBuffer.localTurnOwned = false;
      doneBuffer.remoteTurnActive = true;
      doneBuffer.remoteTerminalSeen = true;
      doneBuffer.busy = false;
      if (sid === state.activeSessionId) saveWorkingSetTo(doneBuffer);
    } else if (completedLocalTurn) {
      // The desktop owns this turn and Rust has already persisted its terminal
      // transcript before emitting chat:done. Do not convert a completed local
      // turn into a remote authority gate: a best-effort readback failure must
      // never block the user's next local message.
      doneBuffer.deferredRemoteUserEvent = null;
      doneBuffer.localTurnOwned = false;
      doneBuffer.remoteTurnActive = false;
      doneBuffer.remoteTerminalSeen = false;
      doneBuffer.remoteBaselineMessageCount = null;
      doneBuffer.remoteBaselineTrusted = false;
      doneBuffer.remoteExpectedAssistantKey = "";
      doneBuffer.remoteCommittedRevision = "";
      doneBuffer.busy = false;
      if (sid === state.activeSessionId) saveWorkingSetTo(doneBuffer);
    }
    notify();
    refreshAuthoritativeTurnTimeline(sid);
    // 异步收尾(按 sid 路由,active/后台通用)
    (async function () {
      await persistMessagesFor(sid);
      var reconciled = requiresAuthorityReconcile && !completedLocalTurn
        ? await reconcileRemoteTurn(sid)
        : true;
      if (reconciled) await persistMessagesFor(sid);
      await refreshHistoryList();
      if (!reconciled) {
        recordAuthoritySyncDiagnostic("authority_sync_notice_shown", Object.assign({
          notice: "desktop_done_sync_pending",
        }, authoritySyncBufferSnapshot(sid, doneBuffer)));
        runSyncOnSession(sid, function () {
          addAuthoritySyncNotice(bt("desktopDoneSyncPending"));
        });
      }
      notify();
      publishRemoteLiveSnapshot(sid).catch(function () {});
      // 排队式:本轮跑完,若该 session 不忙且有待发消息 → 自动发下一条
      if (reconciled) flushQueued(sid);
    })();
  });

  listen("chat:usage", function (e) { onSessionEvent(e, function () {
    var sid = e.payload && e.payload.session_id;
    // 真实窗口是模型能力常量，不随轮内请求数变化，必须先于 dirty guard 消费：
    // 工具轮（最常见的 Agent 场景）只跳过不可信的累计 input，分母仍要更新。
    var windowTok = Number(e.payload && e.payload.context_window) || 0;
    if (windowTok > 0 && windowTok !== state.tokens.max) {
      state.tokens.max = windowTok; // 云端真实窗口，替代 32K 假分母
      notify(); // 窗口变化也要通知 UI（即使本轮 input 不可信）
    }
    if (sid && turnUsageDirty[sid]) return; // 本轮多请求，累加 input 不可信，保留上个准确值
    var input = Number(e.payload && e.payload.input_tokens || 0);
    // 累加值超过窗口说明仍有多请求（内部重试等无事件轮），跳过避免显示超上限
    if (input > 0 && input <= state.tokens.max) {
      state.tokens = { input: input, max: state.tokens.max };
      notify();
    }
  }); });

  listen("chat:compaction", function (e) { onSessionEvent(e, function () {
    if (e.payload && e.payload.session_id) turnUsageDirty[e.payload.session_id] = true; // 压缩轮 usage 含摘要请求
    var phase = e.payload && e.payload.phase;
    var msg = e.payload && e.payload.message || "";
    var auto = e.payload && e.payload.auto ? bt("compactAuto") : "";
    var compactId = e.payload && e.payload.id;
    var before = Number(e.payload && e.payload.messages_before);
    var after = Number(e.payload && e.payload.messages_after);
    var looksLikePruneOnly = /0 removed|messages unchanged|tool results pruned/i.test(msg);
    var pruneOnlyAuto = !!(e.payload && e.payload.auto) &&
      phase === "done" &&
      Number.isFinite(before) &&
      Number.isFinite(after) &&
      before === after &&
      looksLikePruneOnly &&
      msg.indexOf("Emergency compaction") !== 0;
    if (phase === "start") addSystemItem(bt("compactStart") + auto + " " + msg, { compactId: compactId, compactPhase: "start" });
    else if (phase === "done" && pruneOnlyAuto) addOrMergePruneCompaction(compactId);
    else if (phase === "done") addSystemItem(bt("compactDone") + auto + " " + msg);
    else if (phase === "fail") addSystemItem(bt("compactFail") + auto + ": " + msg);
  }); });

  // ── request_user_input：渲染选择卡片（不进 messages.json）─────────
  // payload: { id: tool_call_id, questions: [{header, id, question, options:[{label, description}]}] }
  listen("chat:user_input_required", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (hasChatItemForTool("user_input", p.id)) return;
    var questions = p.questions || [];
    if (!Array.isArray(questions) || questions.length === 0) return;
    addChatItem({
      type: "user_input", toolCallId: p.id, questions: questions,
      resolved: false, cardState: "active", time: timeStr(),
    });
    notify();
  }); });

  // 可恢复的瞬态错误（SSE idle timeout / 瞬态工具失败）：turn 没结束，引擎会 retry，
  // 绝不 setBusy(false)，只飘一条 ⚠️ 提示。
  listen("chat:transient_error", function (e) { onSessionEvent(e, function () {
    if (e.payload && e.payload.session_id) turnUsageDirty[e.payload.session_id] = true; // 重试轮 usage 含重发请求
    var error = e.payload && e.payload.error;
    refreshEffectiveModelConfigAfterAuthError(error);
    if (error) {
      var notice = "⚠️ " + error;
      var duplicate = state.chatItems.some(function (item) {
        return item && item.turnErrorNotice && item.text === notice;
      });
      if (!duplicate) addSystemItem(notice, { turnErrorNotice: true });
    }
  }); });

  // File watcher 推送的产物事件：session workspace 下新文件/修改/删除。
  // 路由到对应 session 的产物列表(后台 session 的产物也跟踪)。
  listen("artifact:disk", function (e) {
    var p = e.payload || {};
    if (!p.path) return;
    onSessionEvent(e, function () {
      noteArtifactChange(p.path, p.event || "modified", p.session_id || state.activeSessionId || "");
      if (p.event === "removed") { untrackArtifact(p.path); return; }
      // 面板只收成品:成品型扩展名 或 present_artifact 过的;中间 / infra / 目录不进面板
      // (file_watcher 递归会推 tmp/ _state/ 等子目录与 infra 文件 → 此处兜住)。
      if (isDeliverable(p.path) || findPresentedArtifact(p.path)) trackArtifact(p.path);
    });
  });

  listen("remote_control:mobile_user_message", async function (e) {
    var p = e.payload || {};
    var sid = p.session_id;
    var content = (p.content || "").trim();
    var attachments = p.attachments || [];
    // 允许纯附件消息(content 为空但 attachments 非空),对齐 Group E user_message 改造。
    if (!sid || (!content && !attachments.length)) return;
    try { await ensureSessionBufferLoaded(sid); }
    catch (err) {
      console.warn("remote session hydrate failed", err);
      return;
    }
    var attachmentNames = attachments.map(function (attachment) {
      return attachment && attachment.basename;
    }).filter(Boolean);
    var displayText = attachmentNames.length
      ? content + (content ? "\n\n" : "") + "📎 " + JSON.stringify(attachmentNames)
      : content;
    var remoteBuffer = getBuffer(sid);
    if (isBusyFor(sid) || (remoteBuffer && remoteBuffer.queued && remoteBuffer.queued.length > 0)) {
      runSyncOnSession(sid, function () {
        state.queued.push({
          id: ++context.itemIdSeq,
          text: content,
          displayText: displayText,
          attachments: attachments,
          meta: { remoteClientMessageId: p.client_message_id || null },
        });
      });
      notify();
      if (!isBusyFor(sid)) flushQueued(sid);
      return;
    }
    doSendFor(sid, content, displayText, attachments, { remoteClientMessageId: p.client_message_id || null });
  });

  // 远程 mobile 改工具开关 → Rust emit remote_control:tools_changed → 这里桥接到
  // 桌面前端监听的 DOM CustomEvent 'pinvou:tools-changed'(tool-events.js / 类似入口),
  // 让 chip 上的工具开关计数立即同步。
  listen("remote_control:tools_changed", function () {
    try { window.dispatchEvent(new CustomEvent('pinvou:tools-changed')); } catch (_) {}
  });

  // 远程 mobile 挂载/摘挂 KB → Rust emit remote_control:kb_mount_changed → 这里同步
  // 桌面前端多知识库状态(由 ChatView 渲染 KB 指示器)。否则 mobile 切了 KB,
  // 桌面端 chip 仍显旧状态直到用户切 session 强制重读。
  // 新 payload 带 collections；collection_id 保留给旧远程端兼容。
  // 只处理当前 active session 的变更(其他 session 的挂载不影响当前视图)。
  var kbMountSyncGeneration = 0;
  function normalizeMountedCollections(value) {
    if (!Array.isArray(value)) return [];
    var seen = Object.create(null);
    return value.map(function (entry) {
      if (entry == null) return null;
      var collectionId = typeof entry === "object"
        ? (entry.collectionId != null ? entry.collectionId : entry.collection_id)
        : entry;
      if (collectionId == null || seen[String(collectionId)]) return null;
      seen[String(collectionId)] = true;
      return { collectionId: collectionId, enabled: typeof entry === "object" ? entry.enabled !== false : true };
    }).filter(Boolean);
  }
  function normalizeMountedRemoteCollections(value) {
    if (!Array.isArray(value)) return [];
    var seen = Object.create(null);
    return value.map(function (entry) {
      if (!entry || typeof entry !== "object") return null;
      var serverId = entry.serverId != null ? entry.serverId : entry.server_id;
      var collectionId = entry.collectionId != null ? entry.collectionId : entry.collection_id;
      if (!serverId || collectionId == null) return null;
      var key = String(serverId) + ":" + String(collectionId);
      if (seen[key]) return null;
      seen[key] = true;
      return { serverId: serverId, collectionId: collectionId, enabled: entry.enabled !== false };
    }).filter(Boolean);
  }
  listen("remote_control:kb_mount_changed", function (e) {
    var p = e && e.payload;
    if (!p || !state.activeSessionId) return;
    if (p.session_id !== state.activeSessionId) return;
    var sessionId = p.session_id;
    var generation = ++kbMountSyncGeneration;
    var payloadMounted = Array.isArray(p.collections)
      ? normalizeMountedCollections(p.collections)
      : (p.collection_id == null ? [] : [{ collectionId: p.collection_id, enabled: true }]);
    function normalizeSnapshot(value) {
      if (value && !Array.isArray(value) && Array.isArray(value.collections)) {
        return { revision: Number(value.revision || 0), collections: value.collections };
      }
      return { revision: 0, collections: Array.isArray(value) ? value : payloadMounted };
    }
    function commit(value) {
      if (generation !== kbMountSyncGeneration || state.activeSessionId !== sessionId) return;
      var snapshot = normalizeSnapshot(value);
      if (snapshot.revision < Number(state.mountedCollectionsRevision || 0)) return;
      var mounted = normalizeMountedCollections(snapshot.collections);
      state.mountedCollections = mounted;
      state.mountedCollectionsRevision = snapshot.revision;
      var firstEnabled = mounted.find(function (entry) { return entry.enabled; });
      state.mountedCollection = firstEnabled ? firstEnabled.collectionId : null;
      notify();
    }
    function commitRemote(value) {
      if (generation !== kbMountSyncGeneration || state.activeSessionId !== sessionId) return;
      if (!Array.isArray(value)) return;
      state.mountedRemoteCollections = normalizeMountedRemoteCollections(value);
      notify();
    }
    var payloadRemote = Array.isArray(p.remote_collections)
      ? p.remote_collections
      : (Array.isArray(p.remoteCollections) ? p.remoteCollections : null);
    if (payloadRemote) {
      // Collection deletion carries the post-cascade list, so the composer drops its chip
      // synchronously instead of waiting for another IPC round trip.
      commitRemote(payloadRemote);
    } else {
      // Older producers omit remote mounts. Re-read the session fact source so this shared event
      // still converges both local and remote mount state.
      invoke("session_mounted_remote_collections", { sessionId: sessionId })
        .then(function (collections) { commitRemote(collections); })
        .catch(function () {});
    }
    // 事件可能由并发命令乱序发出；重新读取后端事实源，并以 generation 防止旧请求晚回覆盖。
    invoke("session_mounted_collections_snapshot", { sessionId: sessionId })
      .then(function (snapshot) { commit(snapshot); })
      .catch(function () {
        commit({ revision: Number(p.revision || 0), collections: payloadMounted });
      });
  });

  // 本地语音识别依赖安装进度（模型下载 / ffmpeg 安装）
  listen("voice_asr:progress", function (e) {
    var p = e && e.payload;
    if (!p) return;
    state.voiceAsrSetup = Object.assign({}, state.voiceAsrSetup, { progress: p });
    notify();
  });

  // vllm-setup:phase —— MegaCube 本地大模型引导阶段(authorizing→waiting{attempt}→ready),驱动引导框步骤指示。
  listen("vllm-setup:phase", function (e) {
    var p = e.payload || {};
    if (!p.phase) return;
    state.vllmSetupPhase = p.phase;
    if (typeof p.attempt === "number") state.vllmSetupAttempt = p.attempt;
    notify();
  });

  // 知识库 embedding 模型下载进度（download → verify → prepare → done）
  listen("kb_model:progress", function (e) {
    var p = e && e.payload;
    if (!p) return;
    state.kbModelSetup = Object.assign({}, state.kbModelSetup, { progress: p });
    notify();
  });

  // chat:plan_snapshot —— update_plan/checklist_write 后实时更新进度，与 plan_ready 解耦
  listen("chat:plan_snapshot", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    if (p.plan_snapshot) state.planSnapshot.plan = p.plan_snapshot;
    if (p.todos_snapshot) state.planSnapshot.todos = p.todos_snapshot;
    notify();
  }); });

  // chat:plan_ready —— 底座式:Plan 模式调过 update_plan 即弹方案卡(快照非空)
  listen("chat:plan_ready", function (e) { onSessionEvent(e, function () {
    var p = e.payload || {};
    var planId = String(p.plan_id || p.planId || "").trim();
    var readyMode = p.mode_state || p.modeState;
    // 事件负载的权威 mode 写回走收敛点（bump epoch 防在途旧读覆盖；
    // sid 取事件 payload，onSessionEvent 内与 state.activeSessionId 一致）。
    if (readyMode) applyAuthoritativeModeState(state.activeSessionId, readyMode);
    if (planId && state.chatItems.some(function (item) {
      return item && item.type === "plan_card" && String(item.planId || "") === planId;
    })) return;
    // 新方案出现 → 旧的 active 方案卡冻结
    state.chatItems.forEach(function (it) {
      if (it.type === "plan_card" && it.cardState === "active") {
        it.cardState = "frozen"; it.statusLabel = bt("planSuperseded");
      }
    });
    var snaps = { plan: p.plan_snapshot || null, todos: p.todos_snapshot || null };
    addChatItem({
      type: "plan_card", plan: snaps.plan, todos: snaps.todos,
      planMarkdown: composePlanMarkdown(snaps), planId: planId || null,
      cardState: planId ? "active" : "frozen", resolved: !planId,
      planResolutionConfirmed: false,
      statusLabel: planId ? "" : bt("planHistorical"), time: timeStr(),
    });
    notify();
  }); });

  listen("chat:plan_resolved", function (e) {
    var p = e && e.payload || {};
    var sid = p.session_id || state.activeSessionId;
    var planId = String(p.plan_id || p.planId || "").trim();
    if (!sid || !planId) return;
    runSyncOnSession(sid, function () {
      state.chatItems.forEach(function (item) {
        if (item && item.type === "plan_card" && String(item.planId || "") === planId) {
          item.cardState = "frozen";
          item.resolved = true;
          item.planResolutionConfirmed = true;
          item.statusLabel = bt("planDiscarded");
        }
      });
      var resolvedMode = p.mode_state || p.modeState;
      // 事件负载的权威 mode 写回走收敛点（bump epoch 防在途旧读覆盖），
      // 与 web 版对齐：方案在别处（另一窗口/远端）被 discard 时 chip 须刷新。
      if (resolvedMode) applyAuthoritativeModeState(sid, resolvedMode);
    });
    notify();
  });

    return {
      latestTimelineCompletion: latestTimelineCompletion,
      authoritativeTimelineMissesKnownCompletion: authoritativeTimelineMissesKnownCompletion,
      refreshAuthoritativeTurnTimeline: refreshAuthoritativeTurnTimeline,
    };
  };
})();
