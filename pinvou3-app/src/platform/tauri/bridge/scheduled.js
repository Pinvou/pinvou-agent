/** Scheduled-task state and Tauri command adapters. */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.scheduled = function (context) {let pinvouSharedtauriScheduledCache = null;
function pinvouSharedtauriScheduled() {
  if (!pinvouSharedtauriScheduledCache) pinvouSharedtauriScheduledCache = window.PinvouBridgeShared.create("tauriScheduled", { SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY, scheduledTaskTemplateSources: { get value() { return scheduledTaskTemplateSources; } }, persistScheduledTaskTemplateSources, state, scheduledTaskRequestTokens: { get value() { return scheduledTaskRequestTokens; } }, scheduledTaskRefreshInFlight: { get value() { return scheduledTaskRefreshInFlight; }, set value(v) { scheduledTaskRefreshInFlight = v; } }, scheduledRecentRunsRequestToken: { get value() { return scheduledRecentRunsRequestToken; }, set value(v) { scheduledRecentRunsRequestToken = v; } }, scheduledRunEventRefreshTimer: { get value() { return scheduledRunEventRefreshTimer; }, set value(v) { scheduledRunEventRefreshTimer = v; } }, notify, scheduledTaskPendingLoads: { get value() { return scheduledTaskPendingLoads; } }, scheduledTaskSelectionGeneration: { get value() { return scheduledTaskSelectionGeneration; }, set value(v) { scheduledTaskSelectionGeneration = v; } }, bt, createScheduledTask, invoke, rememberScheduledRunOwner, scheduledRunShortcutRefreshes: { get value() { return scheduledRunShortcutRefreshes; } }, SCHEDULED_LINK_POLL_DEADLINE_MS: { get value() { return SCHEDULED_LINK_POLL_DEADLINE_MS; } }, SCHEDULED_LINK_POLL_FAST_ATTEMPTS: { get value() { return SCHEDULED_LINK_POLL_FAST_ATTEMPTS; } }, SCHEDULED_LINK_POLL_FAST_MS: { get value() { return SCHEDULED_LINK_POLL_FAST_MS; } }, SCHEDULED_LINK_POLL_SLOW_MS: { get value() { return SCHEDULED_LINK_POLL_SLOW_MS; } }, isScheduledRunTerminal, scheduledTaskBackendInput, forgetScheduledTaskTemplateSource, scheduledTaskAutoCreateSeq: { get value() { return scheduledTaskAutoCreateSeq; }, set value(v) { scheduledTaskAutoCreateSeq = v; } }, createNewSession, prefillComposer });
  return pinvouSharedtauriScheduledCache;
}


    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const runSyncOnSession = context.runSyncOnSession;
    const addSystemItem = context.addSystemItem;
    const rememberScheduledRunOwner = context.rememberScheduledRunOwner;
    const isScheduledRunTerminal = context.isScheduledRunTerminal;
    const createNewSession = context.createNewSession;
    const prefillComposer = context.prefillComposer;
    const sessionStates = context.sessionStates;
    const SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY = "pinvou3-scheduled-task-template-sources-v1";
    const scheduledTaskTemplateSources = loadScheduledTaskTemplateSources();
    let scheduledTaskSelectionGeneration = 0;
    const scheduledTaskRequestTokens = { tasks: 0, detail: 0, runs: 0 };
    let scheduledTaskRefreshInFlight = null;
    let scheduledRecentRunsRequestToken = 0;
    let scheduledRunEventRefreshTimer = null;
    const scheduledTaskPendingLoads = Object.create(null);
    const scheduledTaskAutoCreateInFlight = Object.create(null);
function loadScheduledTaskTemplateSources() { return pinvouSharedtauriScheduled().loadScheduledTaskTemplateSources(); }

  function persistScheduledTaskTemplateSources() {
    try {
      window.localStorage.setItem(
        SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY,
        JSON.stringify(scheduledTaskTemplateSources)
      );
    } catch { /* ignore when localStorage is unavailable */ }
  }

function rememberScheduledTaskTemplateSource(taskId, templateId) { return pinvouSharedtauriScheduled().rememberScheduledTaskTemplateSource(taskId, templateId); }

  function forgetScheduledTaskTemplateSource(taskId) {
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor; Object.hasOwn is unavailable, and this call is already the safe form
    if (!taskId || !Object.prototype.hasOwnProperty.call(scheduledTaskTemplateSources, taskId)) return;
    delete scheduledTaskTemplateSources[taskId];
    persistScheduledTaskTemplateSources();
  }

function attachScheduledTaskTemplateSource(task) { return pinvouSharedtauriScheduled().attachScheduledTaskTemplateSource(task); }

function attachAndPruneScheduledTaskTemplateSources(tasks) { return pinvouSharedtauriScheduled().attachAndPruneScheduledTaskTemplateSources(tasks); }

function upsertScheduledTask(task) { return pinvouSharedtauriScheduled().upsertScheduledTask(task); }

function applyScheduledRunViewed(automationId, runId, receipt) { return pinvouSharedtauriScheduled().applyScheduledRunViewed(automationId, runId, receipt); }

function invalidateScheduledTaskReads(automationId) { return pinvouSharedtauriScheduled().invalidateScheduledTaskReads(automationId); }

function invalidateScheduledRecentRuns() { return pinvouSharedtauriScheduled().invalidateScheduledRecentRuns(); }

function invalidateScheduledRecentRunsForSession(id) { return pinvouSharedtauriScheduled().invalidateScheduledRecentRunsForSession(id); }

function scheduleScheduledRunRefresh() { return pinvouSharedtauriScheduled().scheduleScheduledRunRefresh(); }

function scheduledTaskErrorText(error) { return pinvouSharedtauriScheduled().scheduledTaskErrorText(error); }

function setScheduledTaskError(error, kind) { return pinvouSharedtauriScheduled().setScheduledTaskError(error, kind); }

function dismissScheduledTaskError() { return pinvouSharedtauriScheduled().dismissScheduledTaskError(); }

function clearScheduledTaskLoadError() { return pinvouSharedtauriScheduled().clearScheduledTaskLoadError(); }

function beginScheduledTaskLoad(stamp) { return pinvouSharedtauriScheduled().beginScheduledTaskLoad(stamp); }

function endScheduledTaskLoad(stamp) { return pinvouSharedtauriScheduled().endScheduledTaskLoad(stamp); }

function scheduledTaskRequestStamp(kind, id) { return pinvouSharedtauriScheduled().scheduledTaskRequestStamp(kind, id); }

function isCurrentScheduledTaskRequest(stamp) { return pinvouSharedtauriScheduled().isCurrentScheduledTaskRequest(stamp); }

function selectScheduledTask(id) { return pinvouSharedtauriScheduled().selectScheduledTask(id); }

function clearScheduledTaskSelection() { return pinvouSharedtauriScheduled().clearScheduledTaskSelection(); }

function extractBalancedJsonObject(text) { return pinvouSharedtauriScheduled().extractBalancedJsonObject(text); }

  function parseLooseJsonObject(text) {
    try { return JSON.parse(text); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(String(text || "").replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    const balanced = extractBalancedJsonObject(String(text || ""));
    if (!balanced) return null;
    try { return JSON.parse(balanced); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(balanced.replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    return null;
  }

function normalizeScheduledTaskDraft(value) { return pinvouSharedtauriScheduled().normalizeScheduledTaskDraft(value); }

function activeScheduledTaskModelConfig() { return pinvouSharedtauriScheduled().activeScheduledTaskModelConfig(); }

function lockScheduledTaskDraftModel(draft) { return pinvouSharedtauriScheduled().lockScheduledTaskDraftModel(draft); }

  function parseScheduledTaskDraftFromText(text) {
    if (!text || !text.includes("{")) return null;
    let preferred = null;
    let fallback = null;
    const re = /```([^\n`]*)\n([\s\S]*?)```/g;
    let match;
    // biome-ignore lint/suspicious/noAssignInExpressions: the assignment doubles as the loop condition; refactoring would hurt readability
    while ((match = re.exec(text))) {
      const label = String(match[1] || "").trim().toLowerCase();
      const raw = String(match[2] || "").trim();
      if (!raw || raw.charAt(0) !== "{") continue;
      const candidate = normalizeScheduledTaskDraft(parseLooseJsonObject(raw));
      if (!candidate) continue;
      if (label === "scheduled-task-draft") return candidate;
      if ((label === "json" || !label) && !fallback) fallback = candidate;
      if (!preferred) preferred = candidate;
    }
    return fallback || preferred;
  }

function clearScheduledTaskDraft() { return pinvouSharedtauriScheduled().clearScheduledTaskDraft(); }

async function confirmScheduledTaskDraft(editedDraft) { return pinvouSharedtauriScheduled().confirmScheduledTaskDraft(editedDraft); }

function scheduledTaskInputFromDraft(draft) { return pinvouSharedtauriScheduled().scheduledTaskInputFromDraft(draft); }

  // 聊天创建拿到合法参数后立即落成任务。草稿不会进入可渲染 state，避免再出现一层确认卡。
  // autoOpenId 全局 last-writer：两会话并发创建时后完成者覆盖，且
  // startScheduledTaskChat 清空后陈旧 completion 会复活 auto-open（审计 f）。
  // 全局单调创建序号，仅最新意图可写。
  let scheduledTaskAutoCreateSeq = 0;
  function autoCreateScheduledTaskDraft(draft, creationSessionId) {
    if (!draft || !creationSessionId || scheduledTaskAutoCreateInFlight[creationSessionId]) return;
    const lockedDraft = lockScheduledTaskDraftModel(draft);
    state.scheduledTaskDraft = null;
    const creationSeq = ++scheduledTaskAutoCreateSeq;
    const creation = Promise.resolve()
      .then(function () {
        return createScheduledTask(scheduledTaskInputFromDraft(lockedDraft));
      })
      .then(function (created) {
        if (state.scheduledTaskCreationSessionId === creationSessionId) {
          state.scheduledTaskCreationSessionId = null;
        }
        const creationBuffer = sessionStates[creationSessionId];
        if (creationBuffer) creationBuffer.scheduledTaskDraft = null;
        if (created && created.id && creationSeq === scheduledTaskAutoCreateSeq) state.scheduledTaskAutoOpenId = created.id;
        notify();
        return created;
      })
      .catch(function (error) {
        // createScheduledTask 通常已记录错误；忙锁在进入 action 前抛出时在这里补记，且不产生未处理 Promise。
        if (!state.scheduledTaskError) setScheduledTaskError(error, "action");
        runSyncOnSession(creationSessionId, function () {
          addSystemItem(bt("scheduledCreateFailed") + scheduledTaskErrorText(error), {
            scheduledTaskCreationError: true,
          });
        });
        notify();
        return null;
      })
      .finally(function () {
        if (scheduledTaskAutoCreateInFlight[creationSessionId] === creation) {
          delete scheduledTaskAutoCreateInFlight[creationSessionId];
        }
      });
    scheduledTaskAutoCreateInFlight[creationSessionId] = creation;
  }

async function loadScheduledTasks() { return pinvouSharedtauriScheduled().loadScheduledTasks(); }

async function readScheduledTask(id) { return pinvouSharedtauriScheduled().readScheduledTask(id); }

  // 按 run.id upsert 单个任务的运行到侧边栏快捷列表。不裁剪条数(侧边栏显示所有
  // 现存定时运行,后端 retention 已按 automation 限制终态运行上限);传入窗口有限
  // (如任务详情页只拉了前 N 条)时不会误删其余任务或本任务的更早记录。
function mergeScheduledTaskRecentRuns(task, runs) { return pinvouSharedtauriScheduled().mergeScheduledTaskRecentRuns(task, runs); }

async function loadScheduledTaskRuns(id, limit) { return pinvouSharedtauriScheduled().loadScheduledTaskRuns(id, limit); }

  // 侧边栏"定时任务记录"一次读取所有保留的运行。后端只做一次 reconcile 和
  // Session 元数据扫描，避免任务数增长后形成 N 次命令调用与重复完整会话读取。
async function loadScheduledTaskRecentRuns() { return pinvouSharedtauriScheduled().loadScheduledTaskRecentRuns(); }

function refreshScheduledTaskData(limit) { return pinvouSharedtauriScheduled().refreshScheduledTaskData(limit); }

  const scheduledRunShortcutRefreshes = Object.create(null);
  const SCHEDULED_LINK_POLL_FAST_MS = 1000;
  const SCHEDULED_LINK_POLL_SLOW_MS = 5000;
  const SCHEDULED_LINK_POLL_FAST_ATTEMPTS = 15;
  // 兜底上限:只在 run 卡在 queued/running 且永不终态时才会走到,正常路径靠下面
  // 「拿到 sessionId」或「进入终态」提前收工。
  const SCHEDULED_LINK_POLL_DEADLINE_MS = 30 * 60 * 1000;

  // Fallback for run-now:正常路径由 sched-* 文件 watcher 推送刷新；但文件事件可能
  // 早于 ThreadCreated / ThreadLinked 被 run 记录吸收，或 watcher 本身不可用，因此
  // 仍定向轮询本次 run，直到拿到 sessionId 或进入终态。它独立于页面生命周期，
  // 用户立即切走也不会让侧边栏永远漏掉这条记录。
  //
  // 停止条件按 run 自身状态,不用固定次数:TaskManager 只有 1 个 worker,前一个任务
  // 正在跑 LLM turn 时,新 run 排队几分钟是常态,固定 20 次(20 秒)会提前放弃,
  // watcher 是主路径；这里保留较长窗口只为覆盖事件丢失和链接时序空窗。
function refreshScheduledRunShortcutUntilLinked(automationId, runId) { return pinvouSharedtauriScheduled().refreshScheduledRunShortcutUntilLinked(automationId, runId); }

function upsertScheduledTaskRun(run) { return pinvouSharedtauriScheduled().upsertScheduledTaskRun(run); }

async function runScheduledTaskAction(action, operation) { return pinvouSharedtauriScheduled().runScheduledTaskAction(action, operation); }

  const SCHEDULED_TASK_WRITABLE_FIELDS = ["name", "prompt", "rrule", "model", "modelId", "paused"];

  // Scheduled tasks always run as Yolo. Keep the wire boundary intentionally narrow so
  // legacy callers cannot reintroduce task-level permissions or external directories.
  function scheduledTaskBackendInput(input) {
    const source = input || {};
    const backendInput = { mode: "yolo" };
    SCHEDULED_TASK_WRITABLE_FIELDS.forEach(function (field) {
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor; Object.hasOwn is unavailable, and this call is already the safe form
      if (Object.prototype.hasOwnProperty.call(source, field)) backendInput[field] = source[field];
    });
    return backendInput;
  }

  async function createScheduledTask(input) {
    return runScheduledTaskAction("create", async function () {
      const templateId = input && typeof input.templateId === "string" ? input.templateId.trim() : "";
      // kind is create-time task metadata (currently only "memory_organize"; the
      // backend rejects anything else). It deliberately stays out of
      // SCHEDULED_TASK_WRITABLE_FIELDS so edit flows can never resend it.
      const kind = input && typeof input.kind === "string" ? input.kind.trim() : "";
      const selectAfterCreate = !input || input.selectAfterCreate !== false;
      const backendInput = scheduledTaskBackendInput(input);
      if (kind) backendInput.kind = kind;
      const created = await invoke("create_scheduled_task", { input: backendInput });
      if (!created || !created.id) {
        throw new Error(bt("scheduledCreateNoId"));
      }
      if (templateId) rememberScheduledTaskTemplateSource(created.id, templateId);
      attachScheduledTaskTemplateSource(created);
      // 立即重拉任务列表:新 stamp 会使创建前仍在途的 list_scheduled_tasks 响应失效,
      // 防止旧结果落地时把刚创建的任务从列表里覆盖掉。
      await loadScheduledTasks();
      upsertScheduledTask(created);
      if (selectAfterCreate) selectScheduledTask(created.id);
      if (selectAfterCreate) state.scheduledTaskDetail = created;
      notify();
      return created;
    });
  }

async function updateScheduledTask(id, input) { return pinvouSharedtauriScheduled().updateScheduledTask(id, input); }

async function pauseScheduledTask(id) { return pinvouSharedtauriScheduled().pauseScheduledTask(id); }

async function resumeScheduledTask(id) { return pinvouSharedtauriScheduled().resumeScheduledTask(id); }

async function toggleScheduledTaskPinned(id, pinned) { return pinvouSharedtauriScheduled().toggleScheduledTaskPinned(id, pinned); }

async function deleteScheduledTask(id) { return pinvouSharedtauriScheduled().deleteScheduledTask(id); }

async function runScheduledTaskNow(id) { return pinvouSharedtauriScheduled().runScheduledTaskNow(id); }

  // 不直接替用户发消息:引导词存为 pending,预填一句短话进输入框,由用户编辑后自己发送。
async function startScheduledTaskChat() { return pinvouSharedtauriScheduled().startScheduledTaskChat(); }


    return {
      loadScheduledTaskTemplateSources,
      persistScheduledTaskTemplateSources,
      rememberScheduledTaskTemplateSource,
      forgetScheduledTaskTemplateSource,
      attachScheduledTaskTemplateSource,
      attachAndPruneScheduledTaskTemplateSources,
      upsertScheduledTask,
      applyScheduledRunViewed,
      invalidateScheduledTaskReads,
      invalidateScheduledRecentRuns,
      invalidateScheduledRecentRunsForSession,
      scheduleScheduledRunRefresh,
      scheduledTaskErrorText,
      setScheduledTaskError,
      dismissScheduledTaskError,
      clearScheduledTaskLoadError,
      beginScheduledTaskLoad,
      endScheduledTaskLoad,
      scheduledTaskRequestStamp,
      isCurrentScheduledTaskRequest,
      selectScheduledTask,
      clearScheduledTaskSelection,
      extractBalancedJsonObject,
      parseLooseJsonObject,
      normalizeScheduledTaskDraft,
      activeScheduledTaskModelConfig,
      lockScheduledTaskDraftModel,
      parseScheduledTaskDraftFromText,
      clearScheduledTaskDraft,
      confirmScheduledTaskDraft,
      scheduledTaskInputFromDraft,
      autoCreateScheduledTaskDraft,
      loadScheduledTasks,
      readScheduledTask,
      mergeScheduledTaskRecentRuns,
      loadScheduledTaskRuns,
      loadScheduledTaskRecentRuns,
      refreshScheduledTaskData,
      refreshScheduledRunShortcutUntilLinked,
      upsertScheduledTaskRun,
      runScheduledTaskAction,
      scheduledTaskBackendInput,
      createScheduledTask,
      updateScheduledTask,
      pauseScheduledTask,
      resumeScheduledTask,
      toggleScheduledTaskPinned,
      deleteScheduledTask,
      runScheduledTaskNow,
      startScheduledTaskChat
    };
  };
})(window);
