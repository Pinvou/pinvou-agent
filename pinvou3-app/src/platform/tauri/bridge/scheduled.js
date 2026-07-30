/** Scheduled-task state and Tauri command adapters. */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry.scheduled = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var runSyncOnSession = context.runSyncOnSession;
    var addSystemItem = context.addSystemItem;
    var rememberScheduledRunOwner = context.rememberScheduledRunOwner;
    var isScheduledRunTerminal = context.isScheduledRunTerminal;
    var purgeSessionBuffer = context.purgeSessionBuffer;
    var createNewSession = context.createNewSession;
    var prefillComposer = context.prefillComposer;
    var sessionStates = context.sessionStates;
    var SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY = "pinvou3-scheduled-task-template-sources-v1";
    var scheduledTaskTemplateSources = loadScheduledTaskTemplateSources();
    var scheduledTaskSelectionGeneration = 0;
    var scheduledTaskRequestTokens = { tasks: 0, detail: 0, runs: 0 };
    var scheduledTaskRefreshInFlight = null;
    var scheduledRecentRunsRequestToken = 0;
    var scheduledRunEventRefreshTimer = null;
    var scheduledTaskPendingLoads = Object.create(null);
    var scheduledTaskAutoCreateInFlight = Object.create(null);
  function loadScheduledTaskTemplateSources() {
    try {
      var parsed = JSON.parse(window.localStorage.getItem(SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY) || "{}");
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return Object.create(null);
      return Object.keys(parsed).reduce(function (result, taskId) {
        if (typeof parsed[taskId] === "string" && parsed[taskId].trim()) {
          result[taskId] = parsed[taskId].trim();
        }
        return result;
      }, Object.create(null));
    } catch (_) {
      return Object.create(null);
    }
  }

  function persistScheduledTaskTemplateSources() {
    try {
      window.localStorage.setItem(
        SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY,
        JSON.stringify(scheduledTaskTemplateSources)
      );
    } catch (_) {}
  }

  function rememberScheduledTaskTemplateSource(taskId, templateId) {
    if (!taskId || !templateId) return;
    scheduledTaskTemplateSources[taskId] = templateId;
    persistScheduledTaskTemplateSources();
  }

  function forgetScheduledTaskTemplateSource(taskId) {
    if (!taskId || !Object.prototype.hasOwnProperty.call(scheduledTaskTemplateSources, taskId)) return;
    delete scheduledTaskTemplateSources[taskId];
    persistScheduledTaskTemplateSources();
  }

  function attachScheduledTaskTemplateSource(task) {
    if (!task || !task.id) return task;
    var templateId = task.templateId || scheduledTaskTemplateSources[task.id] || null;
    if (templateId) {
      task.templateId = templateId;
      if (scheduledTaskTemplateSources[task.id] !== templateId) {
        rememberScheduledTaskTemplateSource(task.id, templateId);
      }
    }
    return task;
  }

  function attachAndPruneScheduledTaskTemplateSources(tasks) {
    var activeIds = Object.create(null);
    (tasks || []).forEach(function (task) {
      if (!task || !task.id) return;
      activeIds[task.id] = true;
      attachScheduledTaskTemplateSource(task);
    });
    var changed = false;
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
    var found = false;
    state.scheduledTasks = (state.scheduledTasks || []).map(function (item) {
      if (item.id !== task.id) return item;
      found = true;
      return task;
    });
    if (!found) state.scheduledTasks = [task].concat(state.scheduledTasks || []);
  }

  function applyScheduledRunViewed(automationId, runId, receipt) {
    function markRunViewed(item) {
      var itemAutomationId = item.automationId || state.selectedScheduledTaskId;
      if (itemAutomationId !== automationId || item.id !== runId) return item;
      return Object.assign({}, item, { unread: false });
    }
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(markRunViewed);
    state.scheduledTaskRecentRuns = (state.scheduledTaskRecentRuns || []).map(markRunViewed);
    var hasUnreadRuns = receipt && typeof receipt.hasUnreadRuns === "boolean"
      ? receipt.hasUnreadRuns
      : (state.scheduledTaskRuns || []).some(function (item) {
          return (item.automationId || state.selectedScheduledTaskId) === automationId && !!item.unread;
        });
    state.scheduledTasks = (state.scheduledTasks || []).map(function (task) {
      return task.id === automationId
        ? Object.assign({}, task, { hasUnreadRuns: hasUnreadRuns })
        : task;
    });
    if (state.scheduledTaskDetail && state.scheduledTaskDetail.id === automationId) {
      state.scheduledTaskDetail = Object.assign({}, state.scheduledTaskDetail, {
        hasUnreadRuns: hasUnreadRuns,
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
    var generation = stamp.generation;
    scheduledTaskPendingLoads[generation] = (scheduledTaskPendingLoads[generation] || 0) + 1;
    if (generation === scheduledTaskSelectionGeneration) {
      state.scheduledTaskLoading = true;
      clearScheduledTaskLoadError();
      notify();
    }
  }

  function endScheduledTaskLoad(stamp) {
    var generation = stamp.generation;
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
      kind: kind,
      token: scheduledTaskRequestTokens[kind],
      generation: scheduledTaskSelectionGeneration,
      id: id || null,
    };
  }

  function isCurrentScheduledTaskRequest(stamp) {
    if (!stamp || stamp.generation !== scheduledTaskSelectionGeneration) return false;
    if (scheduledTaskRequestTokens[stamp.kind] !== stamp.token) return false;
    if (stamp.kind !== "tasks" && state.selectedScheduledTaskId !== stamp.id) return false;
    return true;
  }

  function selectScheduledTask(id) {
    var nextId = typeof id === "string" && id.trim() ? id.trim() : null;
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
    var start = String(text || "").indexOf("{");
    if (start < 0) return null;
    var depth = 0;
    var inString = false;
    var escaping = false;
    for (var i = start; i < text.length; i++) {
      var ch = text.charAt(i);
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
    try { return JSON.parse(text); } catch (_) {}
    try { return JSON.parse(String(text || "").replace(/,(\s*[}\]])/g, "$1")); } catch (_) {}
    var balanced = extractBalancedJsonObject(String(text || ""));
    if (!balanced) return null;
    try { return JSON.parse(balanced); } catch (_) {}
    try { return JSON.parse(balanced.replace(/,(\s*[}\]])/g, "$1")); } catch (_) {}
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
    var model = activeScheduledTaskModelConfig();
    return model && model.model || null;
  }

  function lockScheduledTaskDraftModel(draft) {
    if (!draft) return null;
    var active = activeScheduledTaskModelConfig();
    draft.model = draft.model || (active && active.model) || null;
    draft.modelId = draft.modelId || (active && active.id) || null;
    return draft;
  }

  function parseScheduledTaskDraftFromText(text) {
    if (!text || text.indexOf("{") < 0) return null;
    var preferred = null;
    var fallback = null;
    var re = /```([^\n`]*)\n([\s\S]*?)```/g;
    var match;
    while ((match = re.exec(text))) {
      var label = String(match[1] || "").trim().toLowerCase();
      var raw = String(match[2] || "").trim();
      if (!raw || raw.charAt(0) !== "{") continue;
      var candidate = normalizeScheduledTaskDraft(parseLooseJsonObject(raw));
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
    var active = activeScheduledTaskModelConfig();
    var lockedModel = state.scheduledTaskDraft.model || (active && active.model) || null;
    var lockedModelId = state.scheduledTaskDraft.modelId || (active && active.id) || null;
    var draft = normalizeScheduledTaskDraft(Object.assign({}, state.scheduledTaskDraft, editedDraft || {}, {
      model: lockedModel,
      modelId: lockedModelId,
    }));
    if (!draft) {
      var invalidDraftError = new Error(bt("scheduledDraftInvalid"));
      setScheduledTaskError(invalidDraftError, "action");
      notify();
      throw invalidDraftError;
    }
    var created = await createScheduledTask({
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
  function autoCreateScheduledTaskDraft(draft, creationSessionId) {
    if (!draft || !creationSessionId || scheduledTaskAutoCreateInFlight[creationSessionId]) return;
    var lockedDraft = lockScheduledTaskDraftModel(draft);
    state.scheduledTaskDraft = null;
    var creation = Promise.resolve()
      .then(function () {
        return createScheduledTask(scheduledTaskInputFromDraft(lockedDraft));
      })
      .then(function (created) {
        if (state.scheduledTaskCreationSessionId === creationSessionId) {
          state.scheduledTaskCreationSessionId = null;
        }
        var creationBuffer = sessionStates[creationSessionId];
        if (creationBuffer) creationBuffer.scheduledTaskDraft = null;
        if (created && created.id) state.scheduledTaskAutoOpenId = created.id;
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
    var stamp = scheduledTaskRequestStamp("tasks", null);
    beginScheduledTaskLoad(stamp);
    try {
      var tasks = await invoke("list_scheduled_tasks");
      if (!isCurrentScheduledTaskRequest(stamp)) return state.scheduledTasks;
      state.scheduledTasks = attachAndPruneScheduledTaskTemplateSources(
        Array.isArray(tasks) ? tasks : []
      );
      if (
        state.selectedScheduledTaskId &&
        !(state.scheduledTasks || []).some(function (task) { return task.id === state.selectedScheduledTaskId; })
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
    var stamp = scheduledTaskRequestStamp("detail", id);
    beginScheduledTaskLoad(stamp);
    try {
      var detail = await invoke("read_scheduled_task", { id: id });
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
    var rows = (state.scheduledTaskRecentRuns || []).slice();
    (Array.isArray(runs) ? runs : []).forEach(function (run) {
      if (!run) return;
      rememberScheduledRunOwner(run);
      var merged = Object.assign({}, run, {
        automationId: run.automationId || task.id,
        taskName: task.name || bt("scheduledTaskFallbackName"),
        taskModel: task.model || null,
      });
      var index = rows.findIndex(function (row) { return row && row.id === merged.id; });
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
    var stamp = scheduledTaskRequestStamp("runs", id);
    beginScheduledTaskLoad(stamp);
    try {
      var runs = await invoke("list_scheduled_task_runs", { id: id, limit: limit });
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
    var requestToken = ++scheduledRecentRunsRequestToken;
    try {
      var tasks = state.scheduledTasks && state.scheduledTasks.length
        ? state.scheduledTasks
        : await loadScheduledTasks();
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      var runs = await invoke("list_scheduled_runs");
      if (requestToken !== scheduledRecentRunsRequestToken) {
        return state.scheduledTaskRecentRuns || [];
      }
      var tasksById = Object.create(null);
      (tasks || []).forEach(function (task) {
        if (task && task.id) tasksById[task.id] = task;
      });
      var rows = (Array.isArray(runs) ? runs : []).map(function (run) {
        if (!run) return null;
        rememberScheduledRunOwner(run);
        var automationId = run.automationId || run.automation_id;
        var task = tasksById[automationId] || null;
        return Object.assign({}, run, {
          automationId: automationId,
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
    var generation = scheduledTaskSelectionGeneration;
    if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.generation === generation) {
      return scheduledTaskRefreshInFlight.promise;
    }
    var selectedId = state.selectedScheduledTaskId;
    var requests = [loadScheduledTasks()];
    if (selectedId) {
      requests.push(readScheduledTask(selectedId));
      requests.push(loadScheduledTaskRuns(selectedId, limit || 20));
    }
    var promise = Promise.all(requests).finally(function () {
      if (scheduledTaskRefreshInFlight && scheduledTaskRefreshInFlight.promise === promise) {
        scheduledTaskRefreshInFlight = null;
      }
    });
    scheduledTaskRefreshInFlight = { generation: generation, promise: promise };
    return promise;
  }

  var scheduledRunShortcutRefreshes = Object.create(null);
  var SCHEDULED_LINK_POLL_FAST_MS = 1000;
  var SCHEDULED_LINK_POLL_SLOW_MS = 5000;
  var SCHEDULED_LINK_POLL_FAST_ATTEMPTS = 15;
  // 兜底上限:只在 run 卡在 queued/running 且永不终态时才会走到,正常路径靠下面
  // 「拿到 sessionId」或「进入终态」提前收工。
  var SCHEDULED_LINK_POLL_DEADLINE_MS = 30 * 60 * 1000;

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
    var key = automationId + ":" + runId;
    if (scheduledRunShortcutRefreshes[key]) return;
    scheduledRunShortcutRefreshes[key] = true;
    var deadline = Date.now() + SCHEDULED_LINK_POLL_DEADLINE_MS;

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

    function poll(attempt) {
      invoke("list_scheduled_task_runs", { id: automationId }).then(function (runs) {
        var task = (state.scheduledTasks || []).find(function (item) {
          return item && item.id === automationId;
        }) || { id: automationId, name: bt("scheduledTaskFallbackName") };
        mergeScheduledTaskRecentRuns(task, runs);
        notify();
        // 必须看原始响应:mergeScheduledTaskRecentRuns 会滤掉尚无 sessionId 的记录,
        // 从合并结果里读不到目标 run 的状态。
        var target = (Array.isArray(runs) ? runs : []).find(function (run) {
          return run && run.id === runId;
        });
        // 会话已挂上 → 记录已进侧边栏;run 已终态却仍无会话 → 会话没建起来,再等也不会有;
        // run 记录消失(被删或被 retention 清掉)→ 没有等待对象。三种情况都收工。
        if (!target || target.sessionId || isScheduledRunTerminal(target.status)) {
          stop();
          return;
        }
        again(attempt);
      }).catch(function () { again(attempt); });
    }

    poll(0);
  }

  function upsertScheduledTaskRun(run) {
    if (!run || !run.id) return;
    rememberScheduledRunOwner(run);
    if (state.selectedScheduledTaskId && run.automationId && state.selectedScheduledTaskId !== run.automationId) return;
    var found = false;
    state.scheduledTaskRuns = (state.scheduledTaskRuns || []).map(function (item) {
      if (item.id === run.id) {
        found = true;
        return run;
      }
      return item;
    });
    if (!found) state.scheduledTaskRuns = [run].concat(state.scheduledTaskRuns || []);
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

  var SCHEDULED_TASK_WRITABLE_FIELDS = ["name", "prompt", "rrule", "model", "modelId", "paused"];

  // Scheduled tasks always run as Yolo. Keep the wire boundary intentionally narrow so
  // legacy callers cannot reintroduce task-level permissions or external directories.
  function scheduledTaskBackendInput(input) {
    var source = input || {};
    var backendInput = { mode: "yolo" };
    SCHEDULED_TASK_WRITABLE_FIELDS.forEach(function (field) {
      if (Object.prototype.hasOwnProperty.call(source, field)) backendInput[field] = source[field];
    });
    return backendInput;
  }

  async function createScheduledTask(input) {
    return runScheduledTaskAction("create", async function () {
      var templateId = input && typeof input.templateId === "string" ? input.templateId.trim() : "";
      var selectAfterCreate = !input || input.selectAfterCreate !== false;
      var backendInput = scheduledTaskBackendInput(input);
      var created = await invoke("create_scheduled_task", { input: backendInput });
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
      var backendInput = scheduledTaskBackendInput(input);
      var updated = await invoke("update_scheduled_task", { id: id, input: backendInput });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function pauseScheduledTask(id) {
    return runScheduledTaskAction("pause", async function () {
      var updated = await invoke("pause_scheduled_task", { id: id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function resumeScheduledTask(id) {
    return runScheduledTaskAction("resume", async function () {
      var updated = await invoke("resume_scheduled_task", { id: id });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function toggleScheduledTaskPinned(id, pinned) {
    return runScheduledTaskAction(pinned ? "pin" : "unpin", async function () {
      var updated = await invoke("set_scheduled_task_pinned", { id: id, pinned: !!pinned });
      upsertScheduledTask(updated);
      if (state.selectedScheduledTaskId === id) state.scheduledTaskDetail = updated;
      notify();
      return updated;
    });
  }

  async function deleteScheduledTask(id) {
    return runScheduledTaskAction("delete", async function () {
      invalidateScheduledRecentRuns();
      var deleted = await invoke("delete_scheduled_task", { id: id });
      var deletedSessionIds = deleted && Array.isArray(deleted.deletedSessionIds)
        ? deleted.deletedSessionIds
        : [];
      var deletedSessionSet = Object.create(null);
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
      var run = await invoke("run_scheduled_task_now", { id: id });
      invalidateScheduledTaskReads(id);
      upsertScheduledTaskRun(run);
      var runStatus = String(run && run.status || "").toLowerCase();
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
      var prompt = await invoke("scheduled_task_chat_prompt");
      state.scheduledTaskDraft = null;
      state.scheduledTaskCreationSessionId = null;
      state.scheduledTaskAutoOpenId = null;
      await createNewSession();
      state.scheduledTaskPendingGuide = prompt;
      prefillComposer(bt("scheduledChatPrefill"));
      notify();
      return prompt;
    });
  }


    return {
      loadScheduledTaskTemplateSources: loadScheduledTaskTemplateSources,
      persistScheduledTaskTemplateSources: persistScheduledTaskTemplateSources,
      rememberScheduledTaskTemplateSource: rememberScheduledTaskTemplateSource,
      forgetScheduledTaskTemplateSource: forgetScheduledTaskTemplateSource,
      attachScheduledTaskTemplateSource: attachScheduledTaskTemplateSource,
      attachAndPruneScheduledTaskTemplateSources: attachAndPruneScheduledTaskTemplateSources,
      upsertScheduledTask: upsertScheduledTask,
      applyScheduledRunViewed: applyScheduledRunViewed,
      invalidateScheduledTaskReads: invalidateScheduledTaskReads,
      invalidateScheduledRecentRuns: invalidateScheduledRecentRuns,
      invalidateScheduledRecentRunsForSession: invalidateScheduledRecentRunsForSession,
      scheduleScheduledRunRefresh: scheduleScheduledRunRefresh,
      scheduledTaskErrorText: scheduledTaskErrorText,
      setScheduledTaskError: setScheduledTaskError,
      dismissScheduledTaskError: dismissScheduledTaskError,
      clearScheduledTaskLoadError: clearScheduledTaskLoadError,
      beginScheduledTaskLoad: beginScheduledTaskLoad,
      endScheduledTaskLoad: endScheduledTaskLoad,
      scheduledTaskRequestStamp: scheduledTaskRequestStamp,
      isCurrentScheduledTaskRequest: isCurrentScheduledTaskRequest,
      selectScheduledTask: selectScheduledTask,
      clearScheduledTaskSelection: clearScheduledTaskSelection,
      extractBalancedJsonObject: extractBalancedJsonObject,
      parseLooseJsonObject: parseLooseJsonObject,
      normalizeScheduledTaskDraft: normalizeScheduledTaskDraft,
      activeScheduledTaskModelConfig: activeScheduledTaskModelConfig,
      activeScheduledTaskModel: activeScheduledTaskModel,
      lockScheduledTaskDraftModel: lockScheduledTaskDraftModel,
      parseScheduledTaskDraftFromText: parseScheduledTaskDraftFromText,
      clearScheduledTaskDraft: clearScheduledTaskDraft,
      confirmScheduledTaskDraft: confirmScheduledTaskDraft,
      scheduledTaskInputFromDraft: scheduledTaskInputFromDraft,
      autoCreateScheduledTaskDraft: autoCreateScheduledTaskDraft,
      loadScheduledTasks: loadScheduledTasks,
      readScheduledTask: readScheduledTask,
      mergeScheduledTaskRecentRuns: mergeScheduledTaskRecentRuns,
      loadScheduledTaskRuns: loadScheduledTaskRuns,
      loadScheduledTaskRecentRuns: loadScheduledTaskRecentRuns,
      refreshScheduledTaskData: refreshScheduledTaskData,
      refreshScheduledRunShortcutUntilLinked: refreshScheduledRunShortcutUntilLinked,
      upsertScheduledTaskRun: upsertScheduledTaskRun,
      runScheduledTaskAction: runScheduledTaskAction,
      scheduledTaskBackendInput: scheduledTaskBackendInput,
      createScheduledTask: createScheduledTask,
      updateScheduledTask: updateScheduledTask,
      pauseScheduledTask: pauseScheduledTask,
      resumeScheduledTask: resumeScheduledTask,
      toggleScheduledTaskPinned: toggleScheduledTaskPinned,
      deleteScheduledTask: deleteScheduledTask,
      runScheduledTaskNow: runScheduledTaskNow,
      startScheduledTaskChat: startScheduledTaskChat
    };
  };
})(window);
