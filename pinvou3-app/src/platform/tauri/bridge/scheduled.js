/** Scheduled-task state and Tauri command adapters. */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.scheduled = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const bt = context.bt;
    const runSyncOnSession = context.runSyncOnSession;
    const addSystemItem = context.addSystemItem;
    const rememberScheduledRunOwner = context.rememberScheduledRunOwner;
    const isScheduledRunTerminal = context.isScheduledRunTerminal;
    const purgeSessionBuffer = context.purgeSessionBuffer;
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
  function loadScheduledTaskTemplateSources() {
    try {
      const parsed = JSON.parse(window.localStorage.getItem(SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY) || "{}");
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return Object.create(null);
      return Object.keys(parsed).reduce(function (result, taskId) {
        if (typeof parsed[taskId] === "string" && parsed[taskId].trim()) {
          result[taskId] = parsed[taskId].trim();
        }
        return result;
      }, Object.create(null));
    } catch {
      return Object.create(null);
    }
  }

  function persistScheduledTaskTemplateSources() {
    try {
      window.localStorage.setItem(
        SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY,
        JSON.stringify(scheduledTaskTemplateSources)
      );
    } catch { /* ignore when localStorage is unavailable */ }
  }

  function rememberScheduledTaskTemplateSource(taskId, templateId) {
    if (!taskId || !templateId) return;
    scheduledTaskTemplateSources[taskId] = templateId;
    persistScheduledTaskTemplateSources();
  }

  function forgetScheduledTaskTemplateSource(taskId) {
    // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor; Object.hasOwn is unavailable, and this call is already the safe form
    if (!taskId || !Object.prototype.hasOwnProperty.call(scheduledTaskTemplateSources, taskId)) return;
    delete scheduledTaskTemplateSources[taskId];
    persistScheduledTaskTemplateSources();
  }

  function attachScheduledTaskTemplateSource(task) {
    if (!task || !task.id) return task;
    const templateId = task.templateId || scheduledTaskTemplateSources[task.id] || null;
    if (templateId) {
      task.templateId = templateId;
      if (scheduledTaskTemplateSources[task.id] !== templateId) {
        rememberScheduledTaskTemplateSource(task.id, templateId);
      }
    }
    return task;
  }

  function attachAndPruneScheduledTaskTemplateSources(tasks) {
    const activeIds = Object.create(null);
    (tasks || []).forEach(function (task) {
      if (!task || !task.id) return;
      activeIds[task.id] = true;
      attachScheduledTaskTemplateSource(task);
    });
    let changed = false;
    Object.keys(scheduledTaskTemplateSources).forEach(function (taskId) {
      if (activeIds[taskId]) return;
      delete scheduledTaskTemplateSources[taskId];
      changed = true;
    });
    if (changed) persistScheduledTaskTemplateSources();
    return tasks;
  }

  function upsertScheduledTask(task) {
    if (!task || !task.id) return;
    attachScheduledTaskTemplateSource(task);
    let found = false;
    state.scheduledTasks = (state.scheduledTasks || []).map(function (item) {
      if (item.id !== task.id) return item;
      found = true;
      return task;
    });
    if (!found) state.scheduledTasks = [task, ...(state.scheduledTasks || [])];
  }

  function applyScheduledRunViewed(automationId, runId, receipt) {
    function markRunViewed(item) {
      const itemAutomationId = item.automationId || state.selectedScheduledTaskId;
      if (itemAutomationId !== automationId || item.id !== runId) return item;
      return Object.assign({}, item, { unread: false });
    }
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(markRunViewed);
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(markRunViewed);
    const hasUnreadRuns = receipt && typeof receipt.hasUnreadRuns === "boolean"
      ? receipt.hasUnreadRuns
      : (state.scheduledTaskRuns || []).some(function (item) {
          return (item.automationId || state.selectedScheduledTaskId) === automationId && !!item.unread;
        });
    state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
      return task.id === automationId
        ? Object.assign({}, task, { hasUnreadRuns })
        : task;
    });
    if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === automationId) {
      state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, {
        hasUnreadRuns,
      });
    }
  }

  function invalidateScheduledTaskReads(automationId) {
    scheduledTaskRequestTokens.tasks += 1;
    if (state.selectedScheduledTaskId === automationId) {
      scheduledTaskRequestTokens.detail += 1;
      scheduledTaskRequestTokens.runs += 1;
    }
    scheduledTaskRefreshInFlight = null;
  }

  function invalidateScheduledRecentRuns() {
    scheduledRecentRunsRequestToken += 1;
  }

  function invalidateScheduledRecentRunsForSession(id) {
    if (String(id || "").indexOf("sched-") === 0) invalidateScheduledRecentRuns();
  }

  function scheduleScheduledRunRefresh() {
    if (scheduledRunEventRefreshTimer) clearTimeout(scheduledRunEventRefreshTimer);
    scheduledRunEventRefreshTimer = setTimeout(function () {
      scheduledRunEventRefreshTimer = null;
      // Refresh task badges/detail first, then replace the global run list from
      // the same retained backend state. The aggregate request has its own stale
      // response guard, so a concurrent archive/delete cannot resurrect a row.
      Promise.resolve(refreshScheduledTaskData(20))
        .catch(function () {})
        .then(function () { return loadScheduledTaskRecentRuns(); })
        .catch(function () {});
    }, 400);
  }

  function scheduledTaskErrorText(error) {
    return String(error && error.message ? error.message : error);
  }

  function setScheduledTaskError(error, kind) {
    state.scheduledTaskError = error ? scheduledTaskErrorText(error) : null;
    state.scheduledTaskErrorKind = error ? (kind || "load") : null;
  }

  function dismissScheduledTaskError() {
    setScheduledTaskError(null);
    notify();
  }

  function clearScheduledTaskLoadError() {
    if (state.scheduledTaskErrorKind === "load") setScheduledTaskError(null);
  }

  function beginScheduledTaskLoad(stamp) {
    const generation = stamp.generation;
    scheduledTaskPendingLoads[generation] = (scheduledTaskPendingLoads[generation] || 0) + 1;
    if (generation === scheduledTaskSelectionGeneration) {
      state.scheduledTaskLoading = true;
      clearScheduledTaskLoadError();
      notify();
    }
  }

  function endScheduledTaskLoad(stamp) {
    const generation = stamp.generation;
    scheduledTaskPendingLoads[generation] = Math.max(0, (scheduledTaskPendingLoads[generation] || 0) - 1);
    if (!scheduledTaskPendingLoads[generation]) delete scheduledTaskPendingLoads[generation];
    if (generation === scheduledTaskSelectionGeneration) {
      state.scheduledTaskLoading = !!scheduledTaskPendingLoads[generation];
      notify();
    }
  }

  function scheduledTaskRequestStamp(kind, id) {
    scheduledTaskRequestTokens[kind] += 1;
    return {
      kind,
      token: scheduledTaskRequestTokens[kind],
      generation: scheduledTaskSelectionGeneration,
      id: id || null,
    };
  }

  function isCurrentScheduledTaskRequest(stamp) {
    if (!stamp || stamp.generation !== scheduledTaskSelectionGeneration) return false;
    if (scheduledTaskRequestTokens[stamp.kind] !== stamp.token) return false;
    // id 检查省略：selectedScheduledTaskId 唯一写者是 selectScheduledTask（每次
    // 改写前 generation+1），id 变化必然被上方 generation 检查拦截（审计清理）。
    return true;
  }

  // eslint-disable-next-line sonarjs/no-invariant-returns -- echoing the normalized id is an intentional API contract
  function selectScheduledTask(id) {
    const nextId = typeof id === "string" && id.trim() ? id.trim() : null;
    if (state.selectedScheduledTaskId === nextId) return nextId;
    scheduledTaskSelectionGeneration += 1;
    state.scheduledTaskSelectionGeneration = scheduledTaskSelectionGeneration;
    state.selectedScheduledTaskId = nextId;
    state.scheduledTaskDetail = null;
    state.scheduledTaskRuns = [];
    state.scheduledTaskLoading = !!scheduledTaskPendingLoads[scheduledTaskSelectionGeneration];
    setScheduledTaskError(null);
    notify();
    return nextId;
  }

  function clearScheduledTaskSelection() {
    selectScheduledTask(null);
  }

  function extractBalancedJsonObject(text) {
    const start = String(text || "").indexOf("{");
    if (start < 0) return null;
    let depth = 0;
    let inString = false;
    let escaping = false;
    for (let i = start; i < text.length; i++) {
      const ch = text.charAt(i);
      if (inString) {
        if (escaping) escaping = false;
        else if (ch === "\\") escaping = true;
        else if (ch === "\"") inString = false;
        continue;
      }
      if (ch === "\"") { inString = true; continue; }
      if (ch === "{") depth++;
      else if (ch === "}") {
        depth--;
        if (depth === 0) return text.slice(start, i + 1);
      }
    }
    return null;
  }

  function parseLooseJsonObject(text) {
    try { return JSON.parse(text); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(String(text || "").replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    const balanced = extractBalancedJsonObject(String(text || ""));
    if (!balanced) return null;
    try { return JSON.parse(balanced); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    try { return JSON.parse(balanced.replaceAll(/,(\s*[}\]])/g, "$1")); } catch { /* invalid JSON: the caller falls back to the raw text */ }
    return null;
  }

  function normalizeScheduledTaskDraft(value) {
    if (!value || typeof value !== "object") return null;
    if (!value.name || !value.prompt || !value.rrule) return null;
    return {
      name: String(value.name),
      prompt: String(value.prompt),
      rrule: String(value.rrule),
      model: value.model ? String(value.model) : null,
      modelId: value.modelId ? String(value.modelId) : (value.model_id ? String(value.model_id) : null),
      mode: "yolo",
      paused: !!value.paused,
    };
  }

  function activeScheduledTaskModelConfig() {
    return (state.savedModels || []).find(function (model) {
      return model && model.id === state.activeModelId;
    }) || null;
  }

  function activeScheduledTaskModel() {
    const model = activeScheduledTaskModelConfig();
    return model && model.model || null;
  }

  function lockScheduledTaskDraftModel(draft) {
    if (!draft) return null;
    const active = activeScheduledTaskModelConfig();
    draft.model = draft.model || (active && active.model) || null;
    draft.modelId = draft.modelId || (active && active.id) || null;
    return draft;
  }

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

  function clearScheduledTaskDraft() {
    state.scheduledTaskDraft = null;
    if (state.activeSessionId === state.scheduledTaskCreationSessionId) {
      state.scheduledTaskCreationSessionId = null;
    }
    notify();
  }

  async function confirmScheduledTaskDraft(editedDraft) {
    if (!state.scheduledTaskDraft || state.activeSessionId !== state.scheduledTaskCreationSessionId) return null;
    const active = activeScheduledTaskModelConfig();
    const lockedModel = state.scheduledTaskDraft.model || (active && active.model) || null;
    const lockedModelId = state.scheduledTaskDraft.modelId || (active && active.id) || null;
    const draft = normalizeScheduledTaskDraft(Object.assign({}, state.scheduledTaskDraft, editedDraft || {}, {
      model: lockedModel,
      modelId: lockedModelId,
    }));
    if (!draft) {
      const invalidDraftError = new Error(bt("scheduledDraftInvalid"));
      setScheduledTaskError(invalidDraftError, "action");
      notify();
      throw invalidDraftError;
    }
    const created = await createScheduledTask({
      name: draft.name,
      prompt: draft.prompt,
      rrule: draft.rrule,
      model: lockedModel,
      modelId: lockedModelId,
      mode: "yolo",
      paused: draft.paused,
    });
    state.scheduledTaskDraft = null;
    state.scheduledTaskCreationSessionId = null;
    notify();
    return created;
  }

  function scheduledTaskInputFromDraft(draft) {
    return {
      name: draft.name,
      prompt: draft.prompt,
      rrule: draft.rrule,
      model: draft.model || null,
      modelId: draft.modelId || null,
      mode: "yolo",
      paused: draft.paused,
    };
  }

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

  async function loadScheduledTasks() {
    const stamp = scheduledTaskRequestStamp("tasks", null);
    beginScheduledTaskLoad(stamp);
    try {
      const tasks = await invoke("list_scheduled_tasks");
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTasks;
      state.scheduledTasks = attachAndPruneScheduledTaskTemplateSources(
        Array.isArray(tasks) ? tasks : []
      );
      if (
        state.selectedScheduledTaskId &&
        (state.scheduledTasks || []).every(function (task) { return task.id !== state.selectedScheduledTaskId; })
      ) {
        selectScheduledTask(null);
      }
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTasks;
  }

  async function readScheduledTask(id) {
    if (!id) {
      clearScheduledTaskSelection();
      return null;
    }
    if (state.selectedScheduledTaskId !== id) selectScheduledTask(id);
    const stamp = scheduledTaskRequestStamp("detail", id);
    beginScheduledTaskLoad(stamp);
    try {
      const detail = await invoke("read_scheduled_task", { id });
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTaskDetail;
      state.scheduledTaskDetail = attachScheduledTaskTemplateSource(detail) || null;
      upsertScheduledTask(detail);
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTaskDetail;
  }

  // 按 run.id upsert 单个任务的运行到侧边栏快捷列表。不裁剪条数(侧边栏显示所有
  // 现存定时运行,后端 retention 已按 automation 限制终态运行上限);传入窗口有限
  // (如任务详情页只拉了前 N 条)时不会误删其余任务或本任务的更早记录。
  function mergeScheduledTaskRecentRuns(task, runs) {
    if (!task || !task.id) return state.scheduledTaskRecentRuns || [];
    invalidateScheduledRecentRuns();
    let rows = [...(state.scheduledTaskRecentRuns || [])];
    (Array.isArray(runs) ? runs : []).forEach(function (run) {
      if (!run) return;
      rememberScheduledRunOwner(run);
      const merged = Object.assign({}, run, {
        automationId: run.automationId || task.id,
        taskName: task.name || bt("scheduledTaskFallbackName"),
        taskModel: task.model || null,
      });
      const index = rows.findIndex(function (row) { return row && row.id === merged.id; });
      if (index >= 0) rows[index] = merged;
      else rows.push(merged);
    });
    rows = rows.filter(function (run) { return run && run.sessionId && !run.archived; });
    rows.sort(function (a, b) {
      return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
        new Date(a.scheduledFor || a.createdAt || 0).getTime();
    });
    state.scheduledTaskRecentRuns = rows;
    return state.scheduledTaskRecentRuns;
  }

  async function loadScheduledTaskRuns(id, limit) {
    if (!id) {
      clearScheduledTaskSelection();
      return [];
    }
    if (state.selectedScheduledTaskId !== id) selectScheduledTask(id);
    const stamp = scheduledTaskRequestStamp("runs", id);
    beginScheduledTaskLoad(stamp);
    try {
      const runs = await invoke("list_scheduled_task_runs", { id, limit });
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTaskRuns;
      state.scheduledTaskRuns = Array.isArray(runs) ? runs : [];
      state.scheduledTaskRuns.forEach(rememberScheduledRunOwner);
      mergeScheduledTaskRecentRuns(
        (state.scheduledTasks || []).find(function (task) { return task && task.id === id; }),
        state.scheduledTaskRuns
      );
    } catch (e) {
      if (isCurrentScheduledTaskRequest(stamp)) setScheduledTaskError(e, "load");
    } finally {
      endScheduledTaskLoad(stamp);
    }
    return state.scheduledTaskRuns;
  }

  // 侧边栏"定时任务记录"一次读取所有保留的运行。后端只做一次 reconcile 和
  // Session 元数据扫描，避免任务数增长后形成 N 次命令调用与重复完整会话读取。
  async function loadScheduledTaskRecentRuns() {
    const requestToken = ++scheduledRecentRunsRequestToken;
    try {
      const tasks = state.scheduledTasks && state.scheduledTasks.length
        ? state.scheduledTasks
        : await loadScheduledTasks();
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      const runs = await invoke("list_scheduled_runs");
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      const tasksById = Object.create(null);
      (tasks || []).forEach(function (task) {
        if (task && task.id) tasksById[task.id] = task;
      });
      const rows = (Array.isArray(runs) ? runs : []).map(function (run) {
        if (!run) return null;
        rememberScheduledRunOwner(run);
        const automationId = run.automationId || run.automation_id;
        const task = tasksById[automationId] || null;
        return Object.assign({}, run, {
          automationId,
          taskName: task && task.name || bt("scheduledTaskFallbackName"),
          taskModel: task && task.model || null,
        });
      }).filter(function (run) {
        return run && run.sessionId && !run.archived;
      });
      rows.sort(function (a, b) {
        return new Date(b.scheduledFor || b.createdAt || 0).getTime() -
          new Date(a.scheduledFor || a.createdAt || 0).getTime();
      });
      state.scheduledTaskRecentRuns = rows;
      notify();
      return state.scheduledTaskRecentRuns;
    } catch (e) {
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      console.warn("loadScheduledTaskRecentRuns failed", e);
      state.scheduledTaskRecentRuns = state.scheduledTaskRecentRuns || [];
      notify();
      return state.scheduledTaskRecentRuns;
    }
  }

  function refreshScheduledTaskData(limit) {
    const generation = scheduledTaskSelectionGeneration;
    if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.generation === generation) {
      return scheduledTaskRefreshInFlight.promise;
    }
    const selectedId = state.selectedScheduledTaskId;
    const requests = [loadScheduledTasks()];
    if (selectedId) {
      requests.push(readScheduledTask(selectedId));
      requests.push(loadScheduledTaskRuns(selectedId, limit || 20));
    }
    const promise = Promise.all(requests).finally(function () {
      if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.promise === promise) {
        scheduledTaskRefreshInFlight = null;
      }
    });
    scheduledTaskRefreshInFlight = { generation, promise };
    return promise;
  }

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
  function refreshScheduledRunShortcutUntilLinked(automationId, runId) {
    if (!automationId || !runId) return;
    const key = automationId + ":" + runId;
    if (scheduledRunShortcutRefreshes[key]) return;
    scheduledRunShortcutRefreshes[key] = true;
    const deadline = Date.now() + SCHEDULED_LINK_POLL_DEADLINE_MS;

    function stop() {
      delete scheduledRunShortcutRefreshes[key];
    }
    function again(attempt) {
      if (Date.now() >= deadline) {
        stop();
        return;
      }
      setTimeout(function () { poll(attempt + 1); }, attempt < SCHEDULED_LINK_POLL_FAST_ATTEMPTS
        ? SCHEDULED_LINK_POLL_FAST_MS
        : SCHEDULED_LINK_POLL_SLOW_MS);
    }

    function taskStillListed() {
      return (state.scheduledTasks || []).some(function (item) {
        return item && item.id === automationId;
      });
    }

    function poll(attempt) {
      invoke("list_scheduled_task_runs", { id: automationId }).then(function (runs) {
        // 任务已被删除时不再回填：陈旧轮询响应会把已删任务以 fallback 名
        // 复活回侧边栏（审计 R1）。任务不在列表即收工，不 merge、不续排。
        const task = (state.scheduledTasks || []).find(function (item) {
          return item && item.id === automationId;
        });
        if (!task) {
          stop();
          return;
        }
        mergeScheduledTaskRecentRuns(task, runs);
        notify();
        // 必须看原始响应:mergeScheduledTaskRecentRuns 会滤掉尚无 sessionId 的记录,
        // 从合并结果里读不到目标 run 的状态。
        const target = (Array.isArray(runs) ? runs : []).find(function (run) {
          return run && run.id === runId;
        });
        // 会话已挂上 → 记录已进侧边栏;run 已终态却仍无会话 → 会话没建起来,再等也不会有;
        // run 记录消失(被删或被 retention 清掉)→ 没有等待对象。三种情况都收工。
        if (!target || target.sessionId || isScheduledRunTerminal(target.status)) {
          stop();
          return;
        }
        again(attempt);
      }).catch(function () {
        // 已删任务的后端响应是 Err 而非空列表（get_automation 文件已移除）。
        // 任务不在列表即收工，否则会以 1s/5s 空转重试到 30 分钟兜底。
        if (!taskStillListed()) {
          stop();
          return;
        }
        again(attempt);
      });
    }

    poll(0);
  }

  function upsertScheduledTaskRun(run) {
    if (!run || !run.id) return;
    rememberScheduledRunOwner(run);
    if (state.selectedScheduledTaskId && run.automationId && state.selectedScheduledTaskId !== run.automationId) return;
    let found = false;
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(function (item) {
      if (item.id === run.id) {
        found = true;
        return run;
      }
      return item;
    });
    if (!found) state.scheduledTaskRuns = [run, ...(state.scheduledTaskRuns || [])];
  }

  async function runScheduledTaskAction(action, operation) {
    if (state.scheduledTaskBusyAction) {
      throw new Error(bt("scheduledActionBusy"));
    }
    state.scheduledTaskBusyAction = action;
    setScheduledTaskError(null);
    notify();
    try {
      return await operation();
    } catch (e) {
      setScheduledTaskError(e, "action");
      throw e;
    } finally {
      state.scheduledTaskBusyAction = null;
      notify();
    }
  }

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
      const selectAfterCreate = !input || input.selectAfterCreate !== false;
      const backendInput = scheduledTaskBackendInput(input);
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

  async function updateScheduledTask(id, input) {
    return runScheduledTaskAction("update", async function () {
      const backendInput = scheduledTaskBackendInput(input);
      const updated = await invoke("update_scheduled_task", { id, input: backendInput });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function pauseScheduledTask(id) {
    return runScheduledTaskAction("pause", async function () {
      const updated = await invoke("pause_scheduled_task", { id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function resumeScheduledTask(id) {
    return runScheduledTaskAction("resume", async function () {
      const updated = await invoke("resume_scheduled_task", { id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function toggleScheduledTaskPinned(id, pinned) {
    return runScheduledTaskAction(pinned ? "pin" : "unpin", async function () {
      const updated = await invoke("set_scheduled_task_pinned", { id, pinned: !!pinned });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function deleteScheduledTask(id) {
    return runScheduledTaskAction("delete", async function () {
      invalidateScheduledRecentRuns();
      const deleted = await invoke("delete_scheduled_task", { id });
      // 作废删除前在途的整表 list / detail / runs 读（与 run-now 同模式）：否则
      // 3 秒轮询的旧 list 响应落地时会把刚删的任务复活回侧边栏（含本 feature
      // run-now 轮询依赖的 taskStillListed 判断，幽灵窗口会击穿 R1 守卫）。
      invalidateScheduledTaskReads(id);
      const deletedSessionIds = deleted && Array.isArray(deleted.deletedSessionIds)
        ? deleted.deletedSessionIds
        : [];
      const deletedSessionSet = Object.create(null);
      deletedSessionIds.forEach(function (sessionId) {
        deletedSessionSet[sessionId] = true;
        purgeSessionBuffer(sessionId);
      });
      forgetScheduledTaskTemplateSource(id);
      state.scheduledTasks = (state.scheduledTasks || []).filter(function (task) { return task.id !== id; });
      state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).filter(function (run) {
        return run && run.automationId !== id && !deletedSessionSet[run.sessionId];
      });
      state.scheduledTaskRuns = (state.scheduledTaskRuns || []).filter(function (run) {
        return run && run.automationId !== id && !deletedSessionSet[run.sessionId];
      });
      if (state.selectedScheduledTaskId === id) selectScheduledTask(null);
      notify();
      return deleted;
    });
  }

  async function runScheduledTaskNow(id) {
    return runScheduledTaskAction("run-now", async function () {
      const run = await invoke("run_scheduled_task_now", { id });
      invalidateScheduledTaskReads(id);
      upsertScheduledTaskRun(run);
      const runStatus = String(run && run.status || "").toLowerCase();
      if (runStatus === "queued" || runStatus === "running") {
        state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
          return task.id === id ? Object.assign({}, task, { isRunning: true }) : task;
        });
        if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === id) {
          state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, { isRunning: true });
        }
      }
      notify();
      refreshScheduledRunShortcutUntilLinked(id, run && run.id);
      return run;
    });
  }

  // 不直接替用户发消息:引导词存为 pending,预填一句短话进输入框,由用户编辑后自己发送。
  async function startScheduledTaskChat() {
    return runScheduledTaskAction("chat-create", async function () {
      const prompt = await invoke("scheduled_task_chat_prompt");
      state.scheduledTaskDraft = null;
      state.scheduledTaskCreationSessionId = null;
      scheduledTaskAutoCreateSeq++; // 清空意图：作废在途 auto-create 的陈旧 completion（审计 f）
      state.scheduledTaskAutoOpenId = null;
      await createNewSession();
      state.scheduledTaskPendingGuide = prompt;
      prefillComposer(bt("scheduledChatPrefill"));
      notify();
      return prompt;
    });
  }


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
      activeScheduledTaskModel,
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
