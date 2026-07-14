#!/usr/bin/env node
/**
 * Static and behavioral regression checks for the scheduled-task frontend shell.
 *
 * Run: node pinvou3-app/tests/scheduled_tasks_unit.js
 */
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const indexHtml = [
  'main.jsx',
  'shared/i18n.js',
  'components/layout/NavigationComponents.jsx',
  'features/chat/ChatView.jsx',
  'features/scheduled/ScheduledTasksView.jsx'
].map(file => fs.readFileSync(path.join(__dirname, '..', 'src', file), 'utf8')).join('\n');
const tauriBridge = fs.readFileSync(path.join(__dirname, '..', 'src', 'tauri-bridge.js'), 'utf8');
const scheduledTasksRust = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'scheduled_tasks.rs'), 'utf8');
const enginePoolRust = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'engine_pool.rs'), 'utf8');
const scheduledTaskPromptRust = scheduledTasksRust.slice(
  scheduledTasksRust.indexOf('const SCHEDULED_TASK_CHAT_PROMPT'),
  scheduledTasksRust.indexOf('pub fn scheduled_automation_root')
);
const scheduledTemplateSource = indexHtml.slice(
  indexHtml.indexOf('const SCHEDULED_TASK_TEMPLATES'),
  indexHtml.indexOf('const ScheduledTasksView')
);

function mustContain(text) {
  assert.ok(
    indexHtml.includes(text) || tauriBridge.includes(text),
    `expected scheduled tasks sources to contain: ${text}`
  );
}

function mustNotContain(text) {
  assert.ok(
    !indexHtml.includes(text) && !tauriBridge.includes(text),
    `expected scheduled tasks sources to not contain: ${text}`
  );
}

assert.ok(
  /scheduledPlans:\s*'定时任务'/.test(indexHtml),
  'left sidebar label should be 定时任务'
);
assert.ok(
  /const SCHEDULED_TASKS_ENTRY_ENABLED = false/.test(indexHtml),
  'scheduled-task entry should remain disabled until the creation flow is fixed'
);
assert.ok(
  /SCHEDULED_TASKS_ENTRY_ENABLED\s*&&\s*\(\s*<NavItem[\s\S]{0,500}label=\{t\.scheduledPlans\}/.test(indexHtml),
  'the scheduled-task navigation item must be gated by the temporary feature flag'
);
assert.ok(
  /SCHEDULED_TASKS_ENTRY_ENABLED\s*&&\s*bs\.scheduledTaskAutoOpenId/.test(indexHtml),
  'automatic scheduled-task navigation must be gated with the visible entry'
);
assert.ok(
  /const ScheduledTasksView\s*=/.test(indexHtml),
  'ScheduledTasksView component should exist'
);
assert.ok(
  /currentView === 'scheduled'/.test(indexHtml),
  'App should render the scheduled view'
);
assert.ok(
  /currentView === 'scheduled'\s*&&\s*\([\s\S]{0,1200}scheduledRunContext[\s\S]{0,800}<ChatView[\s\S]{0,1600}<ScheduledTasksView/.test(indexHtml),
  'a scheduled run should reuse the full ChatView inside the scheduled route'
);
assert.ok(
  /data-current-view=\{currentView\}/.test(indexHtml),
  'the app root should expose the committed route for smoke tests'
);
assert.ok(
  /data-testid="scheduled-page"/.test(indexHtml),
  'scheduled page should expose a stable smoke-test hook'
);
assert.ok(
  /data-testid="scheduled-list-delete"/.test(indexHtml),
  'delete action should live in the scheduled task list'
);
assert.ok(
  /data-testid="scheduled-detail"/.test(indexHtml),
  'scheduled task details should be a secondary selected-task state'
);
assert.ok(
  /没有匹配的定时任务/.test(indexHtml),
  'scheduled tasks should render an empty state by default'
);
assert.ok(
  /const renderTaskRow[\s\S]*?return\s*\(\s*<div/.test(indexHtml),
  'scheduled task rows should render a non-button root container'
);
mustContain("loadScheduledTasks");
mustContain("readScheduledTask");
mustContain("startScheduledTaskChat");
mustContain("confirmScheduledTaskDraft");
mustContain("clearScheduledTaskDraft");
mustContain("scheduledTasks:");
mustContain("scheduledTaskDraft: null");
mustContain("scheduledTaskCreationSessionId: null");
mustContain("scheduledTaskPendingGuide: null");
mustContain("scheduledRunContext: null");
mustContain("selectedScheduledTaskId: null");
mustContain("openScheduledRunChat");
mustContain("exitScheduledRunChat");
mustContain("selectScheduledTask");
mustContain("refreshScheduledTaskData");
mustContain("navigateFromScheduledRun");
mustContain("invoke(\"list_scheduled_tasks\")");
mustContain("scheduled_task:run_updated");
assert.ok(
  /const selectedId = appState\.selectedScheduledTaskId \|\| null/.test(indexHtml),
  'scheduled selection must live above the remounted ScheduledTasksView'
);
assert.ok(
  /const refresh = \(\) => bridge\.refreshScheduledTaskData\(20\)[\s\S]{0,260}setInterval\([\s\S]{0,120}refresh\(\)[\s\S]{0,120}3000/.test(indexHtml),
  'the three-second fallback must refresh tasks, selected detail, and runs through one bridge transaction'
);
assert.ok(
  /async function handleSwitchSession\(id\)[\s\S]{0,260}await bridge\.switchToSession\(id\)[\s\S]{0,180}if \(!switched\) return;[\s\S]{0,180}setCurrentView\('chat'\)/.test(indexHtml),
  'ordinary session navigation must await a successful load before committing the chat route'
);
assert.ok(
  /async function navigateFromScheduledRun\(nextView[\s\S]{0,480}await bridge\.exitScheduledRunChat\(\)[\s\S]{0,160}if \(!exited\) return false;[\s\S]{0,200}setCurrentView\(nextView\)/.test(indexHtml),
  'leaving a scheduled run through other navigation must restore its return session first'
);
assert.ok(
  /onBackScheduledRun=\{\(\) => navigateFromScheduledRun\('scheduled'\)\}/.test(indexHtml),
  'scheduled back navigation must await restoration before committing the Scheduled route'
);
assert.ok(
  /async function startScheduledTaskChat\(\)\s*\{[\s\S]*?var prompt = await invoke\("scheduled_task_chat_prompt"\);[\s\S]*?await createNewSession\(\);[\s\S]*?state\.scheduledTaskPendingGuide = prompt;[\s\S]*?prefillComposer\(/s.test(tauriBridge),
  'startScheduledTaskChat should stash the guide and prefill the composer instead of auto-sending'
);
assert.ok(
  !/await sendMessage\(prompt/.test(tauriBridge),
  'startScheduledTaskChat must not auto-send the guide prompt as a chat message'
);
assert.ok(
  /payloadText = state\.scheduledTaskPendingGuide \+ "\\n\\n" \+ text/.test(tauriBridge),
  'the guide should only be prepended to the model payload, never to the displayed text'
);
assert.ok(
  /restrictTools = true/.test(tauriBridge) && /restrictTools: !!restrictTools/.test(tauriBridge),
  'scheduled-task creation chat should disable model tools while collecting the draft'
);
[
  "activeScheduledTaskRun",
  "completeScheduledTaskRun",
  "createScheduledTaskRunSession",
  "scheduled_task:dispatch",
  "complete_scheduled_task_run",
  "sourceSessionId",
  "collectTurnOutputPaths",
].forEach(function (obsolete) {
  assert.ok(!tauriBridge.includes(obsolete), `frontend bridge should no longer contain ${obsolete}`);
});
assert.ok(
  /model:\s*value\.model\s*\?\s*String\(value\.model\)\s*:\s*null/.test(tauriBridge),
  'normalized scheduled-task drafts should retain an explicit model wire name'
);
assert.ok(
  /function lockScheduledTaskDraftModel\(draft\)[\s\S]{0,180}draft\.model = draft\.model \|\| activeScheduledTaskModel\(\)/.test(tauriBridge) &&
    /var lockedModel = state\.scheduledTaskDraft\.model \|\| activeScheduledTaskModel\(\)/.test(tauriBridge),
  'the final draft should lock the active saved model wire name before display and confirmation'
);
assert.ok(
  /pub struct ScheduledRunDto[\s\S]*?pub session_id: Option<String>/.test(scheduledTasksRust),
  'scheduled run DTO should include the chat session for that run'
);
assert.ok(
  /pub struct ScheduledTaskDto[\s\S]*?pub has_unread_runs: bool/.test(scheduledTasksRust),
  'scheduled task DTO should aggregate unread completed run conversations'
);
assert.ok(
  /pub struct ScheduledTaskDto[\s\S]*?pub is_running: bool/.test(scheduledTasksRust),
  'scheduled task DTO should aggregate queued or running executions'
);
assert.ok(
  /pub struct ScheduledRunDto[\s\S]*?pub unread: bool/.test(scheduledTasksRust),
  'each scheduled run DTO should expose its own unread conversation state'
);
assert.ok(
  /MAX_SCHEDULED_RUN_SESSION_OWNERS\s*=\s*64/.test(tauriBridge) &&
    /function pruneScheduledRunSessionOwners\([\s\S]{0,1800}MAX_SCHEDULED_RUN_SESSION_OWNERS; i < ids\.length; i\+\+/.test(tauriBridge) &&
    /function scheduledRunOwnerPriority\([\s\S]{0,260}activeSessionId[\s\S]{0,260}scheduledRunContext[\s\S]{0,120}return 3/.test(tauriBridge),
  'scheduled run owner tombstones should have a fixed 64-entry LRU bound'
);
assert.ok(
  indexHtml.includes('data-testid="scheduled-task-unread"') && /task\.hasUnreadRuns/.test(indexHtml) &&
    indexHtml.includes('data-testid="scheduled-run-unread"') && /item\.unread/.test(indexHtml),
  'blue dots should represent unread run conversations at task and run level'
);
assert.ok(
  indexHtml.includes('data-testid="scheduled-nav-unread"') &&
    /unread=\{!!\(bs && \(bs\.scheduledTasks \|\| \[\]\)\.some\(task => task\.hasUnreadRuns\)\)\}/.test(indexHtml),
  'the Scheduled sidebar item should aggregate unread completed runs across tasks'
);
assert.ok(
  /scheduled_task:run_updated[\s\S]{0,520}state\.selectedScheduledTaskId === automationId[\s\S]{0,180}refreshScheduledTaskData\(20\)[\s\S]{0,180}loadScheduledTasks\(\)/.test(tauriBridge) &&
    /async function init\(\)[\s\S]{0,420}loadScheduledTasks\(\)\.catch/.test(tauriBridge),
  'task summaries should refresh globally on startup and on run updates outside the Scheduled page'
);
assert.ok(
  /if \(automationId && runId && runStatus === "completed"\)/.test(tauriBridge) &&
    /fn ensure_scheduled_run_can_be_marked_viewed[\s\S]{0,420}AutomationRunStatus::Completed/.test(scheduledTasksRust),
  'only opening a completed run conversation may persist its viewed state'
);
assert.ok(
  /SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION:\s*u32\s*=\s*2/.test(scheduledTasksRust) &&
    /registry\.schema_version < SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION[\s\S]{0,220}ScheduledRunReadRegistry::default\(\)/.test(scheduledTasksRust),
  'legacy read receipts must be reset because they may have been written before completion'
);
assert.ok(
  indexHtml.includes('data-testid="scheduled-task-running"') && /task\.isRunning/.test(indexHtml) &&
    indexHtml.includes('data-testid="scheduled-run-running"') && /queued|running/.test(indexHtml),
  'spinners should appear only on running task and run-history rows'
);
assert.ok(
  /data-testid="scheduled-task-summary"/.test(indexHtml) &&
    /nextRunAt/.test(indexHtml) && /下次/.test(indexHtml) && /秒后/.test(indexHtml) &&
    /setInterval\([\s\S]{0,160}1000/.test(indexHtml),
  'active task rows should show the exact next run and a live seconds countdown'
);
assert.ok(
  /function scheduleRepeatLabel\(/.test(indexHtml) &&
    /editor\.interval/.test(indexHtml) &&
    /scheduleEditor\.repeat !== 'hourly'[\s\S]{0,120}scheduleEditor\.repeat !== 'minutely'/.test(indexHtml),
  'detail frequency should use the real interval and omit clock time for interval schedules'
);
assert.ok(
  /async function pickFolder\(\)[\s\S]{0,260}directory:\s*true[\s\S]{0,120}multiple:\s*false/.test(tauriBridge) &&
    /data-testid="scheduled-detail-pick-folder"/.test(indexHtml),
  'the live scheduled-task editor should offer the native folder picker while retaining path input'
);
assert.ok(
  /data-testid="scheduled-filter-tabs"/.test(indexHtml) &&
    /data-testid="scheduled-left-toolbar"/.test(indexHtml) &&
    /data-testid="scheduled-detail-toolbar"/.test(indexHtml) &&
    /data-testid="scheduled-detail-prompt"/.test(indexHtml) &&
    /data-testid="scheduled-detail-settings"/.test(indexHtml) &&
    /data-testid="scheduled-detail-frequency"/.test(indexHtml) &&
    !/<h1[^>]*>定时任务<\/h1>/.test(indexHtml),
  'the configured-task view should use the compact split layout instead of the redundant hero and status blocks'
);
assert.ok(
  /async function openRunChat\(run\)[\s\S]*?bridge\.openScheduledRunChat\(run,\s*detail \|\| selected\)/.test(indexHtml),
  'scheduled run history rows should open the run chat session'
);
assert.ok(
  !/data-testid="scheduled-run-mode"/.test(indexHtml) &&
    !/data-testid="scheduled-live-mode"/.test(indexHtml) &&
    /backendInput\.mode = "yolo"/.test(tauriBridge) &&
    /Object\.assign\(\{\}, input \|\| \{\}, \{ mode: "yolo" \}\)/.test(tauriBridge),
  'scheduled tasks should hide mode controls and force Yolo on every write'
);
assert.ok(
  !/async function openRunChat\(run\)[\s\S]{0,420}opened && onOpenChat/.test(indexHtml),
  'opening a run must not leave the scheduled route'
);
assert.ok(
  !/data-testid="scheduled-draft-editor"/.test(indexHtml) &&
    !/data-testid="scheduled-draft-confirm"/.test(indexHtml) &&
    !/const ScheduledTaskDraftCard/.test(indexHtml),
  'scheduled tasks should not have a separate draft confirmation surface'
);
assert.ok(
  /data-testid="scheduled-live-title"/.test(indexHtml) &&
    /data-testid="scheduled-live-prompt"/.test(indexHtml) &&
    /data-testid="scheduled-live-project"/.test(indexHtml) &&
    /testId="scheduled-live-model"/.test(indexHtml) &&
    /testId="scheduled-live-repeat"/.test(indexHtml) &&
    /data-testid="scheduled-live-time"/.test(indexHtml),
  'the selected task detail should be the live editable surface'
);
assert.ok(
  /const ScheduledSelect =/.test(indexHtml) &&
    /aria-haspopup="listbox"/.test(indexHtml) &&
    /document\.addEventListener\('pointerdown', closeOutside\)/.test(indexHtml) &&
    /event\.key === 'Escape'/.test(indexHtml) &&
    !/<select data-testid="scheduled-live-(?:model|repeat)"/.test(indexHtml),
  'Scheduled model and frequency controls should use the themed keyboard-dismissible popover'
);
assert.ok(
  /setSaveState\(Object\.keys\(pendingPatchRef\.current\)\.length \? 'editing' : 'saved'\)/.test(indexHtml) &&
    /const failureIsCurrent = Object\.keys\(payload\)\.some/.test(indexHtml) &&
    /mountedRef\.current && failureIsCurrent/.test(indexHtml),
  'an older autosave completion must not flash an error over newer pending edits'
);
assert.ok(
  /editable=\{!busy && item\.id === lastUserId\}/.test(indexHtml) &&
    !/async function editLastTurn\(newText\)[\s\S]{0,420}isScheduledRunSession\(state\.activeSessionId\)\) return false/.test(tauriBridge) &&
    !indexHtml.includes('定时运行使用创建时锁定的模型'),
  'a scheduled run opened from history should use the ordinary chat editor and composer controls'
);
assert.ok(
  /function startTemplate\(template\)[\s\S]{0,1200}bridge\.createScheduledTask\(/.test(indexHtml) &&
    !/function saveDraft\(/.test(indexHtml),
  'clicking a template should create and select the task immediately'
);
assert.ok(
  /fn should_sync_session\([\s\S]{0,120}is_scheduled \|\| has_messages/.test(enginePoolRust) &&
    /should_sync_session\(is_scheduled, !saved\.messages\.is_empty\(\)\)/.test(enginePoolRust),
  'scheduled sessions must SyncSession even when their durable message list is empty'
);
assert.ok(
  /请一次只问我一个问题[\s\S]*1\.[\s\S]*2\.[\s\S]*3\.[\s\S]*4\.[\s\S]*5\./.test(scheduledTaskPromptRust),
  'backend prompt should include the guided-chat checklist'
);
assert.ok(
  scheduledTaskPromptRust.includes("FREQ=MINUTELY;INTERVAL=10") &&
    scheduledTaskPromptRust.includes("FREQ=HOURLY;INTERVAL=6") &&
    scheduledTaskPromptRust.includes("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30") &&
    scheduledTaskPromptRust.includes("FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30"),
  'backend prompt should include supported rrule examples'
);
assert.ok(
  !scheduledTaskPromptRust.includes("不支持分钟级"),
  'backend prompt should not say minute-level schedules are unsupported'
);
mustNotContain("每日项目状态提醒");
mustNotContain("每周资料整理提醒");
mustNotContain("Templates");
mustNotContain("模板库会在后续接入");
mustNotContain('title="编辑"');
assert.strictEqual((scheduledTemplateSource.match(/workspace:\s*\[\]/g) || []).length, 3, 'the suggestion area should contain exactly three templates');
assert.strictEqual((scheduledTemplateSource.match(/mode:\s*'(?:agent|plan|yolo)'/g) || []).length, 0, 'templates should not expose a selectable execution mode');
assert.strictEqual((scheduledTemplateSource.match(/allowShell:\s*false/g) || []).length, 3, 'every template should fail closed for shell by default');
assert.strictEqual((scheduledTemplateSource.match(/trustMode:\s*false/g) || []).length, 3, 'every template should declare its trust default');
assert.strictEqual((scheduledTemplateSource.match(/autoApprove:\s*false/g) || []).length, 3, 'every template should declare its approval default');
assert.strictEqual((scheduledTemplateSource.match(/paused:\s*true/g) || []).length, 3, 'templates without a workspace must start paused');
assert.ok(
  scheduledTasksRust.includes('Scheduled task requires a workspace before it can run') &&
    /active_without_workspace[\s\S]{0,900}AutomationStatus::Paused/.test(scheduledTasksRust),
  'backend must fail closed for new, legacy, resumed, and manual empty-workspace tasks'
);
assert.ok(
  !scheduledTemplateSource.includes("id: 'project-health'") && !scheduledTemplateSource.includes("id: 'material-digest'"),
  'only the three Codex-style suggested templates should remain'
);
assert.ok(
  /选定[^']*(项目|目录)[^']*(近期|最近)[^']*(变化|改动)/.test(scheduledTemplateSource) &&
    /待办|未完成/.test(scheduledTemplateSource) && /风险/.test(scheduledTemplateSource),
  'template prompts should focus on selected workspace changes, pending work, and risks'
);
assert.ok(
  /function startTemplate\(template\)[\s\S]{0,700}bridge\.createScheduledTask\([\s\S]{0,220}cwds:\s*\[\.\.\.workspace\][\s\S]{0,260}mode:\s*'yolo'[\s\S]{0,260}allowShell:\s*!!template\.allowShell/.test(indexHtml),
  'selecting a template should immediately create it with workspace, fixed Yolo mode, and permission defaults'
);
assert.ok(
  /const visibleSuggestions\s*=\s*SCHEDULED_TASK_TEMPLATES\.filter/.test(indexHtml) &&
    /tasks\.some\([\s\S]{0,300}task\.templateId\s*===\s*template\.id/.test(indexHtml) &&
    /visibleSuggestions\.map\(template/.test(indexHtml),
  'a template already represented by a configured task should disappear from suggestions by durable source id'
);
assert.ok(
  /scheduled-task-template-sources-v1/.test(tauriBridge) &&
    /delete backendInput\.templateId/.test(tauriBridge) &&
    /templateId:\s*template\.id/.test(indexHtml),
  'template source ids should persist in the frontend sidecar without leaking into the base automation request'
);

function deferred() {
  var resolve;
  var reject;
  var promise = new Promise(function (res, rej) {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function tick() {
  return new Promise(function (resolve) { setImmediate(resolve); });
}

function createBridgeHarness(sharedStorage) {
  var listeners = Object.create(null);
  var handlers = Object.create(null);
  var calls = [];
  var dialogCalls = [];
  var dialogResult = null;
  var createdSession = 0;
  var storageData = sharedStorage || Object.create(null);
  var storage = {
    getItem: function (key) { return Object.prototype.hasOwnProperty.call(storageData, key) ? storageData[key] : null; },
    setItem: function (key, value) { storageData[key] = String(value); },
    removeItem: function (key) { delete storageData[key]; },
  };
  var document = {
    readyState: "loading",
    addEventListener: function () {},
  };

  function defaultInvoke(cmd, args) {
    if (cmd === "load_session") {
      return {
        metadata: { id: args.id, title: args.id.indexOf("sched-") === 0 ? "Scheduled run" : "New chat" },
        messages: [],
        artifacts: [],
      };
    }
    if (cmd === "create_session") {
      createdSession += 1;
      return { id: "chat-created-" + createdSession, title: "New chat" };
    }
    if (cmd === "list_models") {
      return {
        models: [{ id: "model-active", model: "/wire-active" }],
        active_model_id: "model-active",
      };
    }
    if (cmd === "list_sessions" || cmd === "list_archived_sessions" || cmd === "list_personas" ||
        cmd === "get_session_persona_events" || cmd === "get_session_pinvou_reviews" ||
        cmd === "list_workspace_files" || cmd === "list_scheduled_task_runs") return [];
    if (cmd === "get_mode_state") return { mode: "yolo" };
    if (cmd === "get_memory_overview") return {};
    if (cmd === "session_mounted_collection" || cmd === "get_active_persona" ||
        cmd === "find_resumable_run" || cmd === "check_for_update") return null;
    if (cmd === "get_settings") return { theme: "genesis", language: "zh-Hans" };
    if (cmd === "get_backend_status") return {};
    if (cmd === "scheduled_task_chat_prompt") return "scheduled guide";
    if (cmd === "read_scheduled_task") return { id: args.id, name: args.id };
    if (cmd === "create_scheduled_task") {
      return Object.assign({ id: "automation-created" }, args.input || {});
    }
    return null;
  }

  function invoke(cmd, args) {
    calls.push({ cmd: cmd, args: args || null });
    try {
      if (handlers[cmd]) return Promise.resolve(handlers[cmd](args || {}));
      return Promise.resolve(defaultInvoke(cmd, args || {}));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  var window = {
    __TAURI__: {
      core: { invoke: invoke },
      event: {
        listen: function (name, fn) {
          listeners[name] = fn;
          return Promise.resolve(function () {});
        },
      },
      dialog: {
        open: function (options) {
          dialogCalls.push(options || {});
          return Promise.resolve(dialogResult);
        },
      },
    },
    addEventListener: function () {},
    localStorage: storage,
  };
  window.window = window;
  window.document = document;
  var context = {
    window: window,
    document: document,
    localStorage: storage,
    console: { log: function () {}, warn: function () {}, error: function () {} },
    setTimeout: setTimeout,
    clearTimeout: clearTimeout,
    setInterval: function () { return 0; },
    clearInterval: function () {},
    structuredClone: function (value) { return JSON.parse(JSON.stringify(value)); },
  };
  vm.runInNewContext(tauriBridge, context, { filename: "tauri-bridge.js" });

  return {
    bridge: window.TauriBridge,
    handlers: handlers,
    calls: calls,
    storageData: storageData,
    dialogCalls: dialogCalls,
    setDialogResult: function (value) { dialogResult = value; },
    emit: function (name, payload) {
      assert.ok(listeners[name], "expected listener " + name);
      return listeners[name]({ payload: payload || {} });
    },
  };
}

async function scheduledRunUnreadBehavior() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var task = { id: "automation-unread", name: "Unread task", hasUnreadRuns: true };
  var runs = [
    { id: "run-1", automationId: task.id, sessionId: "sched-run-1", status: "completed", unread: true },
    { id: "run-2", automationId: task.id, sessionId: "sched-run-2", status: "completed", unread: true },
  ];
  var openedContextPublished = false;
  bridge.subscribe(function (state) {
    if (state.scheduledRunContext && state.scheduledRunContext.runId) openedContextPublished = true;
  });
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.read_scheduled_task = function () { return Object.assign({}, task); };
  harness.handlers.list_scheduled_task_runs = function () {
    return runs.map(function (run) { return Object.assign({}, run); });
  };
  harness.handlers.mark_scheduled_run_viewed = function (args) {
    assert.ok(openedContextPublished, "the full conversation view must be published before its run is marked viewed");
    return {
      automationId: args.automationId,
      runId: args.runId,
      hasUnreadRuns: args.runId === "run-1",
    };
  };

  await bridge.switchToSession("chat-origin");
  await bridge.loadScheduledTasks();
  bridge.selectScheduledTask(task.id);
  await bridge.readScheduledTask(task.id);
  await bridge.loadScheduledTaskRuns(task.id, 20);
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length,
    0,
    "opening task details or run history must not mark any independent run conversation as viewed"
  );

  assert.strictEqual(await bridge.openScheduledRunChat(runs[0], task), true);
  var marks = harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; });
  assert.strictEqual(JSON.stringify(marks.map(function (call) { return call.args; })), JSON.stringify([
    { automationId: task.id, runId: "run-1" },
  ]));
  var afterFirst = bridge.getState();
  assert.strictEqual(afterFirst.scheduledTaskRuns[0].unread, false, "the opened run should become viewed");
  assert.strictEqual(afterFirst.scheduledTaskRuns[1].unread, true, "sibling runs remain independently unread");
  assert.strictEqual(afterFirst.scheduledTasks[0].hasUnreadRuns, true, "task dot remains while a child run is unread");
  assert.strictEqual(afterFirst.scheduledTaskDetail.hasUnreadRuns, true);

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.openScheduledRunChat(runs[1], task), true);
  var afterSecond = bridge.getState();
  assert.ok(afterSecond.scheduledTaskRuns.every(function (run) { return run.unread === false; }));
  assert.strictEqual(afterSecond.scheduledTasks[0].hasUnreadRuns, false, "task dot clears only after every child run was opened");
  assert.strictEqual(afterSecond.scheduledTaskDetail.hasUnreadRuns, false);

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  harness.emit("chat:delta", { session_id: "sched-running", text: "partial live output" });
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-running") {
      return {
        metadata: { id: "sched-running", title: "Running scheduled conversation" },
        messages: [{ role: "user", content: [{ type: "text", text: "durable scheduled prompt" }] }],
        artifacts: [],
      };
    }
    return {
      metadata: { id: args.id, title: "Scheduled conversation" },
      messages: [], artifacts: [],
    };
  };
  var markCount = harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length;
  var loadCount = harness.calls.filter(function (call) { return call.cmd === "load_session"; }).length;
  assert.strictEqual(
    await bridge.openScheduledRunChat(
      { id: "run-running", automationId: task.id, sessionId: "sched-running", status: "running", unread: false },
      task
    ),
    true,
    "a running scheduled conversation should open in the ordinary live ChatView"
  );
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length,
    markCount,
    "opening a running conversation must not preemptively mark its future completion as viewed"
  );
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "load_session"; }).length,
    loadCount + 1,
    "a running conversation should hydrate its durable prompt without replacing the live buffer"
  );
  assert.strictEqual(bridge.getState().activeSessionId, "sched-running");
  assert.ok(
    bridge.getState().chatItems.some(function (item) {
      return String(item.text || item.html || "").includes("partial live output");
    }),
    "the normal chat transcript should expose buffered live output"
  );
  assert.ok(
    JSON.stringify(bridge.getState().chatItems).includes("durable scheduled prompt"),
    "the normal chat transcript should also include the durable user prompt"
  );
  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  harness.emit("chat:delta", { session_id: "sched-buffered", text: "partial background output" });
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-buffered") throw new Error("buffered scheduled session is not durable");
    throw new Error("missing scheduled session");
  };
  assert.strictEqual(
    await bridge.openScheduledRunChat(
      { id: "run-buffered", automationId: task.id, sessionId: "sched-buffered", status: "completed", unread: true },
      task
    ),
    false,
    "a background event buffer must never replace loading the complete durable conversation"
  );
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length,
    markCount,
    "a buffered run whose durable conversation failed to load must remain unread"
  );
  assert.strictEqual(
    await bridge.openScheduledRunChat(
      { id: "run-missing", automationId: task.id, sessionId: "sched-missing", status: "completed", unread: true },
      task
    ),
    false
  );
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length,
    markCount,
    "a conversation that failed to load must remain unread"
  );
}

async function scheduledFolderPickerBehavior() {
  var harness = createBridgeHarness();
  harness.setDialogResult("D:/workspace-picked");
  assert.strictEqual(await harness.bridge.pickFolder(), "D:/workspace-picked");
  assert.strictEqual(JSON.stringify(harness.dialogCalls[0]), JSON.stringify({
    directory: true,
    multiple: false,
    title: "选择工作目录",
  }));
  harness.setDialogResult(null);
  assert.strictEqual(await harness.bridge.pickFolder(), null, "canceling folder selection should preserve the typed path");
}

async function scheduledRunningHydrationRaceBehavior() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var load = deferred();
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-live-race") return load.promise;
    return {
      metadata: { id: args.id, title: "Origin" },
      messages: [], artifacts: [],
    };
  };
  harness.handlers.mark_scheduled_run_viewed = function () {
    return { automationId: "automation-race-live", runId: "run-race-live", hasUnreadRuns: false };
  };
  await bridge.switchToSession("chat-origin");
  var opening = bridge.openScheduledRunChat(
    {
      id: "run-race-live", automationId: "automation-race-live",
      sessionId: "sched-live-race", status: "running", unread: false,
    },
    { id: "automation-race-live", name: "Live race task", mode: "agent" }
  );
  await tick();
  harness.emit("chat:delta", { session_id: "sched-live-race", text: "delta received during durable load" });
  harness.emit("chat:tool_start", {
    session_id: "sched-live-race", id: "tool-hydrate", name: "shell", args: { command: "echo hydrate" },
  });
  load.resolve({
    metadata: { id: "sched-live-race", title: "Live scheduled run" },
    messages: [
      { role: "user", content: [{ type: "text", text: "persisted scheduled prompt" }] },
      { role: "assistant", content: [
        { type: "text", text: "delta received during durable load" },
        { type: "tool_use", id: "tool-hydrate", name: "shell", input: { command: "echo hydrate" } },
      ] },
    ],
    artifacts: [],
  });
  assert.strictEqual(await opening, true);
  var rendered = JSON.stringify(bridge.getState().chatItems);
  assert.ok(rendered.includes("persisted scheduled prompt"), "durable history should survive live hydration");
  assert.ok(rendered.includes("delta received during durable load"), "live deltas received during load should survive hydration");
  assert.strictEqual(
    (rendered.match(/delta received during durable load/g) || []).length,
    1,
    "durable and live overlap should render once"
  );
  assert.strictEqual(
    bridge.getState().chatItems.filter(function (item) {
      return item.type === "tool" && item.toolId === "tool-hydrate";
    }).length,
    1,
    "durable and live tool cards should merge by tool id"
  );
}

async function openingRunningMarksBusyBeforeHydration() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var load = deferred();
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-opening-busy") return load.promise;
    return {
      metadata: { id: args.id, title: "Origin" },
      messages: [], artifacts: [],
    };
  };
  await bridge.switchToSession("chat-origin");
  var opening = bridge.openScheduledRunChat({
    id: "run-opening-busy",
    automationId: "automation-opening-busy",
    sessionId: "sched-opening-busy",
    status: "running",
    unread: false,
  }, { id: "automation-opening-busy", name: "Opening busy task" });
  await tick();

  assert.strictEqual(
    bridge.getState().sessionBusy["sched-opening-busy"],
    true,
    "a queued/running scheduled buffer must be busy before durable hydration starts"
  );

  load.resolve({
    metadata: { id: "sched-opening-busy", title: "Opening scheduled run" },
    messages: [], artifacts: [],
  });
  assert.strictEqual(await opening, true);
}

async function followupQueuedUntilScheduledInitialTurnTerminal() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  await bridge.switchToSession("chat-origin");
  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-followup",
    automationId: "automation-followup",
    sessionId: "sched-followup",
    status: "running",
    unread: false,
  }, { id: "automation-followup", name: "Follow-up task" }), true);
  harness.emit("chat:delta", { session_id: "sched-followup", text: "initial scheduled output" });
  var initialAssistantCount = bridge.getState().chatItems.filter(function (item) {
    return item.type === "assistant";
  }).length;

  await bridge.sendMessage("follow up after the scheduled run");
  var queued = bridge.getState();
  assert.strictEqual(queued.queued.length, 1, "follow-up input must queue while the initial scheduled turn is active");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    0,
    "a queued follow-up must not overlap the scheduled engine turn"
  );
  assert.strictEqual(
    queued.chatItems.filter(function (item) { return item.type === "assistant"; }).length,
    initialAssistantCount,
    "queueing a follow-up must not create an overlapping assistant placeholder"
  );

  harness.emit("chat:done", { session_id: "sched-followup" });
  await tick();
  await tick();
  var flushed = bridge.getState();
  assert.strictEqual(flushed.queued.length, 0);
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    1,
    "the queued follow-up should flush only after the scheduled terminal event"
  );
  assert.strictEqual(flushed.busy, true, "the flushed follow-up should own the next busy turn");
}

async function terminalEventWinsStaleRunningOpen() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var firstLoad = deferred();
  var loads = 0;
  harness.handlers.load_session = function (args) {
    if (args.id !== "sched-terminal-wins") {
      return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
    }
    loads += 1;
    if (loads === 1) return firstLoad.promise;
    return {
      metadata: { id: args.id, title: "Completed while opening" },
      messages: [], artifacts: [],
    };
  };
  var staleRun = {
    id: "run-terminal-wins",
    automationId: "automation-terminal-wins",
    sessionId: "sched-terminal-wins",
    status: "running",
    unread: false,
  };
  await bridge.switchToSession("chat-origin");
  var opening = bridge.openScheduledRunChat(staleRun, {
    id: staleRun.automationId,
    name: "Terminal wins task",
  });
  await tick();
  harness.emit("chat:done", { session_id: staleRun.sessionId });
  firstLoad.resolve({
    metadata: { id: staleRun.sessionId, title: "Completed while opening" },
    messages: [], artifacts: [],
  });
  assert.strictEqual(await opening, true);
  assert.strictEqual(bridge.getState().busy, false, "terminal event should clear initial busy after hydration");

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.openScheduledRunChat(staleRun, {
    id: staleRun.automationId,
    name: "Terminal wins task",
  }), true);
  assert.strictEqual(
    bridge.getState().busy,
    false,
    "a stale running DTO must not move a terminal scheduled buffer back to active"
  );
  await bridge.sendMessage("continue after terminal");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    1,
    "a follow-up after terminal should start normally instead of remaining queued"
  );

  var completedHarness = createBridgeHarness();
  var completedBridge = completedHarness.bridge;
  var completedSessionId = "owned-completed-session";
  await completedBridge.switchToSession("chat-origin");
  assert.strictEqual(await completedBridge.openScheduledRunChat({
    id: "run-completed-owned",
    automationId: "automation-completed-owned",
    sessionId: completedSessionId,
    status: "completed",
    unread: true,
  }, { id: "automation-completed-owned", name: "Completed owned task" }), true);
  assert.strictEqual(await completedBridge.exitScheduledRunChat(), true);
  assert.strictEqual(await completedBridge.openScheduledRunChat({
    id: "run-completed-owned",
    automationId: "automation-completed-owned",
    sessionId: completedSessionId,
    status: "running",
    unread: false,
  }, { id: "automation-completed-owned", name: "Completed owned task" }), true);
  assert.strictEqual(
    completedBridge.getState().busy,
    false,
    "a completed durable open must remain terminal when an older running DTO arrives later"
  );
}

async function scheduledDoneBeforeBufferCreatesTerminalTombstone() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  await bridge.switchToSession("chat-origin");

  harness.emit("chat:done", { session_id: "sched-done-before-buffer" });
  await tick();
  await tick();
  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-done-before-buffer",
    automationId: "automation-done-before-buffer",
    sessionId: "sched-done-before-buffer",
    status: "running",
    unread: false,
  }, { id: "automation-done-before-buffer", name: "Done first task" }), true);
  assert.strictEqual(
    bridge.getState().busy,
    false,
    "a scheduled terminal event received before buffer creation must beat a later stale running DTO"
  );
  await bridge.sendMessage("continue after done-first run");
  assert.strictEqual(bridge.getState().queued.length, 0, "done-first terminal state must not strand follow-up input");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    1,
    "follow-up after a done-first run should start immediately"
  );

  harness.emit("chat:done", { session_id: "ordinary-done-without-buffer" });
  await tick();
  assert.ok(
    !Object.prototype.hasOwnProperty.call(bridge.getState().sessionBusy, "ordinary-done-without-buffer"),
    "an ordinary unknown chat:done must not create a background session buffer"
  );
}

async function failedRunningOpenRollsBackOnlyItsProvisionalBusy() {
  var failedLoadHarness = createBridgeHarness();
  await failedLoadHarness.bridge.switchToSession("chat-origin");
  failedLoadHarness.handlers.load_session = function (args) {
    if (args.id === "sched-open-load-fails") throw new Error("scheduled load failed");
    return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
  };
  assert.strictEqual(await failedLoadHarness.bridge.openScheduledRunChat({
    id: "run-open-load-fails",
    automationId: "automation-open-load-fails",
    sessionId: "sched-open-load-fails",
    status: "running",
  }, { name: "Load failure task" }), false);
  assert.strictEqual(failedLoadHarness.bridge.getState().activeSessionId, "chat-origin");
  assert.ok(
    !failedLoadHarness.bridge.getState().sessionBusy["sched-open-load-fails"],
    "a failed running open must roll back the provisional busy flag it introduced"
  );

  var staleRequestHarness = createBridgeHarness();
  var targetLoad = deferred();
  staleRequestHarness.handlers.load_session = function (args) {
    if (args.id === "sched-open-stale-request") return targetLoad.promise;
    return { metadata: { id: args.id, title: "Other" }, messages: [], artifacts: [] };
  };
  await staleRequestHarness.bridge.switchToSession("chat-origin");
  var staleOpening = staleRequestHarness.bridge.openScheduledRunChat({
    id: "run-open-stale-request",
    automationId: "automation-open-stale-request",
    sessionId: "sched-open-stale-request",
    status: "running",
  }, { name: "Stale open task" });
  await tick();
  assert.strictEqual(await staleRequestHarness.bridge.switchToSession("chat-other"), true);
  targetLoad.resolve({
    metadata: { id: "sched-open-stale-request", title: "Stale scheduled load" },
    messages: [], artifacts: [],
  });
  assert.strictEqual(await staleOpening, false);
  assert.strictEqual(staleRequestHarness.bridge.getState().activeSessionId, "chat-other");
  assert.ok(
    !staleRequestHarness.bridge.getState().sessionBusy["sched-open-stale-request"],
    "an invalidated running open must roll back only its provisional busy flag"
  );

  var liveHarness = createBridgeHarness();
  await liveHarness.bridge.switchToSession("chat-origin");
  var liveRun = {
    id: "run-open-live",
    automationId: "automation-open-live",
    sessionId: "sched-open-live",
    status: "running",
  };
  assert.strictEqual(await liveHarness.bridge.openScheduledRunChat(liveRun, { name: "Live task" }), true);
  liveHarness.emit("chat:delta", { session_id: liveRun.sessionId, text: "real live output" });
  assert.strictEqual(await liveHarness.bridge.exitScheduledRunChat(), true);
  liveHarness.handlers.load_session = function (args) {
    if (args.id === liveRun.sessionId) throw new Error("reopen failed");
    return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
  };
  assert.strictEqual(await liveHarness.bridge.openScheduledRunChat(liveRun, { name: "Live task" }), false);
  assert.strictEqual(
    liveHarness.bridge.getState().sessionBusy[liveRun.sessionId],
    true,
    "failure rollback must not clear a busy phase that existed before this open attempt"
  );
}

async function concurrentFailedRunningOpensShareRollback() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var loadCalls = 0;
  await bridge.switchToSession("chat-origin");
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-double-open-fails") {
      loadCalls += 1;
      throw new Error("double open load failed");
    }
    return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
  };
  var run = {
    id: "run-double-open-fails",
    automationId: "automation-double-open-fails",
    sessionId: "sched-double-open-fails",
    status: "running",
  };
  var first = bridge.openScheduledRunChat(run, { name: "Double open task" });
  var second = bridge.openScheduledRunChat(run, { name: "Double open task" });
  var results = await Promise.all([first, second]);

  assert.deepStrictEqual(results, [false, false]);
  assert.strictEqual(loadCalls, 1, "concurrent opens for one scheduled session must share one durable load");
  assert.ok(
    !bridge.getState().sessionBusy[run.sessionId],
    "the shared failed open must roll back provisional busy after its final caller settles"
  );
}

async function scheduledOwnerRegistryIsBoundedAndProtectsLive() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  await bridge.switchToSession("chat-origin");
  var liveRun = {
    id: "run-owner-live",
    automationId: "automation-owner-live",
    sessionId: "owned-owner-live",
    status: "running",
  };
  assert.strictEqual(await bridge.openScheduledRunChat(liveRun, { name: "Owner live task" }), true);
  harness.emit("chat:delta", { session_id: liveRun.sessionId, text: "protected owner live output" });

  harness.handlers.list_scheduled_task_runs = function () {
    return Array.from({ length: 80 }, function (_, index) {
      return {
        id: "run-owner-" + index,
        automationId: "automation-owner-history",
        sessionId: "owned-owner-" + index,
        status: "completed",
        unread: true,
      };
    });
  };
  bridge.selectScheduledTask("automation-owner-history");
  await bridge.loadScheduledTaskRuns("automation-owner-history", 80);
  assert.strictEqual(bridge.getState().activeSessionId, liveRun.sessionId);
  assert.ok(
    JSON.stringify(bridge.getState().chatItems).includes("protected owner live output"),
    "owner pruning must preserve the current live scheduled conversation"
  );
  assert.strictEqual(await bridge.exitScheduledRunChat(), true);

  harness.emit("chat:done", { session_id: "owned-owner-79" });
  await tick();
  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-owner-79",
    automationId: "automation-owner-history",
    sessionId: "owned-owner-79",
    status: "running",
  }, { name: "Pruned owner task" }), true);
  assert.strictEqual(
    bridge.getState().busy,
    true,
    "hard cap must evict lower-priority visible owners once current/context consume registry slots"
  );
  assert.strictEqual(await bridge.exitScheduledRunChat(), true);

  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-owner-0",
    automationId: "automation-owner-history",
    sessionId: "owned-owner-0",
    status: "running",
  }, { name: "Visible owner task" }), true);
  assert.strictEqual(
    bridge.getState().busy,
    false,
    "the most recent visible terminal run owner must survive registry pruning"
  );

  var busyHarness = createBridgeHarness();
  await busyHarness.bridge.switchToSession("chat-origin");
  for (var busyIndex = 0; busyIndex < 70; busyIndex++) {
    assert.strictEqual(await busyHarness.bridge.openScheduledRunChat({
      id: "run-owner-busy-" + busyIndex,
      automationId: "automation-owner-busy-" + busyIndex,
      sessionId: "owned-owner-busy-" + busyIndex,
      status: "running",
    }, { name: "Busy owner " + busyIndex }), true);
    assert.strictEqual(await busyHarness.bridge.exitScheduledRunChat(), true);
  }
  busyHarness.emit("chat:done", { session_id: "owned-owner-busy-0" });
  await tick();
  assert.strictEqual(await busyHarness.bridge.openScheduledRunChat({
    id: "run-owner-busy-0",
    automationId: "automation-owner-busy-0",
    sessionId: "owned-owner-busy-0",
    status: "running",
  }, { name: "Busy owner 0" }), true);
  assert.strictEqual(
    busyHarness.bridge.getState().busy,
    false,
    "a live buffer must remain recognizable after its separate owner registry entry is hard-capped"
  );
}

async function scheduledBufferLruNeverEvictsLive() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  await bridge.switchToSession("chat-origin");

  harness.emit("chat:delta", {
    session_id: "sched-lru-cold",
    text: "cold buffer should be evicted",
  });
  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-lru-live",
    automationId: "automation-lru-live",
    sessionId: "sched-lru-live",
    status: "running",
    unread: false,
  }, { id: "automation-lru-live", name: "LRU live task" }), true);
  harness.emit("chat:delta", {
    session_id: "sched-lru-live",
    text: "live buffer must survive",
  });
  await bridge.sendMessage("queued live follow-up");
  assert.strictEqual(bridge.getState().queued.length, 1);
  assert.strictEqual(await bridge.exitScheduledRunChat(), true);

  for (var i = 0; i < 70; i++) {
    harness.emit("chat:delta", {
      session_id: "sched-lru-cold-" + i,
      text: "cold " + i,
    });
  }

  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-lru-cold",
    automationId: "automation-lru-cold",
    sessionId: "sched-lru-cold",
    status: "running",
    unread: false,
  }, { id: "automation-lru-cold", name: "LRU cold task" }), true);
  assert.ok(
    !JSON.stringify(bridge.getState().chatItems).includes("cold buffer should be evicted"),
    "an inactive scheduled buffer older than the 64-entry cap should be evicted"
  );
  assert.strictEqual(await bridge.exitScheduledRunChat(), true);

  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-lru-live",
    automationId: "automation-lru-live",
    sessionId: "sched-lru-live",
    status: "running",
    unread: false,
  }, { id: "automation-lru-live", name: "LRU live task" }), true);
  var live = bridge.getState();
  assert.ok(
    JSON.stringify(live.chatItems).includes("live buffer must survive"),
    "LRU must never evict a busy scheduled buffer"
  );
  assert.strictEqual(live.queued.length, 1, "LRU must never evict a scheduled buffer with queued input");

  var saturatedHarness = createBridgeHarness();
  await saturatedHarness.bridge.switchToSession("chat-origin");
  for (var protectedIndex = 0; protectedIndex < 64; protectedIndex++) {
    assert.strictEqual(await saturatedHarness.bridge.openScheduledRunChat({
      id: "run-protected-" + protectedIndex,
      automationId: "automation-protected-" + protectedIndex,
      sessionId: "sched-protected-" + protectedIndex,
      status: "running",
    }, { name: "Protected task " + protectedIndex }), true);
    assert.strictEqual(await saturatedHarness.bridge.exitScheduledRunChat(), true);
  }
  assert.strictEqual(await saturatedHarness.bridge.openScheduledRunChat({
    id: "run-protected-new",
    automationId: "automation-protected-new",
    sessionId: "sched-protected-new",
    status: "running",
  }, { name: "New protected task" }), true);
  assert.strictEqual(
    saturatedHarness.bridge.getState().busy,
    true,
    "when all 64 older buffers are live, LRU must retain the newly opened running buffer too"
  );
}

async function scheduledTemplateSourcePersistenceBehavior() {
  var sharedStorage = Object.create(null);
  var first = createBridgeHarness(sharedStorage);
  var backendInput = null;
  first.handlers.create_scheduled_task = function (args) {
    backendInput = args.input;
    return Object.assign({ id: "automation-template" }, args.input);
  };
  var created = await first.bridge.createScheduledTask({
    name: "Completely renamed",
    prompt: "Completely edited prompt",
    rrule: "FREQ=HOURLY;INTERVAL=3",
    templateId: "weekly-review",
  });
  assert.ok(!Object.prototype.hasOwnProperty.call(backendInput, "templateId"), "UI-only template ids must not leak into the base request");
  assert.strictEqual(created.templateId, "weekly-review");

  var second = createBridgeHarness(sharedStorage);
  second.handlers.list_scheduled_tasks = function () {
    return [{
      id: "automation-template",
      name: "Completely renamed",
      prompt: "Completely edited prompt",
      rrule: "FREQ=HOURLY;INTERVAL=3",
    }];
  };
  await second.bridge.loadScheduledTasks();
  assert.strictEqual(
    second.bridge.getState().scheduledTasks[0].templateId,
    "weekly-review",
    "template source must survive a bridge reload even when every visible template field was customized"
  );
}

async function scheduledUnreadPollingRaceBehavior() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var task = { id: "automation-race", name: "Race task", hasUnreadRuns: true };
  var run = {
    id: "run-race", automationId: task.id, sessionId: "sched-race",
    status: "completed", unread: true,
  };
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.read_scheduled_task = function () { return Object.assign({}, task); };
  harness.handlers.list_scheduled_task_runs = function () { return [Object.assign({}, run)]; };
  harness.handlers.mark_scheduled_run_viewed = function () {
    return { automationId: task.id, runId: run.id, hasUnreadRuns: false };
  };
  await bridge.loadScheduledTasks();
  bridge.selectScheduledTask(task.id);
  await bridge.readScheduledTask(task.id);
  await bridge.loadScheduledTaskRuns(task.id, 20);

  var staleTasks = deferred();
  var staleDetail = deferred();
  var staleRuns = deferred();
  harness.handlers.list_scheduled_tasks = function () { return staleTasks.promise; };
  harness.handlers.read_scheduled_task = function () { return staleDetail.promise; };
  harness.handlers.list_scheduled_task_runs = function () { return staleRuns.promise; };
  var staleRefresh = bridge.refreshScheduledTaskData(20);
  await tick();

  assert.strictEqual(await bridge.openScheduledRunChat(run, task), true);
  assert.strictEqual(bridge.getState().scheduledTaskRuns[0].unread, false);
  staleTasks.resolve([Object.assign({}, task)]);
  staleDetail.resolve(Object.assign({}, task));
  staleRuns.resolve([Object.assign({}, run)]);
  await staleRefresh;
  var finalState = bridge.getState();
  assert.strictEqual(finalState.scheduledTaskRuns[0].unread, false, "an older poll must not resurrect a viewed run dot");
  assert.strictEqual(finalState.scheduledTasks[0].hasUnreadRuns, false, "an older poll must not resurrect the task aggregate dot");
  assert.strictEqual(finalState.scheduledTaskDetail.hasUnreadRuns, false);
}

async function scheduledRunNavigationBehavior() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;

  assert.strictEqual(await bridge.switchToSession("chat-origin"), true);
  bridge.selectScheduledTask("automation-1");
  await bridge.readScheduledTask("automation-1");
  harness.handlers.list_scheduled_task_runs = function () {
    return [{ id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" }];
  };
  await bridge.loadScheduledTaskRuns("automation-1", 20);
  assert.strictEqual(
    await bridge.openScheduledRunChat(
      { id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" },
      { id: "automation-1", name: "Nightly report", model: "/locked-model", mode: "plan" }
    ),
    true
  );
  await bridge.sendMessage("continue the scheduled conversation");
  var followup = harness.calls.filter(function (call) { return call.cmd === "chat"; }).pop();
  assert.strictEqual(followup.args.sessionId, "sched-run-1");
  assert.strictEqual(followup.args.restrictTools, false);
  await harness.emit("chat:done", { session_id: "sched-run-1", status: "Completed", error: null });
  var editCallsBefore = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; }).length;
  await bridge.editLastTurn("rewrite scheduled output");
  var editCalls = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; });
  assert.strictEqual(editCalls.length, editCallsBefore + 1);
  assert.strictEqual(editCalls[editCalls.length - 1].args.sessionId, "sched-run-1");
  var opened = bridge.getState();
  assert.strictEqual(opened.activeSessionId, "sched-run-1");
  assert.deepStrictEqual(opened.scheduledRunContext, {
    sessionId: "sched-run-1",
    returnSessionId: "chat-origin",
    automationId: "automation-1",
    runId: "run-1",
    taskName: "Nightly report",
    model: "/locked-model",
    mode: "yolo",
  });

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  var restored = bridge.getState();
  assert.strictEqual(restored.activeSessionId, "chat-origin");
  assert.strictEqual(restored.scheduledRunContext, null);
  assert.strictEqual(restored.selectedScheduledTaskId, "automation-1");
  assert.strictEqual(restored.scheduledTaskDetail.id, "automation-1");
  assert.strictEqual(restored.scheduledTaskRuns[0].id, "run-1");

  await bridge.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" },
    { id: "automation-1", name: "Nightly report" }
  );
  await bridge.createNewSession();
  assert.strictEqual(bridge.getState().activeSessionId, null);
  assert.strictEqual(bridge.getState().scheduledRunContext, null);

  await bridge.switchToSession("chat-origin");
  await bridge.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" },
    { id: "automation-1", name: "Nightly report", model: "/locked-model", mode: "plan" }
  );
  await bridge.switchToSession("chat-origin");
  assert.strictEqual(bridge.getState().scheduledRunContext, null);

  harness.handlers.load_session = function (args) {
    if (args.id === "sched-missing") throw new Error("missing scheduled session");
    return {
      metadata: { id: args.id, title: "New chat" },
      messages: [],
      artifacts: [],
    };
  };
  var chatItemsBeforeFailure = JSON.stringify(bridge.getState().chatItems);
  assert.strictEqual(
    await bridge.openScheduledRunChat(
      { id: "run-missing", automationId: "automation-1", sessionId: "sched-missing", status: "completed" },
      { name: "Missing run" }
    ),
    false
  );
  var failedOpen = bridge.getState();
  assert.strictEqual(failedOpen.activeSessionId, "chat-origin");
  assert.strictEqual(failedOpen.scheduledRunContext, null);
  assert.ok(String(failedOpen.scheduledTaskError).includes("missing scheduled session"));
  assert.strictEqual(JSON.stringify(failedOpen.chatItems), chatItemsBeforeFailure, "scheduled load errors must not pollute chat");
  assert.strictEqual(await bridge.openScheduledRunChat({ id: "no-session", status: "completed" }, {}), false);
  assert.ok(bridge.getState().scheduledTaskError, "missing run sessions should expose a scheduled-scoped error");
}

async function scheduledSelectionGenerationBehavior() {
  var harness = createBridgeHarness();
  var listA = deferred();
  var listB = deferred();
  var listCalls = 0;
  harness.handlers.list_scheduled_tasks = function () {
    listCalls += 1;
    return listCalls === 1 ? listA.promise : listB.promise;
  };

  harness.bridge.selectScheduledTask("automation-a");
  var tasksA = harness.bridge.loadScheduledTasks();
  harness.bridge.selectScheduledTask("automation-b");
  var tasksB = harness.bridge.loadScheduledTasks();
  listB.resolve([{ id: "automation-b", name: "B" }]);
  await tasksB;
  assert.strictEqual(harness.bridge.getState().scheduledTaskLoading, false, "an old task-list generation must not keep the current selection loading");
  listA.resolve([{ id: "automation-a", name: "A" }]);
  await tasksA;
  var state = harness.bridge.getState();
  assert.strictEqual(state.selectedScheduledTaskId, "automation-b");
  assert.strictEqual(state.scheduledTasks[0].id, "automation-b", "a stale task list must not replace the current generation");

  var detailA = deferred();
  var detailB = deferred();
  var runsA = deferred();
  var runsB = deferred();
  harness.handlers.read_scheduled_task = function (args) {
    return args.id === "automation-a" ? detailA.promise : detailB.promise;
  };
  harness.handlers.list_scheduled_task_runs = function (args) {
    return args.id === "automation-a" ? runsA.promise : runsB.promise;
  };

  harness.bridge.selectScheduledTask("automation-a");
  var readA = harness.bridge.readScheduledTask("automation-a");
  var loadRunsA = harness.bridge.loadScheduledTaskRuns("automation-a", 20);
  harness.bridge.selectScheduledTask("automation-b");
  var readB = harness.bridge.readScheduledTask("automation-b");
  var loadRunsB = harness.bridge.loadScheduledTaskRuns("automation-b", 20);
  detailB.resolve({ id: "automation-b", name: "B detail" });
  runsB.resolve([{ id: "run-b", automationId: "automation-b" }]);
  await Promise.all([readB, loadRunsB]);
  assert.strictEqual(harness.bridge.getState().scheduledTaskLoading, false, "old detail/run requests must not keep the current selection loading");
  detailA.resolve({ id: "automation-a", name: "A detail" });
  runsA.resolve([{ id: "run-a", automationId: "automation-a" }]);
  await Promise.all([readA, loadRunsA]);
  state = harness.bridge.getState();
  assert.strictEqual(state.selectedScheduledTaskId, "automation-b");
  assert.strictEqual(state.scheduledTaskDetail.id, "automation-b");
  assert.strictEqual(state.scheduledTaskRuns[0].id, "run-b");
  assert.strictEqual(state.scheduledTaskLoading, false);

  var refreshes = 0;
  harness.handlers.list_scheduled_task_runs = function () {
    refreshes += 1;
    return [{ id: "run-b2", automationId: "automation-b" }];
  };
  harness.handlers.list_scheduled_tasks = function () {
    listCalls += 1;
    return [
      { id: "automation-a", name: "A", hasUnreadRuns: true },
      { id: "automation-b", name: "B", hasUnreadRuns: false },
    ];
  };
  harness.emit("scheduled_task:run_updated", { automationId: "automation-a" });
  await tick();
  assert.strictEqual(listCalls, 3, "unselected automation updates must still refresh global unread task summaries");
  assert.strictEqual(harness.bridge.getState().scheduledTasks[0].hasUnreadRuns, true, "the unselected task unread summary should enter global state");
  assert.strictEqual(refreshes, 0, "unselected automation updates must not refresh run history");
  harness.emit("scheduled_task:run_updated", { automationId: "automation-b" });
  await tick();
  assert.strictEqual(listCalls, 4, "selected automation updates should refresh task summaries with detail and runs");
  assert.strictEqual(refreshes, 1, "selected automation updates should refresh run history");
}

async function scheduledRefreshDoesNotOverlap() {
  var harness = createBridgeHarness();
  var pendingTasks = deferred();
  var pendingDetail = deferred();
  var pendingRuns = deferred();
  var counts = { tasks: 0, detail: 0, runs: 0 };
  harness.handlers.list_scheduled_tasks = function () { counts.tasks += 1; return pendingTasks.promise; };
  harness.handlers.read_scheduled_task = function () { counts.detail += 1; return pendingDetail.promise; };
  harness.handlers.list_scheduled_task_runs = function () { counts.runs += 1; return pendingRuns.promise; };
  harness.bridge.selectScheduledTask("automation-b");

  var first = harness.bridge.refreshScheduledTaskData(20);
  var overlapping = harness.bridge.refreshScheduledTaskData(20);
  await tick();
  assert.deepStrictEqual(counts, { tasks: 1, detail: 1, runs: 1 }, "overlapping polls must share one refresh");
  pendingTasks.resolve([{ id: "automation-b", name: "B" }]);
  pendingDetail.resolve({ id: "automation-b", name: "B" });
  pendingRuns.resolve([{ id: "run-b", automationId: "automation-b" }]);
  await Promise.all([first, overlapping]);

  harness.handlers.list_scheduled_tasks = function () { counts.tasks += 1; return [{ id: "automation-b", name: "B2" }]; };
  harness.handlers.read_scheduled_task = function () { counts.detail += 1; return { id: "automation-b", name: "B2" }; };
  harness.handlers.list_scheduled_task_runs = function () { counts.runs += 1; return [{ id: "run-b2", automationId: "automation-b" }]; };
  await harness.bridge.refreshScheduledTaskData(20);
  assert.deepStrictEqual(counts, { tasks: 2, detail: 2, runs: 2 }, "the next poll should run after the prior one settles");
}

async function scheduledMutationErrorBehavior() {
  var cases = [
    ["pauseScheduledTask", "pause_scheduled_task", ["automation-1"]],
    ["resumeScheduledTask", "resume_scheduled_task", ["automation-1"]],
    ["deleteScheduledTask", "delete_scheduled_task", ["automation-1"]],
    ["runScheduledTaskNow", "run_scheduled_task_now", ["automation-1"]],
    ["createScheduledTask", "create_scheduled_task", [{ name: "X", prompt: "Y", rrule: "FREQ=DAILY" }]],
  ];
  for (var i = 0; i < cases.length; i++) {
    var harness = createBridgeHarness();
    var entry = cases[i];
    harness.handlers[entry[1]] = function () { throw new Error("visible scheduled failure"); };
    await assert.rejects(function () { return harness.bridge[entry[0]].apply(null, entry[2]); }, /visible scheduled failure/);
    var state = harness.bridge.getState();
    assert.ok(String(state.scheduledTaskError).includes("visible scheduled failure"), entry[0] + " should expose its error");
    assert.strictEqual(state.scheduledTaskBusyAction, null, entry[0] + " should clear busy after failure");
  }

  var chatHarness = createBridgeHarness();
  chatHarness.handlers.scheduled_task_chat_prompt = function () { throw new Error("chat creation failed"); };
  await assert.rejects(function () { return chatHarness.bridge.startScheduledTaskChat(); }, /chat creation failed/);
  assert.ok(String(chatHarness.bridge.getState().scheduledTaskError).includes("chat creation failed"));
  assert.strictEqual(chatHarness.bridge.getState().activeSessionId, null, "failed chat creation must not change sessions");
  assert.strictEqual(chatHarness.bridge.getState().scheduledTaskBusyAction, null);
}

async function scheduledDeletePurgesOnlyReportedSessionBuffers() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  harness.emit("chat:delta", { session_id: "sched-delete-exact", text: "purge this exact buffer" });
  harness.emit("chat:delta", { session_id: "sched-delete-retain", text: "retain this sibling buffer" });
  harness.handlers.delete_scheduled_task = function () {
    return {
      id: "automation-delete",
      deletedSessionIds: ["sched-delete-exact"],
    };
  };
  await bridge.deleteScheduledTask("automation-delete");

  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-delete-exact",
    automationId: "automation-delete",
    sessionId: "sched-delete-exact",
    status: "running",
  }, { id: "automation-delete", name: "Deleted task" }), true);
  assert.ok(
    !JSON.stringify(bridge.getState().chatItems).includes("purge this exact buffer"),
    "a backend-reported deleted session id must purge exactly that scheduled buffer"
  );
  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.openScheduledRunChat({
    id: "run-delete-retain",
    automationId: "automation-other",
    sessionId: "sched-delete-retain",
    status: "running",
  }, { id: "automation-other", name: "Sibling task" }), true);
  assert.ok(
    JSON.stringify(bridge.getState().chatItems).includes("retain this sibling buffer"),
    "deleting one task must not guess at or purge unreported scheduled session ids"
  );

  var noIdsHarness = createBridgeHarness();
  noIdsHarness.emit("chat:delta", {
    session_id: "sched-delete-no-ids",
    text: "retain when backend reports no ids",
  });
  noIdsHarness.handlers.delete_scheduled_task = function () {
    return { id: "automation-no-ids" };
  };
  await noIdsHarness.bridge.deleteScheduledTask("automation-no-ids");
  assert.strictEqual(await noIdsHarness.bridge.openScheduledRunChat({
    id: "run-delete-no-ids",
    automationId: "automation-no-ids",
    sessionId: "sched-delete-no-ids",
    status: "running",
  }, { id: "automation-no-ids", name: "No ids task" }), true);
  assert.ok(
    JSON.stringify(noIdsHarness.bridge.getState().chatItems).includes("retain when backend reports no ids"),
    "a deletion response without deletedSessionIds must not trigger heuristic purging"
  );
}

async function scheduledSessionPersistenceBehavior() {
  var harness = createBridgeHarness();
  var sessionId = "owned-run-session-1";
  await harness.bridge.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId: sessionId, status: "running" },
    { name: "Nightly report" }
  );
  await harness.bridge.exitScheduledRunChat();
  harness.calls.length = 0;
  assert.strictEqual(await harness.bridge.switchToSession(sessionId), true);

  await harness.bridge.renameSession(sessionId, "frontend must not rename this");
  await harness.bridge.cancelGeneration();
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();
  var scheduledCalls = harness.calls.filter(function (call) {
    return call.args && (call.args.id === sessionId || call.args.sessionId === sessionId);
  });
  assert.ok(
    !scheduledCalls.some(function (call) { return call.cmd === "save_session_artifacts"; }),
    "scheduled chat completion and stop must never replace backend-owned artifact paths"
  );
  assert.ok(
    !scheduledCalls.some(function (call) { return call.cmd === "save_session_messages"; }),
    "scheduled transcripts are backend-owned"
  );
  assert.ok(
    !scheduledCalls.some(function (call) { return call.cmd === "rename_session"; }),
    "scheduled titles are backend-owned"
  );
  assert.ok(
    !scheduledCalls.some(function (call) { return call.cmd === "list_workspace_files"; }),
    "scheduled sessions must not run the ordinary frontend artifact reconciliation path"
  );
}

async function scheduledDraftModelBehavior() {
  var harness = createBridgeHarness();
  var capturedInput = null;
  var rejectCreate = true;
  harness.handlers.create_scheduled_task = function (args) {
    if (rejectCreate) throw new Error("cannot create scheduled draft");
    capturedInput = args.input;
    return Object.assign({ id: "automation-created" }, args.input);
  };
  await harness.bridge.init();
  await harness.bridge.startScheduledTaskChat();
  await harness.bridge.sendMessage("Create a report schedule");
  var sessionId = harness.bridge.getState().activeSessionId;
  harness.emit("chat:delta", {
    session_id: sessionId,
    text: "```scheduled-task-draft\n{\"name\":\"Report\",\"prompt\":\"Run report\",\"rrule\":\"FREQ=DAILY\"}\n```",
  });
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();
  assert.strictEqual(harness.bridge.getState().scheduledTaskDraft, null, "chat-generated parameters must not create a confirmation-card state");
  assert.ok(String(harness.bridge.getState().scheduledTaskError).includes("cannot create scheduled draft"));
  assert.ok(
    harness.bridge.getState().chatItems.some(function (item) {
      return item.type === "system" && String(item.text || "").includes("cannot create scheduled draft");
    }),
    "automatic creation failures must remain visible in the creation chat"
  );
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "create_scheduled_task"; }).length,
    1,
    "a valid chat-generated definition should attempt creation immediately"
  );

  rejectCreate = false;
  await harness.bridge.startScheduledTaskChat();
  await harness.bridge.sendMessage("Create the edited report schedule");
  sessionId = harness.bridge.getState().activeSessionId;
  harness.emit("chat:delta", {
    session_id: sessionId,
    text: "```scheduled-task-draft\n{\"name\":\"Edited report\",\"prompt\":\"Run the edited report\",\"rrule\":\"FREQ=DAILY\",\"cwds\":[\"D:/workspace\"],\"mode\":\"plan\",\"allowShell\":true}\n```",
  });
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();
  assert.ok(capturedInput, "valid chat-generated parameters should call create_scheduled_task automatically");
  assert.strictEqual(capturedInput.name, "Edited report");
  assert.strictEqual(capturedInput.prompt, "Run the edited report");
  assert.strictEqual(JSON.stringify(capturedInput.cwds), JSON.stringify(["D:/workspace"]));
  assert.strictEqual(capturedInput.mode, "yolo");
  assert.strictEqual(capturedInput.allowShell, true);
  assert.strictEqual(capturedInput.model, "/wire-active");
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "sourceSessionId"));
  assert.strictEqual(harness.bridge.getState().selectedScheduledTaskId, "automation-created");
  assert.strictEqual(harness.bridge.getState().scheduledTaskAutoOpenId, "automation-created");
}

Promise.resolve()
  .then(scheduledRunNavigationBehavior)
  .then(scheduledRunUnreadBehavior)
  .then(openingRunningMarksBusyBeforeHydration)
  .then(followupQueuedUntilScheduledInitialTurnTerminal)
  .then(terminalEventWinsStaleRunningOpen)
  .then(scheduledDoneBeforeBufferCreatesTerminalTombstone)
  .then(failedRunningOpenRollsBackOnlyItsProvisionalBusy)
  .then(concurrentFailedRunningOpensShareRollback)
  .then(scheduledOwnerRegistryIsBoundedAndProtectsLive)
  .then(scheduledBufferLruNeverEvictsLive)
  .then(scheduledRunningHydrationRaceBehavior)
  .then(scheduledUnreadPollingRaceBehavior)
  .then(scheduledFolderPickerBehavior)
  .then(scheduledTemplateSourcePersistenceBehavior)
  .then(scheduledSelectionGenerationBehavior)
  .then(scheduledRefreshDoesNotOverlap)
  .then(scheduledMutationErrorBehavior)
  .then(scheduledDeletePurgesOnlyReportedSessionBuffers)
  .then(scheduledSessionPersistenceBehavior)
  .then(scheduledDraftModelBehavior)
  .then(function () { console.log('PASS scheduled tasks unit'); })
  .catch(function (error) {
    console.error(error && error.stack || error);
    process.exitCode = 1;
  });
