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
    var runSyncOnSession = context.runSyncOnSession;
    var startThinking = context.startThinking;
    var ensureSessionBufferLoaded = context.ensureSessionBufferLoaded;
    var ensureSession = context.ensureSession;
    var clearAttachments = context.clearAttachments;
    var isScheduledRunSession = context.isScheduledRunSession;
    var basename = context.basename;
    var extractArtifactPath = context.extractArtifactPath;
    var parseScheduledTaskDraftFromText = context.parseScheduledTaskDraftFromText;
    var autoCreateScheduledTaskDraft = context.autoCreateScheduledTaskDraft;

  // ── Chat Items (display format for React) ────────────────────────
  function addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
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
      if (it.type === "tool" && (it.name === "write_file" || it.name === "append_file" || it.name === "edit_file")) {
        var ap = extractArtifactPath(it.args);
        if (ap && basename(ap) === bn) return false;
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
  // 桌宠窗口靠全局事件感知回合起止。turn_start 补齐"发送 → 首 token"的空窗
  // (chat:delta 之前引擎在思考,宠物不该干站着);turn_end 只兜 invoke 直接失败
  // 这种不会有 chat:done 的路径。JS emit 是全局广播,宠物窗口 listen 收得到。
  function emitPetEvent(name, sid) {
    try {
      if (TAURI && TAURI.event && TAURI.event.emit) TAURI.event.emit(name, { session_id: sid });
    } catch (_) { /* 桌宠是纯装饰,广播失败不影响对话 */ }
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
    runSyncOnSession(sid, function () {
      var uitem = { type: "user", text: displayText, time: timeStr() };
      if (meta && meta.pinvouTransfer) uitem.pinvouTransfer = meta.pinvouTransfer; // 仅展示层,不进 messages/LLM
      addChatItem(uitem);
      state.messages.push({ role: "user", content: [{ type: "text", text: displayText }] });
      state.busy = true;
      startThinking();
      context.currentStreamText = "";
      context.currentStreamId = ++context.itemIdSeq;
      state.chatItems.push({ id: context.currentStreamId, type: "assistant", html: "", time: timeStr(), streaming: true });
    });
    notify();
    emitPetEvent("pet:turn_start", sid);
    publishRemoteUserMessage(sid, displayText, meta && meta.remoteClientMessageId);
    return invoke("chat", { message: text, attachments: attachmentsPayload, sessionId: sid, restrictTools: !!restrictTools })
      .catch(function (err) {
        console.warn("[pinvou3][chat-ui] send failed", {
          sid: sid,
          error: err && err.toString ? err.toString() : err,
        });
        emitPetEvent("pet:turn_end", sid);
        runSyncOnSession(sid, function () {
          addSystemItem("⚠️ " + (err && err.toString ? err.toString() : err));
          state.busy = false;
          state.chatItems = state.chatItems.filter(function (item) { return item.id !== context.currentStreamId || item.html; });
        });
        notify();
        flushQueued(sid);
        if (surfaceFailure) throw err;
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
  // 本轮跑完(或被停止)后,若该 session 不忙且有排队消息 → 把【整个队列】合并成一条
  // 一次性发出(Claude 式:排队的全部一起扔进下一轮,而不是一条条串行)。
  function flushQueued(sid) {
    if (isBusyFor(sid)) return;            // doFinal 等又起了新 turn → 留给那轮的 done 再 flush
    var q = sid === state.activeSessionId ? state.queued : (sessionStates[sid] && sessionStates[sid].queued);
    if (!q || q.length === 0) return;
    var items = q.splice(0, q.length);
    // 发给模型用 \n\n 分隔(让它清楚是几条独立消息);气泡显示用单换行 \n(紧凑,不空行)
    var text = items.map(function (i) { return i.text; }).filter(Boolean).join("\n\n");
    var displayText = items.map(function (i) { return i.displayText; }).filter(Boolean).join("\n");
    var attachments = [];
    items.forEach(function (i) { if (i.attachments && i.attachments.length) attachments = attachments.concat(i.attachments); });
    var meta = items.length === 1 ? items[0].meta : null; // 单条(如转交)保留 meta;合并多条不标
    var restrictTools = items.some(function (i) { return !!i.restrictTools; });
    notify();
    doSendFor(sid, text, displayText, attachments, meta, restrictTools);
  }

  async function sendMessageToSession(sessionId, text, meta) {
    var sid = String(sessionId || "").trim();
    var content = String(text || "").trim();
    if (!sid) throw new Error("目标会话不存在");
    if (!content) throw new Error("回复内容为空");
    var exists = state.sessions.some(function (session) { return String(session.id) === sid; });
    if (!exists) throw new Error("目标会话不存在");

    await ensureSessionBufferLoaded(sid);
    if (isBusyFor(sid)) {
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
      if (!state.activeSessionId) return;
    }
    // 定时任务引导:引导词只拼进发给模型的 payload,气泡/历史仍只显示用户输入。
    var payloadText = text;
    var restrictTools = false;
    if (state.scheduledTaskPendingGuide) {
      payloadText = state.scheduledTaskPendingGuide + "\n\n" + text;
      restrictTools = true;
      state.scheduledTaskPendingGuide = null;
      state.scheduledTaskCreationSessionId = state.activeSessionId;
      state.scheduledTaskDraft = null;
      notify();
    }

    // 新一轮用户消息 → 先熄灭技能标；本轮若模型再 load_skill 会重新点亮（sticky-ish 生命周期）。
    state.activeSkill = null;

    // 展示文本：把附件 chip 名附在用户消息末尾
    var displayText = readyAttachments.length > 0
      ? text + (text ? "\n\n" : "") + "📎 " + readyAttachments.map(function (a) { return a.basename; }).join(" · ")
      : text;
    var attachmentsPayload = readyAttachments.map(function (a) { return a.result; });
    clearAttachments();

    // 排队式:当前 session 正在生成 → 这句进队列(不打断当前轮),本轮 chat:done 后自动发。
    // 输入框上方显示待发 chip(可✕撤销)。停止按钮仍只硬打断当前轮。
    if (state.busy) {
      state.queued.push({ id: ++context.itemIdSeq, text: payloadText, displayText: displayText, attachments: attachmentsPayload, meta: meta || null, restrictTools: restrictTools });
      notify();
      return;
    }

    await doSendFor(state.activeSessionId, payloadText, displayText, attachmentsPayload, meta, restrictTools);
  }
  function prefillComposer(text) {
    state.composerPrefill = { id: (state.composerPrefill.id || 0) + 1, text: String(text || "") };
    notify();
  }
  // 撤销一条待发消息(点 chip 的 ✕)。
  function removeQueued(id) {
    state.queued = state.queued.filter(function (q) { return q.id !== id; });
    notify();
  }

  // ── Pinvou v4 召唤式检阅:Boss 主动呼叫,审当前 session 前面的工作 ──
  // 设计 docs/品悟v4-常驻检阅助手设计.md。纯召唤、不替 Boss 决策。
  // 审查卡进 chatItems(当前会话可见);跨会话持久化(进 messages/独立存储)是 §6 后续增强。
  async function summonPinvou(focus, mode) {
    if (!state.activeSessionId) { addSystemItem("先开始一个对话,再召唤 Pinvou 检阅。"); return; }
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
      if (meta && (meta.title === "新对话" || meta.title === "New chat" || personaPlaceholderTitles[state.activeSessionId])) {
        var firstUser = state.messages.find(function (m) { return m.role === "user"; });
        var text = firstUser && firstUser.content && firstUser.content.find(function (c) { return c.type === "text"; });
        if (text && text.text) {
          var newTitle = text.text.slice(0, 20);
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
      isDuplicateArtifactCard: isDuplicateArtifactCard,
      addSystemItem: addSystemItem,
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
    };
  };
})();
