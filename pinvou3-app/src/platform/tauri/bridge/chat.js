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
    const recordSteeredMessages = context.recordSteeredMessages || function () {};
    const reconcileRemoteTurn = context.reconcileRemoteTurn;
    const markRemoteTurn = context.markRemoteTurn;
    const adoptManagedAttachments = context.adoptManagedAttachments || function () { return Promise.resolve(); };
    const discardManagedAttachment = context.discardManagedAttachment || function () { return Promise.resolve(); };
    const isScheduledRunSession = context.isScheduledRunSession;
    const userMessageDisplayText = context.userMessageDisplayText;
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

  // Single observable, session-scoped path for restoring dropped/failed steer
  // text (self-review P0 + re-review #1) and send text abandoned by a session
  // switch mid-send (issue #406). A bare setComposerDraft is invisible
  // (the composer is React-local state that only re-reads the store on
  // [activeSessionId, draftEpoch]), so with two dropped chips the first text
  // went store-only and the second chip's prefill write-through destroyed it.
  // Active session: append at the store level with a "\n" separator (immune to
  // same-tick batching) and bump draftEpoch so the existing draft-restore
  // effect re-reads the accumulated draft; ChatView itself stays unchanged.
  // Background session: write the session buffer only — setComposerDraft
  // targets the active working set and would leak background text into the
  // active draft. Must be called outside runSyncOnSession(sid): inside it,
  // state.activeSessionId is temporarily sid even for background sessions.
  function restoreSteerText(sid, text) {
    const value = String(text || "");
    if (!sid || !value) return;
    if (sid === state.activeSessionId) {
      const current = String(state.composerDraft || "");
      setComposerDraft(current ? current + "\n" + value : value);
      state.draftEpoch = (state.draftEpoch || 0) + 1;
      notify();
      return;
    }
    const buffer = sessionStates[sid];
    if (!buffer) return;
    const current = String(buffer.composerDraft || "");
    buffer.composerDraft = current ? current + "\n" + value : value;
  }

  // Per-session in-flight interrupt flag: while an interrupt is in flight,
  // flushQueued must not race queued messages ahead — the chat:done handler
  // triggers flushQueued before the interrupt message's doSendFor, and without
  // this gate a queued message would reserve the turn first while the
  // interrupt itself hits session_turn_in_progress. Cleared once the interrupt
  // message is sent (or its failure path finishes); the remaining queue is
  // then served by the interrupt round's chat:done.
  const interruptInFlight = {};

  // Transport-layer timeout for steer_chat invokes. The Rust steer() awaits
  // a foundation mpsc send; if the engine task is stuck (alive but not
  // draining its channel) the invoke never settles — the composer is already
  // cleared and the chip has no steerId backfilled, so the queue would be
  // blocked by that hanging chip. 25s matches the waitForChatDone fallback: a
  // healthy engine enqueues synchronously, so 25s only catches a real wedge.
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
    // Interrupt in flight: queued messages yield — the interrupt message goes
    // first (otherwise flush would reserve the turn and the interrupt itself
    // would be lost to turn_in_progress).
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
    // The queue head is a steer chip already delivered to the engine (waiting
    // for chat:steer_committed → bubble / dropped → removal) → yield. It is
    // already queued engine-side; a duplicate doSendFor would send it twice.
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

  // Backfill once the steer invoke resolves with a steer_id: settle any
  // committed/dropped events that arrived early; if the chip was taken by
  // ×/zap before backfill (cancelled), fire the compensating withdrawal.
  function onSteerBackfill(queuedItem, sid, steerId) {
    // Legacy backends (no return value) leave the chip id-less; settlement
    // falls back to transcript_committed counting. New backends backfill the
    // id and immediately settle any committed/dropped that arrived early.
    queuedItem.steerId = steerId || null;
    if (!queuedItem.steerId) return;
    if (queuedItem.cancelled) {
      // ⚡-gated chips: the zap path withdraws itself with outcome gating once
      // the invoke settles (see runQueuedZap) — a fire-and-forget withdrawal
      // here would race it and turn the gated outcome into a false
      // not_pending (the second withdraw answers not_pending by idempotency).
      if (!queuedItem.zapGate) withdrawSteerChip(sid, queuedItem);
      return;
    }
    settlePendingSteerEvent(sid, queuedItem);
    // Chips not settled immediately by a stashed event get the settle
    // watchdog (engine reclaim / wedge fallback).
    if (findSteerChipIndex(sid, queuedItem.steerId) >= 0) {
      armSteerSettleWatchdog(sid, queuedItem);
    }
  }

  // Steer failure (invoke rejection / 25s timeout; session missing / engine
  // not started): never silently fall back to invoke("chat") — while busy it
  // would hit session_turn_in_progress and the text would evaporate without a
  // trace. Remove the chip, restore the text to the composer/draft and notify,
  // handing the decision back to the user.
  // Queue routing is per-sid: a steer can take up to 25s to settle and the
  // user may have switched sessions meanwhile, so state.queued may already
  // point at another session's working set — take the chip from the target
  // session's queue by reference (steeredQueueFor semantics), otherwise the
  // chip (steered:true, steerId:null) sticks at the background queue head,
  // flushQueued yields forever and that session's queued messages starve.
  function onSteerFailure(queuedItem, sid, steerPreparation, steerInputText, err) {
    console.warn("[pinvou3][chat-ui] steer failed, restoring draft", {
      sid, error: err && err.toString ? err.toString() : err,
    });
    const failureQueue = steeredQueueFor(sid);
    const failureIndex = failureQueue ? failureQueue.indexOf(queuedItem) : -1;
    // Snapshot restore is sid-guarded (do not clobber the new session's UI
    // turn state after a switch) — same policy as restoreUiTurnState inside
    // sendMessage, expanded here because the nested function is unreachable.
    const snap = steerPreparation.snapshot;
    if (snap && state.activeSessionId === sid) {
      state.scheduledTaskPendingGuide = snap.scheduledTaskPendingGuide;
      state.scheduledTaskCreationSessionId = snap.scheduledTaskCreationSessionId;
      state.scheduledTaskDraft = snap.scheduledTaskDraft;
      state.activeSkill = snap.activeSkill;
    }
    if (failureIndex >= 0 && state.activeSessionId === sid &&
        String(state.composerDraft || "").trim() === "") {
      // Refill only when this callback actually took over the chip and the
      // composer is empty: a chip taken by ×/⚡ is owned by that path, and
      // refilling here would resurrect abandoned text or duplicate a zap
      // resend already in flight. restoreSteerText makes the restore visible
      // (draftEpoch bump) without a prefill write-through.
      failureQueue.splice(failureIndex, 1);
      restoreSteerText(sid, steerInputText);
    } else if (failureIndex >= 0) {
      // Composer occupied or session switched away: degrade the chip in place
      // to a plain local queue entry (same semantics as the zap failure
      // degrade) — the text is kept and sent by flushQueued at turn end;
      // ×/zap remain available.
      queuedItem.steered = false;
      queuedItem.steerId = null;
    }
    // Route the notice by sid, and only when this callback still owns the
    // chip: a ×/⚡ takeover owns the recovery from here on (the chip is no
    // longer in the queue), so "your text was restored to the input" would be
    // false — the text was deliberately discarded (×) or is being re-sent by
    // the zap's own gated path.
    if (failureIndex >= 0) {
      runSyncOnSession(sid, function () {
        addSystemItem("⚠️ " + bt("steerFailed"));
      });
    }
    notify();
    // The turn may have ended inside the 25s wait window: its chat:done flush
    // was yielded by the then-steered queue head (flushQueued's q[0].steered
    // check) and nothing will retrigger after the chip is degraded/removed —
    // compensate with one flush when idle, or the chip hangs forever (sixth
    // review round).
    if (!isBusyFor(sid)) flushQueued(sid);
  }

  // Return protocol (issue #406 — ChatView clears the composer before the
  // await, so every non-dispatching exit must say so; resolving undefined
  // used to read as "accepted" and silently dropped the draft):
  // - true         dispatched: sent, steered (busy chip) or queued for delivery.
  // - "restored"   nothing dispatched, but the text is already back in the
  //                composer (bridge-side restore) — the caller must not
  //                restore again, it would duplicate the draft.
  // - false        nothing dispatched and the text was NOT restored
  //                (notice-only early returns) — the caller owns putting the
  //                draft back (handleSend's empty-vs-typed restore).
  // Main-path send failures still throw (surfaceFailure) and the caller
  // restores through its catch.
  async function sendMessage(text, meta) {
    text = (text || "").trim();
    const readyAttachments = state.attachments.filter(function (a) { return a.status === "ready" && a.result; });
    if (!text && readyAttachments.length === 0) return false;
    // 还有解析中的附件 → 等
    if (state.attachments.some(function (a) { return a.status === "parsing"; })) {
      addSystemItem(bt("attachStillParsing"));
      return false;
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
        // append=true: failure-recovery semantics — the user may have started
        // the next message during the await; replacing would clobber it.
        prefillComposer(text, true);
        // The prefill IS the restore; "restored" stops the caller from doing
        // it a second time (the prefill lands asynchronously and would then
        // append a duplicate).
        return "restored";
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
        // The user navigated away during the await: the text goes back to the
        // session it was typed in (buffer draft), never into the session now
        // on screen — "restored" keeps the caller from prefilling it there.
        restoreSteerText(sid, text);
        return "restored";
      }
      addSystemItem(bt("deviceUploadFailed") + String(error && error.message ? error.message : error));
      return false;
    }
    if (state.activeSessionId !== sid) {
      abandonPreparedAttachments();
      restoreSteerText(sid, text);
      return "restored";
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

    // Mid-turn inject (steer): sending text while busy → the chip enters the
    // queue overlay and steer_chat injects into the engine immediately; the
    // turn loop embeds it at the next step boundary. Chips settle by steer_id
    // via chat:steer_committed (→ bubble) / chat:steer_dropped (→ removal +
    // notice); × cancels through a real withdraw_steer (see removeQueued).
    // Attachments: the steer channel carries text only, so sends with
    // attachments fall back to plain local queuing (queuePrepared, delivered
    // by flushQueued after this turn's chat:done).
    // The busy branch is extracted into a nested function (closures reach the
    // local helpers like consumeUiTurnState) to keep sendMessage's cognitive
    // complexity under the lint threshold.
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
        // Clear the composer draft (mirrors sendMessage's success path).
        // setComposerDraft syncs the session buffer so switching away and back
        // does not resurrect the sent text; a background session's steer
        // (remote control etc.) must not clear the active session's composer.
        if (state.activeSessionId === sid) setComposerDraft("");
        // Show the queue chip immediately. steered=true marks it as delivered
        // to the engine: flushQueued skips it (prevents double sends). The
        // steerId is backfilled when the invoke resolves; events may arrive
        // before that, so unsettled events are stashed per session (see
        // pendingSteerEvents below).
        const queuedItem = {
          id: ++context.itemIdSeq,
          text: steerText,
          displayText: steerText,
          attachments: [],
          meta: null,
          // The steer channel has no restrictTools parameter (steer_chat
          // carries text only); hard-coding false is a deliberate trade-off:
          // restricted scenes like the scheduled-task guide degrade to a plain
          // injection while busy, without tool restrictions. The non-busy
          // queueing path still keeps prepared.restrictTools.
          restrictTools: false,
          queuedAt: Date.now(),
          steered: true,
          steerId: null,
          cancelled: false,
        };
        state.queued.push(queuedItem);
        notify();
        // Backfill/failure recovery are extracted into module-level functions
        // (explicit parameters), also keeping sendMessage's cognitive
        // complexity under the lint threshold. The settlement handle doubles
        // as the ⚡ gate (see runQueuedZap): a zap on a not-yet-backfilled
        // chip must await the engine-side fate before resending. The handle
        // never rejects and is bounded by steer()'s own 25s timeout. It lives
        // in a module-side table, NOT on the chip: queued chips are part of
        // the subscription-visible chat slice, whose snapshot validator only
        // accepts JSON-like values — a Promise on the chip made every
        // notify() throw ("Subscription state only supports arrays and plain
        // objects") and froze the whole streaming UI until the chip left the
        // queue at settlement.
        rememberSteerSettlement(sid, queuedItem.id, steer(sid, steerText).then(
          function (steerId) {
            onSteerBackfill(queuedItem, sid, steerId);
            return { ok: true, steerId: steerId == null ? null : String(steerId) };
          },
          function (err) {
            onSteerFailure(queuedItem, sid, steerPreparation, steerInputText, err);
            return {
              ok: false,
              timedOut: String(err && err.message) === "steer_chat timed out",
            };
          }
        ));
      }
    };

    if (isBusyFor(sid)) {
      busySteerBranch();
      return true;
    }
    // Legacy behavior: with state.queued non-empty, still go through
    // flushQueued (cross-session remote-control edge cases).
    if (state.queued.length > 0) {
      const queuedPreparation = consumeUiTurnState();
      queuePrepared(queuedPreparation);
      flushQueued(sid);
      return true;
    }
    if (activeTurnBuffer && activeTurnBuffer.remoteTurnActive &&
        !(await reconcileRemoteTurn(sid))) {
      if (state.activeSessionId !== sid) {
        abandonPreparedAttachments();
        restoreSteerText(sid, text);
        return "restored";
      }
      recordAuthoritySyncDiagnostic("remote_sync_blocked_action", Object.assign({
        operation: "send",
      }, authoritySyncBufferSnapshot(sid, activeTurnBuffer)));
      addAuthoritySyncNotice(bt("remoteTurnSyncing"));
      return false;
    }
    if (state.activeSessionId !== sid) {
      abandonPreparedAttachments();
      restoreSteerText(sid, text);
      return "restored";
    }
    if (isBusyFor(sid) || state.queued.length > 0) {
      const racedQueuePreparation = consumeUiTurnState();
      queuePrepared(racedQueuePreparation);
      if (!isBusyFor(sid)) flushQueued(sid);
      return true;
    }

    const preparation = consumeUiTurnState();
    // surfaceFailure=true: a main-path failure (reserve conflict / command
    // rejection) must reject up to ChatView — it has already cleared the
    // composer and owns restoring the text; swallowing the error in the
    // bridge would hollow out the "never silently lose a message" promise
    // (fifth review round follow-up). With surfaceFailure, doSendFor either
    // resolves(true) or throws — there is no false branch.
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
    return true;
  }
  // WebUI 草稿首条消息失败时的专用重试入口。桌面端没有远程草稿，
  // 保留同名空实现以维持跨宿主 Bridge API 的稳定形状。
  function retryFirstTurn() {}
  // prefillComposer: template/navigation entries REPLACE the composer draft
  // (restored semantics, re-review #4 — KnowledgeView/scheduled/markdown
  // preview entries are designed for whole-draft replacement). Failure
  // recovery passes append=true to APPEND with a "\n" separator so a draft
  // the user started typing during the await window is not clobbered.
  // ChatView consumes the flag through the prefillAppend prop.
  function prefillComposer(text, append) {
    state.composerPrefill = {
      id: (state.composerPrefill.id || 0) + 1,
      text: String(text || ""),
      append: !!append,
    };
    notify();
  }
  // Undo a pending message (chip ×). A plain queued chip (with attachments)
  // = local removal + discard attachments, zero engine calls; a steered chip
  // = optimistic removal + a real withdraw_steer (the withdrawal is confirmed
  // by chat:steer_dropped; if it lands too late, committed renders the
  // bubble — see the race notes on settleSteerCommitted).
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

  // Mid-turn inject channel (thin wrapper over the steer_chat command).
  // steer_id contract: steer_chat resolves with an opaque steer_id (the pool
  // stamps the engine generation onto the foundation's ordinal id, e.g.
  // "e173…-steer-3", so ids stay unambiguous across engine rebuilds) on
  // success and rejects when the session does not exist or the engine is not
  // running. Plain sends while busy go through here (see sendMessage);
  // remote control and other hosts may use it too.
  async function steer(sid, content) {
    safeConsoleInfo("[pinvou3][chat-ui] steer start", { sid, len: (content || "").length });
    // The invoke has no transport timeout: a stuck engine (alive, not
    // draining) would leave steer_chat unsettled forever and the chip
    // (steered:true, steerId:null) would block the queue head and flushQueued.
    // The timeout goes to the catch-side failure recovery (remove chip,
    // restore composer).
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
    // Review P1-2: a late invoke success after the timeout — the engine has
    // accepted the original message and may inject it at a later step
    // boundary, duplicating delivery against the user's resend of the
    // restored text. Every late success triggers a compensating withdrawal:
    // if not yet injected it will never inject (the foundation's withdrawn
    // marker is idempotent, exactly one SteerDropped); if already committed
    // the withdrawal is a no-op and the late committed event renders the
    // bubble through the withdrawn registration (same-text dedup with the
    // resend is already handled). Must be registered BEFORE the race — once
    // the race rejects on timeout this function has thrown and no later code
    // runs.
    invokePromise.then(function (lateId) {
      if (!timedOut || lateId == null) return;
      rememberWithdrawn(sid, String(lateId), String(content || ""));
      invoke("withdraw_steer", { sessionId: sid, steerId: String(lateId) })
        .catch(function () { /* engine absent = will never inject */ });
    }).catch(function () { /* the rejection is handled by the race/failure path below */ });
    try {
      steerId = await Promise.race([invokePromise, timeout]);
    } finally {
      clearTimeout(timeoutId);
    }
    invokePromise.catch(function () { /* swallow the late rejection after a timeout to avoid unhandledrejection */ });
    safeConsoleInfo("[pinvou3][chat-ui] steer accepted", { sid, steerId });
    return steerId == null ? null : String(steerId);
  }

  // Steer chip settle watchdog (engine reclaim / wedge fallback, fifth
  // review round): once the steerId is backfilled, an engine reclaim (30min
  // idle / model switch / multi-agent toggle — the forwarder aborts first and
  // the SteerDropped is lost) or a wedge inside the "after the last drain,
  // before finish" window means committed/dropped never arrive, and the
  // steered queue head makes flushQueued yield forever — the session can
  // never send again. When the watchdog fires it withdraws (25s transport
  // timeout, aligned with steer/cancel) and awaits the outcome:
  //   retired = the engine confirms the copy will never inject; degrade the
  //     chip to a plain queue entry consumed by flushQueued;
  //   not_pending = already committed with the event lost; a degrade+resend
  //     would duplicate delivery — remove the chip and restore the draft
  //     (sixth review round, aligned with the zap path's outcome semantics);
  //   timeout or Err (invoke rejection, typically a reclaimed engine) = the
  //     withdrawal state is unproven (the steer may already be committed
  //     while only the response path is wedged — JensenChen28 review #2 —
  //     and a reclaimed engine must not replay minutes-old background
  //     messages, self-review P1-3) — remove the chip and restore the text
  //     to the owning session; resending is the user's call.
  // 60s rather than a symmetric 25s: during long tool calls (minute-scale
  // shells) a steer legitimately waits for the next step boundary, and a
  // shorter watchdog would falsely degrade a healthy engine's steer into an
  // independent next-round send.
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
      // Withdraw with a bounded await (self-review P1-2): without the 25s race
      // a live-but-wedged engine leaves the chip `steered` forever, blocking
      // the session queue head. Outcome semantics (aligned with the ⚡ path):
      //   "retired" (resolved) = engine copy will never inject, degrade the
      //     chip so flushQueued delivers it as a plain message;
      //   "not_pending" = committed with the event lost; a resend would
      //     duplicate, so remove the chip and restore the text;
      //   timeout = the withdrawal state is UNPROVEN — the steer may already
      //     be committed while only the response path is wedged, so
      //     auto-resending could double-deliver (JensenChen28 review #2);
      //   Err (rejected) = the engine that accepted the steer is gone (pool
      //     lookup), not a wedged response path. A rejection is NOT proof of
      //     non-delivery — the engine may have committed the steer into the
      //     persisted transcript before dying — so neither path resends on
      //     Err any more: the ⚡ path maps it to "withdraw_unreachable" (no
      //     resend, reconcile watchdog), and this autonomous watchdog
      //     restores the text instead — an unattended degrade would make
      //     flushQueued auto-send a message whose commit fate is unknown,
      //     so the user keeps the call. Both outcomes remove the chip and
      //     restore the text; resending is the user's decision.
      // The late commit is still rendered through the withdrawn registration
      // (same-text dedup against a user resend is already handled).
      rememberWithdrawn(sid, steerId, item.text);
      const withdrawPromise = invoke("withdraw_steer", { sessionId: sid, steerId });
      withdrawPromise.catch(function () { /* late rejection after the race settled is expected */ });
      // Sentinels keep a rejected invoke and a transport timeout distinct from
      // junk resolves (harness/legacy backends may resolve null/undefined) —
      // only a real rejection means "engine not present", and only the timeout
      // means "withdrawal state unknown".
      const WITHDRAW_ERR = "engine_err";
      const WITHDRAW_TIMEOUT = "withdraw_timeout";
      const withdrawOutcome = new Promise(function (resolve) {
        const timerId = setTimeout(function () {
          resolve(WITHDRAW_TIMEOUT);
        }, STEER_INVOKE_TIMEOUT_MS);
        withdrawPromise.then(
          function (outcome) { clearTimeout(timerId); resolve(outcome); },
          function () { clearTimeout(timerId); resolve(WITHDRAW_ERR); }
        );
      });
      withdrawOutcome.then(function (outcome) {
        // The chip may have been taken by ×/zap while awaiting: that path owns
        // the text now; restoring/degrading here would resurrect abandoned
        // text (same takeover guard as onSteerFailure).
        if (!q.includes(item)) return;
        if ([WITHDRAW_TIMEOUT, WITHDRAW_ERR, "not_pending"].includes(outcome)) {
          q.splice(q.indexOf(item), 1);
          notify();
          runSyncOnSession(sid, function () {
            addSystemItem("⚠️ " + bt("steerFailed"));
          });
          restoreSteerText(sid, item.text);
          notify();
          return;
        }
        item.steered = false;
        item.steerId = null;
        notify();
        if (!isBusyFor(sid)) flushQueued(sid);
      });
    }, STEER_SETTLE_WATCHDOG_MS);
  }

  // Zap-path not_pending reconciliation watchdog (foundation contract:
  // NotPending is not proof of delivery — the host must wait for a
  // reconciling terminal event). If committed/dropped is lost to an engine
  // reclaim, restore the text to the composer with a notice after 60s instead
  // of treating an uncertain message as delivered. When the event arrives
  // normally, the settle function clears the watchdog (committed renders the
  // bubble / dropped is silent) and the watchdog later no-ops.
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
      if (text === undefined) return; // already reconciled (a settle consumed the registration)
      runSyncOnSession(sid, function () {
        addSystemItem("⚠️ " + bt("steerFailed"));
      });
      // Session-scoped restore (draftEpoch bump for the owning session when
      // active) instead of a global prefill that overwrites other restores.
      restoreSteerText(sid, text);
      notify();
    }, STEER_SETTLE_WATCHDOG_MS);
  }

  // Pending steer-event stash: chat:steer_committed / chat:steer_dropped can
  // arrive before invoke("steer_chat") resolves (an extremely fast engine
  // commits and the event is dispatched first); the chip has no backfilled
  // steerId yet and cannot match. Stash steer_id → outcome per session and
  // settle immediately when the chip's id is backfilled (sendMessage's
  // steer().then). If the user removes the chip via × before backfill, the
  // stash entry lingers — negligible volume, unique steer_ids, acceptable.
  // Steer settlement side table (sid → chipId → promise): the steer_chat
  // settlement doubles as the ⚡ gate but is NOT subscription state. The chat
  // slice's snapshot validator only accepts arrays/plain objects/JSON scalars,
  // so parking the promise on the queued chip made every notify() throw and
  // froze all streaming updates until the chip left the queue. Entries
  // self-delete once settled; the zap takes (and deletes) its entry before
  // awaiting — whoever holds the promise reference keeps it either way.
  const steerSettlements = {};
  function rememberSteerSettlement(sid, chipId, settlement) {
    let byId = steerSettlements[sid];
    if (!byId) {
      byId = Object.create(null);
      steerSettlements[sid] = byId;
    }
    byId[chipId] = settlement.finally(function () { delete byId[chipId]; });
  }
  function takeSteerSettlement(sid, chipId) {
    const byId = steerSettlements[sid];
    if (!byId) return null;
    const settlement = byId[chipId] || null;
    if (settlement) delete byId[chipId];
    return settlement;
  }

  const pendingSteerEvents = {};
  // Cap (sixth review round P2): remotely injected steer events may never
  // have a local chip to consume them; the stash must not grow unbounded —
  // evict the oldest by insertion order past the cap (steer_id string keys,
  // Objects preserve insertion order).
  const PENDING_STEER_EVENT_CAP = 64;
  function stashSteerEvent(sid, steerId, kind) {
    let byId = pendingSteerEvents[sid];
    if (!byId) {
      byId = Object.create(null);
      pendingSteerEvents[sid] = byId;
    }
    byId[steerId] = kind;
    const keys = Object.keys(byId);
    if (keys.length > PENDING_STEER_EVENT_CAP) delete byId[keys[0]];
  }
  function takeSteerEvent(sid, steerId) {
    const byId = pendingSteerEvents[sid];
    if (!byId || !byId[steerId]) return null;
    const kind = byId[steerId];
    delete byId[steerId];
    return kind;
  }
  // Steers we withdrew ourselves (chip optimistically removed): a later
  // dropped = withdrawal effective, stay silent (no duplicate notice); a
  // later committed = withdrawal too late (already injected), the engine wins
  // and the stashed text renders a bubble. Key = steer_id, value = message
  // text.
  const withdrawnSteers = {};
  // Committed steers whose persisted position has not been captured yet
  // (sid → [{ text }]). The settle event can arrive before the forwarder
  // finishes persisting the injection, so the capture retries on each
  // chat:transcript_committed (guaranteed to follow the persist).
  const pendingSteerPositions = {};
  // On session delete/eviction, clear that session's steer intermediates:
  // stashed events / withdrawn texts (they contain user message text that
  // must not outlive the deleted session) / the in-flight interrupt flag.
  // Engine-side leftovers are covered by the foundation's
  // SyncSession/Shutdown SteerDropped; the frontend need not wait for events.
  function purgeSteerState(sid) {
    delete pendingSteerEvents[sid];
    delete withdrawnSteers[sid];
    delete pendingSteerPositions[sid];
    delete interruptInFlight[sid];
    delete steerSettlements[sid];
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
    // biome-ignore lint/suspicious/noPrototypeBuiltins: repo pins ES2021; the prototype-safe hasOwnProperty.call is the idiom, Object.hasOwn is ES2022
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
  // A committed steer is persisted by the Rust forwarder as a plain display
  // copy (no <turn_meta> tail, same shape as admissions since the
  // persistence alignment). The reload projection can therefore no longer
  // recognize it from the transcript; record its position in the
  // steered-messages sidecar instead. The persisted position is learned
  // from a load_session snapshot: the steer is the latest same-text user
  // message, matched from the tail so older same-text history never
  // mispairs (same tail-alignment rule as the legacy fallback below).
  function noteSteerCommittedForPosition(sid, text) {
    text = String(text || "");
    if (!sid || !text) return;
    const pending = pendingSteerPositions[sid] || [];
    pending.push({ text });
    pendingSteerPositions[sid] = pending;
    captureSteerPositions(sid);
  }
  function captureSteerPositions(sid) {
    const pending = pendingSteerPositions[sid];
    if (!sid || !pending || !pending.length) return;
    const scanned = pending.length;
    invoke("load_session", { id: sid, setActive: false }).then(function (saved) {
      const messages = saved && Array.isArray(saved.messages) ? saved.messages : [];
      const used = Object.create(null);
      const found = [];
      const remaining = [];
      pending.slice(0, scanned).forEach(function (entry) {
        let index = -1;
        for (let i = messages.length - 1; i >= 0; i--) {
          if (used[i]) continue;
          const m = messages[i];
          if (!m || m.role !== "user") continue;
          const content = Array.isArray(m.content) ? m.content : [];
          if (userMessageDisplayText(content, true) === entry.text) { index = i; break; }
        }
        if (index < 0) {
          remaining.push(entry);
          return;
        }
        used[index] = true;
        found.push({ pos: index, text: entry.text });
      });
      // Entries appended while the snapshot was in flight live beyond
      // `scanned` in the same array; keep them alongside the unmatched.
      const current = pendingSteerPositions[sid] || [];
      pendingSteerPositions[sid] = [...remaining, ...current.slice(scanned)];
      if (found.length) recordSteeredMessages(sid, found);
    }).catch(function () { /* snapshot unavailable: retried on the next transcript_committed */ });
  }
  function findSteerChipIndex(sid, steerId) {
    const q = steeredQueueFor(sid);
    if (!q) return -1;
    for (let i = 0; i < q.length; i++) {
      if (q[i] && q[i].steered && q[i].steerId && q[i].steerId === steerId) return i;
    }
    return -1;
  }
  // Withdraw a steered chip: with an id → invoke withdraw_steer immediately
  // and register (an Err means the engine is absent — the message never
  // entered it, local removal suffices); without a backfilled id (invoke in
  // flight) → set the cancelled flag; the backfill callback in sendMessage
  // fires the withdrawal later.
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
      .catch(function () { /* engine absent = message never entered the engine, nothing to do */ });
  }
  // Immediate settlement when the event arrives before the chip's steerId
  // backfill.
  function settlePendingSteerEvent(sid, item) {
    if (!item || !item.steerId) return;
    const kind = takeSteerEvent(sid, item.steerId);
    if (!kind) return;
    // The chip may already be removed by the user's ×: drop the event result
    // if it is no longer queued.
    if (findSteerChipIndex(sid, item.steerId) < 0) return;
    if (kind === "committed") settleSteerCommitted(sid, item.steerId);
    else settleSteerDropped(sid, item.steerId);
  }
  // chat:steer_committed settlement: the chip becomes a user bubble. When
  // no chip is found: if we withdrew it ourselves (too late, already
  // injected) → the engine wins, render the bubble; otherwise stash and
  // settle after the id backfill via settlePendingSteerEvent.
  function settleSteerCommitted(sid, steerId) {
    clearSteerSettleWatchdog(sid, steerId);
    clearOutcomeReconcileWatchdog(sid, steerId);
    if (findSteerChipIndex(sid, steerId) < 0) {
      const withdrawnText = takeWithdrawn(sid, steerId);
      if (withdrawnText !== undefined) {
        runSyncOnSession(sid, function () {
          // The zap success path's doSendFor already rendered the same-text
          // bubble → skip to avoid duplication (tiny race window).
          let lastUser = null;
          for (let i = state.chatItems.length - 1; i >= 0; i--) {
            if (state.chatItems[i] && state.chatItems[i].type === "user") { lastUser = state.chatItems[i]; break; }
          }
          if (lastUser && lastUser.text === withdrawnText) return;
          addChatItem({ type: "user", text: withdrawnText, time: timeStr(), steeredMidTurn: true });
        });
        // A committed event means the engine injected the message regardless
        // of which path rendered the bubble: the persisted position must be
        // recorded either way (re-recording is idempotent per pos).
        noteSteerCommittedForPosition(sid, withdrawnText);
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
      // The message is already in the engine transcript; the bubble renders
      // the chip text and transcript_committed later syncs state.messages to
      // the authoritative version. steeredMidTurn tells the conversation
      // projection this is a mid-turn injection, not a turn admission — it
      // must not consume a timing record or carry a phantom lifecycle badge.
      state.chatItems = state.chatItems.filter(function (ci) {
        return !ci.turnErrorNotice && !ci.authoritySyncNotice;
      });
      addChatItem({ type: "user", text: item.text, time: timeStr(), steeredMidTurn: true });
      noteSteerCommittedForPosition(sid, item.text);
    });
    notify();
  }
  // chat:steer_dropped settlement: remove the chip + notice. Our own
  // withdrawals (chip optimistically removed) stay silent — no duplicate
  // notice.
  function settleSteerDropped(sid, steerId) {
    clearSteerSettleWatchdog(sid, steerId);
    // Probe the zap reconcile watchdog BEFORE clearing it: a dropped event
    // landing while it is armed is the authoritative "never delivered"
    // terminal for a ⚡ zap whose withdraw answered not_pending or timed out
    // (the foundation reports NotPending for ids that already settled, and
    // the async dropped event routinely lands after the invoke response).
    // The silent path below is only correct for a × removal, where silence
    // means "the user deleted the text"; for a zap it would dismantle both
    // safety nets and silently lose the message.
    const zapReconciling = !!(outcomeReconcileWatchdogs[sid] && outcomeReconcileWatchdogs[sid][steerId]);
    clearOutcomeReconcileWatchdog(sid, steerId);
    if (findSteerChipIndex(sid, steerId) < 0) {
      const withdrawnText = takeWithdrawn(sid, steerId);
      if (withdrawnText !== undefined) {
        if (zapReconciling) {
          // Same recovery as the reconcile watchdog expiry: the dropped
          // event just made the expiry unnecessary by proving
          // non-delivery.
          runSyncOnSession(sid, function () {
            addSystemItem("⚠️ " + bt("steerFailed"));
          });
          restoreSteerText(sid, withdrawnText);
          notify();
        }
        return;
      }
      stashSteerEvent(sid, steerId, "dropped");
      return;
    }
    let restoredText = null;
    runSyncOnSession(sid, function () {
      const q = steeredQueueFor(sid);
      const index = findSteerChipIndex(sid, steerId);
      if (!q || index < 0) return;
      const item = q[index];
      q.splice(index, 1);
      addSystemItem("⚠️ " + bt("steerDropped"));
      restoredText = item.text;
    });
    // Restore the text to the draft (sixth review round P2): every other
    // failure path (steer failure / watchdog / zap degrade) hands the text
    // back; an engine-side drop (including ⏹ stop clearing) must not be the
    // only branch that makes the user retype. Session-scoped restore (append
    // + draftEpoch), not a global prefill.
    if (restoredText !== null) restoreSteerText(sid, restoredText);
    notify();
  }
  // Mid-turn INTERRUPT: break the current AI step and start a new turn
  // immediately. Unlike steer (which waits for the next step boundary without
  // interrupting tool calls), interrupt cancels the current turn right away,
  // starts a new one, and the message goes through the chat command path.
  //
  // Event-driven synchronization: instead of polling state.busy we await the
  // chat:done event itself. state.busy is set to false synchronously inside
  // the chat:done handler, so the event firing means the turn lifecycle has
  // finished cancel + cleanup and a new turn can be reserved safely. A 25s
  // fallback timeout covers long tool chains (avoids a hanging cancel; 5s is
  // too short for long tool-chain teardown and would trip the failure
  // recovery path too early).
  //
  // Generation matching (P0-B): the chat:done payload carries the backend
  // turn identity (generation); only the target turn resolves the wait —
  // late terminal events from old turns or other turns' terminals cannot
  // unlock it early. Legacy backends (no generation field) degrade to the
  // old sid-only matching.
  // Return value: true = the target turn's terminal was actually observed
  // (chat:done / busy cleared); false = fallback timeout. Callers use this to
  // distinguish "safe to reserve" from "cancel unwind still in progress".
  // chat:done watcher (review P2): on interrupt, register and buffer BEFORE
  // cancel — the old implementation listened only after cancel returned, so
  // events landing in the "cancel returned → listener registered" gap were
  // missed and terminal=false caused a pointless 25s wait. The watcher
  // buffers every done for the session (with generation); waitFor checks the
  // buffer first, then waits for future events; cancel() unsubscribes and
  // clears. Without an event channel (tests / web mode) it returns null and
  // the caller falls back to waitForChatDone's busy polling.
  function createChatDoneWatcher(sid) {
    if (!TAURI || !TAURI.event || typeof TAURI.event.listen !== "function") return null;
    const buffered = [];
    const waiters = [];
    let unlisten = null;
    let closed = false;
    // Same policy as waitForChatDone: if either side lacks a generation
    // (legacy backend), degrade to sid-only matching.
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
      // The gap is only truly closed once the registration promise settles;
      // callers await this before cancel.
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
      // Subscribe to webview events directly via TAURI.event.listen; resolve
      // after matching the sid. Tauri 2's listen returns
      // Promise<UnlistenFn> and invokes the callback as events arrive.
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
            // The listen promise may resolve after the timeout/event:
            // unsubscribe immediately in that case, otherwise one listener
            // leaks each time (the resolved guard makes it harmless but they
            // accumulate).
            if (resolved) { try { un(); } catch { /* already unlistened */ } return; }
            unlisten = un;
          }).catch(function () {});
        }
      } else {
        // Fallback: poll busy (test environments or web mode).
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

  // Withdraw an engine-side steer with a bounded outcome await (25s transport
  // timeout, aligned with steer/cancel). The withdrawn registration happens
  // BEFORE the await so a late committed renders the bubble through it.
  // Outcomes (foundation contract / JensenChen28 review #2 / zhuowp re-review
  // P1-1):
  //   "retired" (resolve) = the engine confirms the copy is marked withdrawn
  //     and will never inject — the caller may safely resend;
  //   "not_pending" (resolve) = already committed/settled — never resend;
  //   "withdraw_timeout" = unresolved, proves nothing either way — the caller
  //     must not resend and defers to its reconcile path;
  //   "withdraw_unreachable" (invoke rejection, e.g. the engine was reclaimed)
  //     = the engine that accepted this steer is gone — it may have committed
  //     the steer into the persisted transcript before dying, so a rejection
  //     is NOT proof of non-delivery and must not be treated as "retired".
  //     No resend; defer to the reconcile path (late committed → bubble /
  //     dropped → silent / event lost → text restored after 60s).
  //   The one genuinely safe-to-resend rejection is the steer_chat invoke's
  //   own deterministic rejection (handled by the zap gate): that engine never
  //   accepted anything.
  async function withdrawSteerOutcome(sid, steerId, text) {
    clearSteerSettleWatchdog(sid, steerId);
    rememberWithdrawn(sid, steerId, text);
    const withdrawPromise = invoke("withdraw_steer", { sessionId: sid, steerId });
    withdrawPromise.catch(function () { /* late rejection after the race settled is expected */ });
    let withdrawTimerId = null;
    let withdrawTimedOut = false;
    const withdrawTimeout = new Promise(function (_, reject) {
      withdrawTimerId = setTimeout(function () {
        withdrawTimedOut = true;
        reject(new Error("withdraw_steer timed out"));
      }, STEER_INVOKE_TIMEOUT_MS);
    });
    try {
      return await Promise.race([withdrawPromise, withdrawTimeout]);
    } catch {
      return withdrawTimedOut ? "withdraw_timeout" : "withdraw_unreachable";
    } finally {
      clearTimeout(withdrawTimerId);
    }
  }

  async function interruptAndSend(sid, text, displayText, attachments, meta, restrictTools) {
    safeConsoleInfo("[pinvou3][chat-ui] interrupt-and-send start", { sid });
    interruptInFlight[sid] = true;
    try {
      // 1) Cancel the current turn. cancel_generation returns CancelOutcome
      //    { generation, terminal }: terminal=true (the claim path's terminal
      //    already confirmed by cancel itself / target turn already over /
      //    idle) → no event wait needed; false → wait for the chat:done
      //    carrying the target generation (event-driven).
      //    This closes two deterministic races: the claim path emits chat:done
      //    before cancel returns (a listener would always miss it), and a
      //    cancel no-op right after natural turn end produces no event — in
      //    both cases the frontend cannot converge by waiting for events and
      //    only the command's return value confirms the terminal.
      // Busy is judged per sid (the user may switch sessions during the
      // await; state.busy would be misleading).
      if (isBusyFor(sid)) {
        // Review P2: register and wait for the listener before cancel,
        // closing the "cancel returns → listen registers" gap where a lost
        // chat:done caused a pointless 25s wait; with a null watcher (no
        // event channel) fall back to busy polling.
        const watcher = createChatDoneWatcher(sid);
        if (watcher) await watcher.ready;
        let outcome = null;
        // The cancel invoke gets the same transport timeout as steer (sixth
        // review round P1): a wedged engine (turn_lock held, not draining)
        // never settles the invoke — this function's finally would never run,
        // interruptInFlight[sid] would never clear and flushQueued would
        // block forever; the zap path's chip is already out of the queue
        // before the call, so its recovery catch would never run either —
        // silent message loss. The 25s timeout fails explicitly and the
        // caller restores the chip/message; a late resolve is harmless (the
        // cancel eventually lands) and a late rejection is swallowed to avoid
        // unhandledrejection.
        const cancelPromise = invoke("cancel_generation", { sessionId: sid, keepInbox: true });
        let cancelTimeoutId = null;
        let cancelTimedOut = false;
        const cancelTimeout = new Promise(function (_, reject) {
          cancelTimeoutId = setTimeout(function () {
            cancelTimedOut = true;
            reject(new Error("cancel_generation timed out"));
          }, STEER_INVOKE_TIMEOUT_MS);
        });
        cancelPromise.catch(function () { /* swallow the late rejection after a timeout */ });
        try {
          // keepInbox=true (interrupt semantics): un-injected steers are kept
          // for the next turn and queued chips are not silently cancelled;
          // the stop button (cancelGeneration) omits the flag, and the
          // backend treats it as false — clearing un-injected steers and
          // emitting chat:steer_dropped.
          outcome = await Promise.race([cancelPromise, cancelTimeout]);
        } catch (e) {
          if (cancelTimedOut) {
            // The timeout throw propagates before the terminal/non-terminal
            // branches below, so it never reaches their watcher.cancel() —
            // release explicitly here to avoid a listener leak.
            if (watcher) watcher.cancel();
            throw e;
          }
          console.warn("[pinvou3][chat-ui] cancel failed before interrupt", e);
        } finally {
          clearTimeout(cancelTimeoutId);
        }
        const terminal = !!(outcome && outcome.terminal);
        const generation = outcome && outcome.generation;
        if (terminal) {
          if (watcher) watcher.cancel();
        } else {
          // Event-driven wait (P0-B): the backend guarantees the reserve gate
          // is reopened by the time chat:done arrives — no fixed sleep is
          // needed to cover the window. The timeout is only a last resort.
          const observed = watcher
            ? await watcher.waitFor(generation, 25000)
            : await waitForChatDone(sid, generation, 25000);
          if (watcher) watcher.cancel();
          if (!observed && isBusyFor(sid)) {
            // 25s fallback timeout with the session still busy: the cancel
            // unwind is unfinished and doSendFor would reliably hit
            // session_turn_in_progress. bridge.chat.interruptAndSend is a
            // public API (remote control / other hosts) whose callers may not
            // handle rejections — fail explicitly without sending; the caller
            // restore the message (the UI's zap/chip paths all have recovery
            // catches). Never silently drop a message. If the session is no
            // longer busy (the listener missed the event but the turn
            // actually ended), proceed with the send.
            throw new Error(
              "interrupt-and-send: cancel did not reach terminal within 25s and session is still busy; message not sent"
            );
          }
        }
      }
      // 2) Do not clear the queue wholesale: interrupting only abandons the
      //    current turn's progress and keeps the user's other queued messages
      //    (remotely injected steers are kept for the next turn by the
      //    engine-side keepInbox semantics).
      // 3) Attachments are passed along by the caller (a queued chip's
      //    attachments are already in payload form).
      const attachmentPayload = attachments || [];
      // 4) Actually send the new message; on failure the caller owns
      //    recovery (chip zap restores back into the queue overlay).
      const result = await doSendFor(sid, text, displayText, attachmentPayload, meta, restrictTools, true);
      return result;
    } finally {
      interruptInFlight[sid] = false;
    }
  }

  // Zap-send a queued chip: remove it from the queue (its attachments go
  // out with the message, no discard) and run the interruptAndSend cancel +
  // doSendFor chain. While busy this interrupts the current generation; when
  // not busy (queue leftover not yet consumed by flushQueued) it sends
  // directly without cancelling.
  // A steered chip first withdraws the engine-side copy and awaits an
  // explicit outcome (review P1-1): retired = the engine copy is marked
  // withdrawn and will never inject, safe to resend; not_pending = already
  // committed (injection done/in flight), must not resend — the bubble comes
  // from the late or stashed steer_committed (the engine wins; same-text
  // dedup already handled).
  // On failure the message is restored to its original queue position (not
  // the composer) with a notice — the restored chip is degraded to
  // non-steered: the withdrawal was already sent and the engine only settles
  // it at the next drain; keeping steered would make flushQueued yield and
  // the next round would never come (queue deadlock). Engine-side leftovers
  // are bubbled by a late steer_committed, dedup already handled.
  // ⚡ entry, per-sid single-flight (concurrency P2 → entry guard): two
  // overlapping zaps let the second cancel_generation claim the FIRST zap's
  // freshly started turn (the cancel target is snapshotted at command
  // execution) and let flushQueued slip a queued chip between the two
  // interrupts. Reject the second zap with an explicit notice BEFORE it
  // touches the queue; retrying is the user's call. The try/finally brackets
  // the whole zap — including the withdraw-outcome window that precedes the
  // interruptAndSend call, which previously ran unlocked. interruptAndSend
  // keeps its own flag section (it is also a public API for remote hosts,
  // which are expected to serialize their own calls).

  // Settle the fate of a zapped chip whose withdraw outcome forbids resend:
  // route by the stashed terminal event first (a committed renders the
  // bubble; a dropped is the authoritative non-delivery terminal → immediate
  // restore), otherwise defer to the outcome reconcile watchdog.
  function settleZapSkipResend(sid, item) {
    const stashedSteerKind = takeSteerEvent(sid, item.steerId);
    if (stashedSteerKind === "committed") {
      settleSteerCommitted(sid, item.steerId);
      return;
    }
    if (stashedSteerKind === "dropped") {
      // A stashed chat:steer_dropped IS the authoritative "never delivered"
      // terminal for this exact steer_id — the very proof the reconcile
      // watchdog would wait a full window for. Settle now with the same
      // recovery semantics as the watchdog expiry and settleSteerDropped's
      // zap-reconciling branch (failure notice + session-scoped restore);
      // the chip itself was already removed by the zap.
      const withdrawnText = takeWithdrawn(sid, item.steerId);
      runSyncOnSession(sid, function () {
        addSystemItem("⚠️ " + bt("steerFailed"));
      });
      restoreSteerText(sid, withdrawnText === undefined ? item.text : withdrawnText);
      notify();
      return;
    }
    armOutcomeReconcileWatchdog(sid, item);
  }

  async function interruptAndSendQueued(sid, queuedId) {
    if (interruptInFlight[sid]) {
      runSyncOnSession(sid, function () {
        addSystemItem("⚠️ " + bt("interruptBusy"));
      });
      notify();
      return false;
    }
    interruptInFlight[sid] = true;
    try {
      return await runQueuedZap(sid, queuedId);
    } finally {
      interruptInFlight[sid] = false;
    }
  }

  async function runQueuedZap(sid, queuedId) {
    const q = steeredQueueFor(sid);
    if (!q) return false;
    let index = -1;
    for (let i = 0; i < q.length; i++) {
      if (q[i] && q[i].id === queuedId) { index = i; break; }
    }
    if (index < 0) return false; // chip already gone (double click / cancelled) — naturally single-flight
    const item = q[index];
    q.splice(index, 1);
    let skipResend = false;
    let steerSettlement = null;
    // Only an explicit foundation "retired" (or a deterministic rejection of
    // the steer_chat invoke itself, which never accepted anything) proves
    // resend safety; not_pending / transport timeout / an unreachable engine
    // are all uncertain and defer to the reconcile path (zhuowp re-review
    // P1-1: a reclaimed engine may have committed the steer first).
    const withdrawOutcomeForbidsResend = function (outcome) {
      return ["not_pending", "withdraw_timeout", "withdraw_unreachable"].includes(outcome);
    };
    if (item.steered && item.steerId && sid) {
      const outcome = await withdrawSteerOutcome(sid, item.steerId, item.text);
      skipResend = withdrawOutcomeForbidsResend(outcome);
    } else if (item.steered && sid) {
      // steerId not backfilled (steer_chat invoke in flight): the engine may
      // already hold the steer, and a copy parked by this zap's own keepInbox
      // cancel would commit at the new turn's first drain — resending before
      // the engine-side fate is known can deliver the same message twice
      // (display dedup does not cover model input). Gate the resend on the
      // invoke's settlement (bounded by steer()'s own 25s timeout; normally
      // milliseconds):
      //   resolves with an id → withdraw with outcome gating, same rules as
      //     the backfilled branch above;
      //   deterministic rejection of the steer_chat invoke itself (engine/
      //     session gone) → nothing entered the engine, resend normally (the
      //     one rejection that IS safe: no engine ever accepted the steer —
      //     unlike a withdraw_steer rejection, see withdrawSteerOutcome);
      //   transport timeout → delivery state unproven — never blindly send;
      //     restore the text to the composer (steer()'s late-success
      //     compensating withdrawal covers the registered-late case).
      withdrawSteerChip(sid, item); // marks cancelled for the backfill path
      item.zapGate = true; // backfill must not fire its own withdrawal (see onSteerBackfill)
      steerSettlement = takeSteerSettlement(sid, item.id);
    }
    notify();
    if (steerSettlement) {
      const settled = await steerSettlement;
      if (settled && settled.ok && settled.steerId && sid) {
        item.steerId = settled.steerId;
        const outcome = await withdrawSteerOutcome(sid, settled.steerId, item.text);
        skipResend = withdrawOutcomeForbidsResend(outcome);
      } else if (settled && !settled.ok && settled.timedOut) {
        runSyncOnSession(sid, function () {
          addSystemItem("⚠️ " + bt("steerFailed"));
        });
        restoreSteerText(sid, item.text);
        notify();
        return true;
      }
      // settled.ok && !settled.steerId (legacy backend, no engine-side
      // identity): nothing withdrawable — fall through and resend; display
      // settlement relies on the transcript-counting fallback.
    }
    if (skipResend) {
      // Already injected (not_pending), withdrawal unresolved (timeout), or
      // the engine that accepted the steer is gone (unreachable) — do not
      // resend in any of them. A committed stashed before the backfill
      // renders the bubble immediately; a late committed renders through the
      // withdrawn registration. None of these outcomes is proof of delivery
      // (foundation contract / JensenChen28 review #2 / zhuowp re-review
      // P1-1): wait for the reconciling event; if it is lost (engine
      // reclaim) the watchdog restores the text to the composer with a notice
      // instead of treating an uncertain message as delivered.
      settleZapSkipResend(sid, item);
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
      // The restored chip is degraded to non-steered (plain local queue
      // entry): the withdrawal was already sent and the engine only settles
      // it with a steer_dropped at the next drain; the chat send has failed,
      // so keeping the steered flag would make flushQueued yield to a steered
      // queue head whose next round never comes — queue deadlock. Degraded,
      // the chip is consumed normally by flushQueued; engine-side leftovers
      // (if the withdrawal lands too late) are bubbled by a late
      // steer_committed, dedup already handled.
      if (item.steered) {
        item.steered = false;
        item.steerId = null;
        item.cancelled = false;
      }
      const retryQueue = steeredQueueFor(sid);
      if (retryQueue) retryQueue.splice(Math.min(index, retryQueue.length), 0, item);
      runSyncOnSession(sid, function () {
        addSystemItem("⚠️ " + bt("interruptQueuedFailed"));
      });
      notify();
      // If the turn ended inside the wait window, its chat:done flush
      // already yielded — compensate once to prevent a hanging chip. The
      // notice is unconditional: the compensating flush retries
      // asynchronously and its outcome is unknown now — if the retry also
      // fails, flush's internal catch silently re-queues the chip, and a
      // conditional notice would leave the user with zero feedback.
      if (!isBusyFor(sid)) flushQueued(sid);
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
      captureSteerPositions,
      purgeSteerState,
    };
  };
})();
