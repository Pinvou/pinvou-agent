(function () {
  "use strict";

  var registry = window.__PINVOU_TAURI_BRIDGE_FEATURES__ = window.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["chat-events"] = function (context) {
    var state = context.state;
    var listen = context.listen;
    var notify = context.notify;
    var invoke = context.invoke;
    var turnUsageDirty = context.turnUsageDirty;
    var sessionStates = context.sessionStates;
    var renderMarkdown = context.renderMarkdown;
    var bt = context.bt;
    var onSessionEvent = context.onSessionEvent;
    var runSyncOnSession = context.runSyncOnSession;
    var addChatItem = context.addChatItem;
    var addSystemItem = context.addSystemItem;
    var timeStr = context.timeStr;
    var flushPendingTextBlock = context.flushPendingTextBlock;
    var flushAssistantMessageToHistory = context.flushAssistantMessageToHistory;
    var resetPendingAssistant = context.resetPendingAssistant;
    var flushQueued = context.flushQueued;
    var isBusyFor = context.isBusyFor;
    var doSendFor = context.doSendFor;
    var ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    var thinkingTool = context.thinkingTool;
    var thinkingIdle = context.thinkingIdle;
    var stopThinking = context.stopThinking;
    var scheduleScheduledRunRefresh = context.scheduleScheduledRunRefresh;
    var handleMemoryWrite = context.handleMemoryWrite;
    var isPresentArtifactTool = context.isPresentArtifactTool;
    var artifactPathFromToolOutput = context.artifactPathFromToolOutput;
    var shouldUseToolOutputAsArtifact = context.shouldUseToolOutputAsArtifact;
    var presentArtifactAbsPath = context.presentArtifactAbsPath;
    var extractArtifactPath = context.extractArtifactPath;

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
  listen("chat:delta", function (e) { onSessionEvent(e, function () {
    var text = e.payload && e.payload.text || "";
    context.pendingAssistantText += text;
    context.currentStreamText += text;
    // Update the streaming chat item
    var item = state.chatItems.find(function (it) { return it.id === context.currentStreamId; });
    if (item) {
      item.html = renderMarkdown(context.currentStreamText);
      item.streaming = true;
    } else {
      // New bubble needed (after tool card)
      context.currentStreamId = ++context.itemIdSeq;
      state.chatItems.push({
        id: context.currentStreamId,
        type: "assistant",
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
    if (p.session_id) turnUsageDirty[p.session_id] = true; // 多请求轮，usage 累加值不可当占用
    context.toolMeta[p.id] = { name: p.name, args: p.args };
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

    // load_skill：模型加载技能 → 点亮 composer 技能标（内置自动技能"正在使用"指示），
    // 不渲染裸工具卡（用药丸指示器替代）。当前只识别视觉设计。
    if (p.name === "load_skill") {
      var skArg = ((p.args && (p.args.name || p.args.skill)) || "").toString();
      if (skArg.indexOf("视觉设计") >= 0 || skArg.toLowerCase().indexOf("visual-design") >= 0) {
        state.activeSkill = "visual-design";
      }
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
    var meta = context.toolMeta[p.id];
    thinkingIdle();
    var resultContent = typeof p.output === "string" ? p.output : JSON.stringify(p.output);
    flushAssistantMessageToHistory();
    var trBlock = { type: "tool_result", tool_use_id: p.id, content: resultContent };
    if (!p.success) trBlock.is_error = true;
    state.messages.push({ role: "user", content: [trBlock] });

    var backgroundTaskId = p.metadata && p.metadata.backgrounded === true &&
      p.metadata.status === "Running" && p.metadata.task_id;
    if (meta && meta.name === "exec_shell" && backgroundTaskId) {
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
    var outForCard = (meta && meta.name === "load_skill") ? "（技能已加载，内容不展示）" : p.output;
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

    // Careful hook：DeepSeek-TUI shell.rs 拦截 Dangerous → 红色拦截卡
    var md = p.metadata;
    if (md && md.safety_level === "dangerous" && md.blocked) {
      addChatItem({ type: "careful_blocked", args: meta && meta.args, metadata: md, time: timeStr() });
    }

    // write_file/append_file/edit_file 改了产物 → 记账,turn 结束(chat:done)统一补成品卡。
    // 改成记账+去重:AI 一个 turn 会 edit_file 改很多次,实时续会刷出一堆卡;且 edit_file
    // 之前不触发续卡 → 改完没新卡片 → 没法对改后产物再召唤 pinvou(核账闭环断裂)。
    if (p.success && meta && (meta.name === "write_file" || meta.name === "append_file" || meta.name === "edit_file")) {
      var ap = extractArtifactPath(meta.args);
      if (ap) {
        // 面板只收「成品」:成品型扩展名(自动当成品)或之前 present_artifact 过的文件;
        // 中间草稿(content_p1.txt / *_params.json 等)不进面板。edit_file 只改已有不新建。
        if (meta.name !== "edit_file" && (isDeliverable(ap) || findPresentedArtifact(ap))) trackArtifact(ap);
        // 产物(present 过的成品 或 write/append 写进产物列表的)被写/改 → turn 结束补卡。
        // 不再要求 present 过:AI 经常写完产物忘了 present_artifact → 没成品卡 = 没召唤入口。
        // 按 basename 比对:disk watcher(artifact:disk)写盘后抢先用**绝对**路径 trackArtifact
        // 占了名额,而这里 ap 是 write_file 的**相对**参数 —— 用 a.path===ap 比绝对≠相对永远落空,
        // turnDirty 收不到 → 实时不补成品卡(只能靠重启 rerender 才出)。basename 比对消除该竞态。
        var _apbn = basename(ap);
        var isArtifact = !!findPresentedArtifact(ap) || state.artifacts.some(function (a) { return basename(a.path) === _apbn; });
        if (isArtifact) markTurnDirtyArtifact(ap);
      }
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
    safeConsoleInfo("[pinvou3][chat-ui] chat done event", {
      sid: sid,
      error: e.payload && e.payload.error || null,
    });
    if (isScheduledRunSession(sid)) markScheduledInitialTurnTerminal(sid);
    runSyncOnSession(sid, function () {
      var error = e.payload && e.payload.error;
      refreshEffectiveModelConfigAfterAuthError(error);
      if (error) addSystemItem("⚠️ " + error);
      flushAssistantMessageToHistory();
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
    notify();
    // 异步收尾(按 sid 路由,active/后台通用)
    (async function () {
      await persistMessagesFor(sid);
      await refreshHistoryList();
      notify();
      publishRemoteLiveSnapshot(sid).catch(function () {});
      // 排队式:本轮跑完,若该 session 不忙且有待发消息 → 自动发下一条
      flushQueued(sid);
    })();
  });

  listen("chat:usage", function (e) { onSessionEvent(e, function () {
    var sid = e.payload && e.payload.session_id;
    if (sid && turnUsageDirty[sid]) return; // 本轮多请求，累加值≠占用，保留上个准确值
    var input = Number(e.payload && e.payload.input_tokens || 0);
    if (input > 0) {
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
    if (state.workflow.run.status === "stopped" &&
        state.workflow.run.sessionId && p.session_id === state.workflow.run.sessionId) return;
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
    if (error) addSystemItem("⚠️ " + error);
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
      ? content + (content ? "\n\n" : "") + "📎 " + attachmentNames.join(" · ")
      : content;
    if (isBusyFor(sid)) {
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
  // 桌面前端 state.mountedCollection(由 ChatView 渲染 KB 指示器)。否则 mobile 切了 KB,
  // 桌面端 chip 仍显旧状态直到用户切 session 强制重读。
  // payload 形状:{ session_id, collection_id } 或 { session_id, collection_id: null }。
  // 只处理当前 active session 的变更(其他 session 的挂载不影响当前视图)。
  listen("remote_control:kb_mount_changed", function (e) {
    var p = e && e.payload;
    if (!p || !state.activeSessionId) return;
    if (p.session_id !== state.activeSessionId) return;
    var cid = (p.collection_id == null) ? null : p.collection_id;
    if (state.mountedCollection === cid) return;
    state.mountedCollection = cid;
    notify();
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

  // 知识库 embedding 模型下载进度（download → verify → extract → done）
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
    // 新方案出现 → 旧的 active 方案卡冻结
    state.chatItems.forEach(function (it) {
      if (it.type === "plan_card" && it.cardState === "active") {
        it.cardState = "frozen"; it.statusLabel = bt("planSuperseded");
      }
    });
    var snaps = { plan: p.plan_snapshot || null, todos: p.todos_snapshot || null };
    addChatItem({
      type: "plan_card", plan: snaps.plan, todos: snaps.todos,
      planMarkdown: composePlanMarkdown(snaps), cardState: "active", statusLabel: "", time: timeStr(),
    });
    notify();
  }); });

  // workflow:project_started —— start_workflow 后端建项目+绑定 session 后 emit。
  // 必须真正 switchToSession 切过去（load 新 session 的空 messages + sync engine +
  // syncSessionSkill），否则只设 activeSessionId 会让旧对话的 messages 残留在屏上，
  // 顶部又叠加 PhaseChips，看起来像"旧对话被 append 了项目名"（Phase A 关键 bug）。
  // refreshHistoryList 先跑让新 session 进 sidebar 列表 + 刷 bindings(🧭)。
  // switchToSession 内部已调 syncSessionSkill，切完 App useEffect 自动 setCurrentView('chat')。
  // [卡片流] start_workflow 后端建项目+绑定 session 后 emit。
  // 新设计：**不再 switchToSession 跳聊天页** —— 用户停在工作流看板，
  // 工作流 session 作为后台 session 跑，看板靠下面的 workflow:* 事件按 session_id 驱动。
  listen("workflow:project_started", async function (e) {
    var p = e.payload || {};
    state.workflow.run = {
      active: true, sessionId: p.session_id || null, projectDir: p.project_dir || null,
      scenario: p.scenario || null, status: "running", agents: {}, cards: [], selectedRole: null,
    };
    await refreshHistoryList();
    notify();
  });


    return {};
  };
})();
