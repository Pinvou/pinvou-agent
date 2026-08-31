#!/usr/bin/env node
/**
 * Static and behavioral regression checks for the scheduled-task frontend shell.
 *
 * Run: node --test pinvou3-app/tests/scheduled_tasks_unit.test.js
 */
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const indexHtml = [
  'app/main.jsx',
  'shared/i18n/zh.js', // 定时任务断言的中文文案在 zh 词典(拆分后)
  'shared/model-options.js',
  'components/layout/NavigationComponents.jsx',
  'features/chat/ChatView.jsx',
  'features/scheduled/ScheduledTasksView.jsx'
].map(file => fs.readFileSync(path.join(__dirname, '..', 'src', file), 'utf8')).join('\n');
const tauriBridgeFeatureNames = [
  'artifact-tracker', 'chat', 'chat-events', 'sessions', 'terminal', 'scheduled', 'monitor', 'settings', 'memory', 'artifacts', 'personas', 'updater',
  'remote-control', 'dependencies', 'voice', 'knowledge-model', 'interaction',
  'multiagent'
];
const bridgeMessages = fs.readFileSync(
  path.join(__dirname, '..', 'src', 'shared', 'bridge-messages.js'),
  'utf8'
);
const tauriBridge = [bridgeMessages,
    ...tauriBridgeFeatureNames.map(name => fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', `${name}.js`), 'utf8')),
    fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge.js'), 'utf8'),
  ].join('\n');
const webBridge = [
  bridgeMessages,
  fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'web', 'bridge', 'turn-terminal.js'), 'utf8'),
  fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'web', 'bridge.js'), 'utf8'),
].join('\n');
const scheduledTasksRust = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'features', 'scheduled', 'tasks.rs'), 'utf8');
// Wave 2 把版本化存储层拆到 stores.rs；read-state 迁移（migrate→default）落该子模块。
const scheduledStoresRust = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'features', 'scheduled', 'stores.rs'), 'utf8');
const enginePoolRust = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'features', 'assistant', 'engine_pool.rs'), 'utf8');
const scheduledTaskPromptRust = scheduledTasksRust.slice(
  scheduledTasksRust.indexOf('const SCHEDULED_TASK_CHAT_PROMPT'),
  scheduledTasksRust.indexOf('pub fn scheduled_automation_root')
);
const scheduledTemplateSource = indexHtml.slice(
  indexHtml.indexOf('const SCHEDULED_TASK_TEMPLATES'),
  indexHtml.indexOf('const ScheduledTasksView')
);
const scheduledViewSource = indexHtml.slice(
  indexHtml.indexOf('const ScheduledTasksView'),
  indexHtml.indexOf('export { ScheduledTasksView }')
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
  /const SCHEDULED_TASKS_ENTRY_ENABLED = true/.test(indexHtml),
  'scheduled-task entry should be enabled after the creation flow is fixed'
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
  /currentView === 'scheduled'\s*&&\s*\(\s*[\s\S]{0,1200}scheduledRunContext[\s\S]{0,800}<ChatView[\s\S]{0,1600}<LazyScheduledTasksView/.test(indexHtml),
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
  !/data-testid="scheduled-list-delete"/.test(indexHtml) &&
  /data-testid="scheduled-detail-delete"/.test(indexHtml),
  'delete action should live in task details instead of the scheduled task list'
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
  /const refresh = \(\) => bridge\.scheduled\.refreshScheduledTaskData\(20\)[\s\S]{0,260}setInterval\([\s\S]{0,120}refresh\(\)[\s\S]{0,120}3000/.test(indexHtml),
  'the three-second fallback must refresh tasks, selected detail, and runs through one bridge transaction'
);
assert.ok(
  /const handleSwitchSession = useCallback\(async \(id\) => \{[\s\S]{0,220}runBrowserUiTransition[\s\S]{0,260}setCurrentView\('chat'\)[\s\S]{0,180}await bridge\.sessions\.switchToSession\(id\)[\s\S]{0,180}!switched \|\| !isCurrent\(\)[\s\S]{0,260}channel: 'session'[\s\S]{0,100}hideMode: 'workspace'[\s\S]{0,80}serialize: true/.test(indexHtml),
  'ordinary session navigation must hide the native browser before publishing the chat route and loading the remote session'
);
assert.ok(
  /async function navigateFromScheduledRun\(nextView[\s\S]{0,520}runBrowserUiTransition[\s\S]{0,260}await bridge\.scheduled\.exitScheduledRunChat\(\)[\s\S]{0,160}!exited \|\| !isCurrent\(\)[\s\S]{0,200}setCurrentView\(nextView\)[\s\S]{0,360}hideMode: bs && bs\.scheduledRunContext[\s\S]{0,80}'workspace'/.test(indexHtml),
  'leaving a scheduled run must hide the native browser before restoring its return session and publishing the next route'
);
assert.ok(
  /onBackScheduledRun=\{\(\) => navigateFromScheduledRun\('scheduled'\)\}/.test(indexHtml),
  'scheduled back navigation must await restoration before committing the Scheduled route'
);
assert.ok(
  /async function startScheduledTaskChat\(\)\s*\{[\s\S]*?(?:var|const|let) prompt = await invoke\("scheduled_task_chat_prompt"\);[\s\S]*?await createNewSession\(\);[\s\S]*?state\.scheduledTaskPendingGuide = prompt;[\s\S]*?prefillComposer\(/s.test(tauriBridge),
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
  /modelId:\s*value\.modelId\s*\?/.test(tauriBridge) &&
    /SCHEDULED_TASK_WRITABLE_FIELDS = \["name", "prompt", "rrule", "model", "modelId", "paused"\]/.test(tauriBridge) &&
    /pub model_id: Option<String>/.test(scheduledTasksRust),
  'scheduled tasks should carry a stable saved-model id through the frontend bridge and backend DTO'
);
assert.ok(
  /function asScheduledTaskDraft\(d\)[\s\S]{0,320}mode:\s*'yolo'/.test(indexHtml) &&
    !/function asScheduledTaskDraft\(d\)[\s\S]{0,320}d\.mode/.test(indexHtml),
  'chat-rendered scheduled drafts should normalize their mode to Yolo immediately'
);
assert.ok(
  /function lockScheduledTaskDraftModel\(draft\)[\s\S]{0,260}draft\.model = draft\.model \|\| \(active && active\.model\)/.test(tauriBridge) &&
    /draft\.modelId = draft\.modelId \|\| \(active && active\.id\)/.test(tauriBridge) &&
    /(?:var|const|let) lockedModelId = state\.scheduledTaskDraft\.modelId \|\| \(active && active\.id\)/.test(tauriBridge),
  'the final draft should lock the active saved model wire name and stable model id before creation'
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
  /scheduled_task:run_updated[\s\S]{0,180}scheduleScheduledRunRefresh\(\)/.test(tauriBridge) &&
    /function scheduleScheduledRunRefresh\([\s\S]{0,900}refreshScheduledTaskData\(20\)[\s\S]{0,320}loadScheduledTaskRecentRuns\(\)/.test(tauriBridge) &&
    /async function init\(\)[\s\S]{0,1200}loadScheduledTasks\(\)\.catch/.test(tauriBridge),
  'run updates should debounce a global task and run refresh regardless of the current page'
);
assert.ok(
  /if \(automationId && runId && runStatus === "completed"\)/.test(tauriBridge) &&
    /fn ensure_scheduled_run_can_be_marked_viewed[\s\S]{0,420}AutomationRunStatus::Completed/.test(scheduledTasksRust),
  'only opening a completed run conversation may persist its viewed state'
);
assert.ok(
  /SCHEDULED_RUN_READ_STATE_SCHEMA_VERSION:\s*u32\s*=\s*2/.test(scheduledTasksRust) &&
    /impl VersionedRegistry for ScheduledRunReadRegistry[\s\S]{0,700}?fn migrate\([\s\S]*?Self::default\(\)/.test(scheduledStoresRust),
  'legacy read receipts must be reset because they may have been written before completion'
);
assert.ok(
  indexHtml.includes('data-testid="scheduled-task-running"') && /task\.isRunning/.test(indexHtml) &&
    indexHtml.includes('data-testid="scheduled-run-running"') && /queued|running/.test(indexHtml),
  'spinners should appear only on running task and run-history rows'
);
assert.ok(
  /data-testid="scheduled-task-summary"/.test(indexHtml) &&
    /nextRunAt/.test(indexHtml) && /scheduledCopy\.nextRun/.test(indexHtml) && /scheduledCopy\.secondsAfter/.test(indexHtml) &&
    /setInterval\([\s\S]{0,160}1000/.test(indexHtml),
  'active task rows should show the exact next run and a live seconds countdown'
);
assert(
  /scheduled-task-next-run/.test(indexHtml) && /font-semibold/.test(indexHtml) && /text-\[#1769B0\]/.test(indexHtml),
  'active task rows should visually distinguish the next run from the schedule label'
);
assert.ok(
  /function scheduleRepeatLabel\(/.test(indexHtml) &&
    /editor\.interval/.test(indexHtml) &&
    /editor\.repeat === 'hourly' \? scheduledCopy\.startTime : scheduledCopy\.time/.test(indexHtml) &&
    /const hasTimeAnchor = fields\.BYHOUR != null \|\| fields\.BYMINUTE != null/.test(indexHtml) &&
    /previousEditor\.hasTimeAnchor/.test(indexHtml) &&
    /placeholder=.*scheduledCopy\.setStart/.test(indexHtml) &&
    !indexHtml.includes("repeat === 'minutely'"),
  'hourly schedules should expose an optional start anchor without migrating legacy rules implicitly'
);
assert.ok(
  !/data-testid="scheduled-detail-pick-folder"/.test(indexHtml) &&
    !/data-testid="scheduled-live-project"/.test(indexHtml) &&
    !/scheduled-workspace-required/.test(indexHtml),
  'the external-directory setting is gone: no folder picker, project field, or workspace-required hint'
);
assert.ok(
    /data-testid="scheduled-filter-tabs"/.test(indexHtml) &&
    /data-testid="scheduled-left-toolbar"/.test(indexHtml) &&
    /data-testid="scheduled-list-intro"/.test(indexHtml) &&
    /\{renderTemplateSuggestions\(\)\}[\s\S]{0,120}(?:<MyTasksSection className="mb-0" \/>|\{MyTasksSection\(\{ className: 'mb-0' \}\)\})/.test(indexHtml) &&
    /const DetailTaskDialog = \(\) => (?:!\(selected && detailForm\) \? null|\(selected && detailForm\) \? renderModal\()/.test(indexHtml) &&
    /const renderModal = node => modalPortalTarget \? createPortal\(node, modalPortalTarget\) : node/.test(indexHtml) &&
    /DetailTaskDialog = \(\) => (?:!\(selected && detailForm\) \? null :|\(selected && detailForm\) \?) ?renderModal\(/.test(indexHtml) &&
    /role="dialog"/.test(indexHtml) &&
    /data-testid="scheduled-detail-toolbar"/.test(indexHtml) &&
    /data-testid="scheduled-detail-close"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-menu"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-menu-popover"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-toggle"/.test(indexHtml) &&
    /flex shrink-0 flex-wrap items-center justify-between/.test(indexHtml) &&
    /data-testid="scheduled-run-now"[\s\S]{0,720}scheduledCopy\.runNow/.test(indexHtml) &&
    /data-testid="scheduled-open-folder"[\s\S]{0,520}scheduledCopy\.openFolder/.test(indexHtml) &&
    !/data-testid="scheduled-detail-cancel"/.test(indexHtml) &&
    /data-testid="scheduled-detail-save"[\s\S]{0,320}scheduledCopy\.save/.test(indexHtml) &&
    /scheduled-detail-delete[\s\S]{0,1400}scheduled-detail-save/.test(indexHtml) &&
    /data-testid="scheduled-detail-delete"/.test(indexHtml) &&
    /data-testid="scheduled-detail-prompt"/.test(indexHtml) &&
    /testId="scheduled-live-model"/.test(indexHtml) &&
    /data-testid="scheduled-detail-settings"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-frequency"/.test(indexHtml),
  'the list home should stay visible while configured-task editing opens in a modal with direct actions'
);
assert.ok(
  !/>权限</.test(indexHtml) &&
    !indexHtml.includes("['allowShell', 'Shell']") &&
    !indexHtml.includes("['trustMode', '信任模式']"),
  'scheduled task details must not expose task-level permission controls'
);
assert.ok(
  /async function openRunChat\(run\)[\s\S]*?bridge\.scheduled\.openScheduledRunChat\(run,\s*detail \|\| selected\)/.test(indexHtml),
  'scheduled run history rows should open the run chat session'
);
assert.ok(
  !/data-testid="scheduled-run-mode"/.test(indexHtml) &&
    !/data-testid="scheduled-live-mode"/.test(indexHtml) &&
    /function scheduledTaskBackendInput\(input\)/.test(tauriBridge) &&
    /(?:var|const|let) backendInput = \{ mode: "yolo" \}/.test(tauriBridge) &&
    (tauriBridge.match(/scheduledTaskBackendInput\(input\)/g) || []).length === 3,
  'scheduled tasks should hide mode controls and force Yolo on every write'
);
assert.ok(
  !/data-testid="scheduled-yolo-mode"/.test(scheduledViewSource) &&
    !scheduledViewSource.includes('执行模式') &&
    scheduledViewSource.includes('testId="scheduled-live-model"') &&
    scheduledViewSource.includes('testId="scheduled-live-repeat"') &&
    scheduledViewSource.includes('testId="scheduled-live-interval"') &&
    scheduledViewSource.includes('testId="scheduled-live-time"'),
  'the detail view should keep model and schedule in one settings card without an execution-mode row'
);
assert.ok(
  /HOURLY_INTERVAL_OPTIONS\s*=\s*Array\.from\(\{ length: 24 \}/.test(indexHtml) &&
    /scheduleEditor\.repeat === 'hourly'[\s\S]{0,500}data-testid="scheduled-live-interval-row"/.test(scheduledViewSource) &&
    /onChange=\{value => editSchedule\('interval', value\)\}/.test(scheduledViewSource),
  'hourly schedules should expose a themed 1-24 hour interval selector'
);
assert.ok(
  /SCHEDULED_TASK_WRITABLE_FIELDS\s*=\s*\["name", "prompt", "rrule", "model", "modelId", "paused"\]/.test(tauriBridge) &&
    !/SCHEDULED_TASK_WRITABLE_FIELDS[^;]*allowShell/.test(tauriBridge) &&
    !/SCHEDULED_TASK_WRITABLE_FIELDS[^;]*trustMode/.test(tauriBridge) &&
    !/SCHEDULED_TASK_WRITABLE_FIELDS[^;]*autoApprove/.test(tauriBridge) &&
    !/SCHEDULED_TASK_WRITABLE_FIELDS[^;]*cwds/.test(tauriBridge),
  'the frontend wire boundary should allow-list task fields and reject permission or directory inputs'
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
    /testId="scheduled-live-model"/.test(indexHtml) &&
    /testId="scheduled-live-repeat"/.test(indexHtml) &&
    /testId="scheduled-live-interval"/.test(indexHtml) &&
    /testId="scheduled-live-day"/.test(indexHtml) &&
    /testId="scheduled-live-time"/.test(indexHtml),
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
  /const iosInsetSurface =/.test(indexHtml) &&
    /data-testid="scheduled-create-settings" className=\{`overflow-visible rounded-\[16px\] \$\{iosInsetSurface\}`\}/.test(indexHtml) &&
    /data-testid="scheduled-detail-settings" className=\{`overflow-visible rounded-\[16px\] \$\{iosInsetSurface\}`\}/.test(indexHtml) &&
    /data-testid="scheduled-detail-actions-group" className=\{`overflow-hidden rounded-\[16px\] \$\{iosInsetSurface\}`\}/.test(indexHtml) &&
    /data-testid="scheduled-run-history-list" className=\{`overflow-hidden rounded-\[12px\] \$\{iosHistorySurface\}`\}/.test(indexHtml) &&
    /fixed z-\[1000\][\s\S]{0,120}rounded-\[12px\] border/.test(indexHtml) &&
    /fixed z-\[1000\][\s\S]{0,120}rounded-\[14px\] border/.test(indexHtml) &&
    /data-testid="scheduled-detail-delete-confirmation"[\s\S]{0,260}rounded-\[14px\] border/.test(indexHtml),
  'embedded scheduled task form groups should not have outer borders, while floating surfaces keep borders'
);
assert.ok(
  /const modelOptions = savedModels\.map\(model => \(\{[\s\S]{0,120}value:\s*model\.id/.test(indexHtml) &&
    /<ScheduledSelect value=\{detailForm\.modelId \|\| ''\} options=\{modelOptions\}/.test(indexHtml) &&
    /modelId:\s*activeModel && activeModel\.id/.test(indexHtml),
  'scheduled model selection should use saved model ids and submit modelId with the wire model'
);
assert.ok(
  /label:\s*selectorMainLabel\(model, t\)/.test(indexHtml) &&
    !/label:\s*model\.name && model\.name !== model\.model \?/.test(indexHtml),
  'scheduled model selection should display the wire model instead of stale display names'
);
assert.ok(
  /footerAction, alwaysCommit = false,/.test(indexHtml) &&
    /if \(alwaysCommit \|\| !active\) onChange\(option\.value\);/.test(indexHtml) &&
    /onChange=\{value => editModel\(value\)\} alwaysCommit/.test(indexHtml) &&
    (indexHtml.match(/alwaysCommit/g) || []).length === 3,
  're-selecting the already-active model option must re-commit the binding: ' +
    'an in-place edited model config keeps its id while its wire name changes, ' +
    'and the engine rejects the stale binding snapshot until the task is re-saved'
);
assert.ok(
  !/data-testid="scheduled-task-pin"/.test(indexHtml) &&
    !/data-testid="scheduled-task-actions"/.test(indexHtml) &&
    !/data-testid="scheduled-task-action-menu"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-actions"/.test(indexHtml) &&
    /scheduledRunHistory\.map/.test(indexHtml) &&
    /<RecentItem[\s\S]{0,900}chat=\{chat\}[\s\S]{0,900}handleOpenScheduledRunShortcut\(chat\.scheduledRun\)/.test(indexHtml) &&
    /onContextMenu=\{openContextMenu\}/.test(indexHtml) &&
    /onTogglePinned && onTogglePinned\(chat\.id, !chat\.pinned\)/.test(indexHtml) &&
    /setConfirming\(true\)/.test(indexHtml) &&
    /renameSession\(id, title\)/.test(indexHtml),
  'scheduled run record operations belong to the sidebar RecentItem, not the scheduled task definition list'
);
assert.ok(
    /multiple = false, minSelected = 0/.test(indexHtml) &&
    /aria-multiselectable=\{multiple \|\| undefined\}/.test(indexHtml) &&
    /const lastRequiredSelection = multiple && active && selectedValues\.length <= minSelected/.test(indexHtml) &&
    /onChange=\{values => editSchedule\('days', values\)\} multiple minSelected=\{1\}/.test(indexHtml) &&
    /onClose=\{\(\) => setScheduleRepeatIntent\(null\)\}/.test(indexHtml) &&
    /WEEKDAY_CODES\.filter\(day => requested\.has\(day\)\)/.test(indexHtml),
  'weekly schedules should support an ordered one-to-seven day multi-select, reject empty selections, and normalize presets after the menu closes'
);
assert.ok(
  /const ScheduledTimeWheel =/.test(indexHtml) &&
    /scrollSnapType: 'y mandatory'/.test(indexHtml) &&
    /const WheelColumn = [\s\S]{0,1800}\}, \[value\]\);/.test(indexHtml) &&
    !/type="time"/.test(indexHtml) &&
    !indexHtml.includes('独立会话'),
  'time editing should use the iOS-style wheel picker and the detail panel drops the static session row'
);
assert.ok(
  /setSaveState\(Object\.keys\(pendingPatchRef\.current\)\.length \? 'editing' : 'saved'\)/.test(indexHtml) &&
    /const failureIsCurrent = Object\.keys\(payload\)\.some/.test(indexHtml) &&
    /mountedRef\.current && failureIsCurrent/.test(indexHtml),
  'an older autosave completion must not flash an error over newer pending edits'
);
assert.ok(
  /editable=\{!busy && !isMultiAgentReadOnly && item\.id === lastUserId\}/.test(indexHtml) &&
    !/async function editLastTurn\(newText\)[\s\S]{0,420}isScheduledRunSession\(state\.activeSessionId\)\) return false/.test(tauriBridge) &&
    !indexHtml.includes('定时运行使用创建时锁定的模型'),
  'a scheduled run opened from history should use the ordinary chat editor and composer controls'
);
assert.ok(
  /function startTemplate\(template\)[\s\S]{0,900}setCreateForm\(\{[\s\S]{0,260}templateId:\s*template\.id[\s\S]{0,260}name:\s*template\.name[\s\S]{0,260}prompt:\s*template\.prompt[\s\S]{0,260}rrule:\s*template\.rrule/.test(indexHtml) &&
    !/function saveDraft\(/.test(indexHtml),
  'clicking a template should open the second-level creation sheet with template fields prefilled'
);
assert.ok(
  /data-testid="scheduled-create-dialog"/.test(indexHtml) &&
    /data-testid="scheduled-create-close"/.test(indexHtml) &&
    /data-testid="scheduled-create-name"/.test(indexHtml) &&
    /data-testid="scheduled-create-prompt"/.test(indexHtml) &&
    /testId="scheduled-create-repeat"/.test(indexHtml) &&
    /data-testid="scheduled-create-submit"/.test(indexHtml) &&
    /<span[^>]*>\{scheduledCopy\.taskName\}<\/span>/.test(indexHtml) &&
    /disabled=\{!!busyAction \|\| !String\(createForm\.name/.test(indexHtml) &&
    /async function startBlankTask\(\)[\s\S]{0,1200}setCreateForm\(/.test(indexHtml) &&
    /selectAfterCreate:\s*false/.test(indexHtml) &&
    /async function submitCustomTask\(event\)[\s\S]{0,1600}bridge\.scheduled\.createScheduledTask\(/.test(indexHtml),
  'custom creation should collect a valid task in a dialog before creating it'
);
assert.ok(
  /(?:var|const|let) selectAfterCreate = !input \|\| input\.selectAfterCreate !== false/.test(tauriBridge) &&
    /if \(!created \|\| !created\.id\)/.test(tauriBridge) &&
    /if \(selectAfterCreate\) selectScheduledTask\(created\.id\)/.test(tauriBridge) &&
    /if \(selectAfterCreate\) state\.scheduledTaskDetail = created/.test(tauriBridge),
  'scheduled creation dialogs should be able to create without immediately opening the edit sheet'
);
assert.ok(
  /fn should_sync_session\(_is_scheduled: bool, _has_messages: bool\)[\s\S]{0,520}\n\s*true\s*\n}/.test(enginePoolRust) &&
    /should_sync_session\(is_scheduled, !saved\.messages\.is_empty\(\)\)/.test(enginePoolRust),
  'every Session must SyncSession even when its durable message list is empty'
);
assert.ok(
  tauriBridge.includes('preserveInterruptedAssistantPresentation') &&
    webBridge.includes('preserveInterruptedAssistantPresentation') &&
    tauriBridge.includes('item.interruptedDisplayOnly = true') &&
    webBridge.includes('item.interruptedDisplayOnly = true'),
  'desktop and Web bridges must share the display-only interrupted response behavior'
);
assert.ok(
  /provenance === "runtime" \|\| provenance === "subagent_handoff"|\["runtime", "subagent_handoff"(?:, "[a-z_]+")*\]\.includes\(provenance\)/.test(tauriBridge) &&
    /provenance === "runtime" \|\| provenance === "subagent_handoff"|\["runtime", "subagent_handoff"(?:, "[a-z_]+")*\]\.includes\(provenance\)/.test(webBridge),
  'desktop and Web bridges must both hide internal runtime and sub-agent handoff messages'
);
assert.ok(
  tauriBridge.includes('!snapshotAlreadyCoversTurn && !hideInternalRuntimeMessage') &&
    webBridge.includes('!snapshotAlreadyCoversTurn && !hideInternalRuntimeMessage'),
  'desktop and Web live event paths must not render internal runtime messages'
);
assert.ok(
  /请一次只问我一个问题[\s\S]*1\.[\s\S]*2\./.test(scheduledTaskPromptRust) &&
    !/\n3\./.test(scheduledTaskPromptRust) &&
    !scheduledTaskPromptRust.includes('autoApprove') &&
    scheduledTaskPromptRust.includes('不需要询问工作目录或权限设置') &&
    !scheduledTaskPromptRust.includes('allowShell') &&
    !scheduledTaskPromptRust.includes('trustMode') &&
    !scheduledTaskPromptRust.includes('cwds'),
  'backend prompt should include the guided-chat checklist without approval or workspace questions'
);
assert.ok(
  scheduledTaskPromptRust.includes("FREQ=HOURLY;INTERVAL=6") &&
    scheduledTaskPromptRust.includes("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR,SA,SU;BYHOUR=8;BYMINUTE=30") &&
    scheduledTaskPromptRust.includes("FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30"),
  'backend prompt should include supported rrule examples'
);
assert.ok(
  scheduledTaskPromptRust.includes("create_scheduled_task") &&
    scheduledTaskPromptRust.includes("schtasks") &&
    scheduledTaskPromptRust.includes("Windows Task Scheduler") &&
    scheduledTaskPromptRust.includes("cron") &&
    scheduledTaskPromptRust.includes("systemd timer") &&
    scheduledTaskPromptRust.includes("不支持分钟级") &&
    !scheduledTaskPromptRust.includes("FREQ=MINUTELY"),
  'backend prompt should forbid system schedulers and ask before unsupported minute-level schedules'
);
mustNotContain("每日项目状态提醒");
mustNotContain("每周资料整理提醒");
assert.ok(!scheduledViewSource.includes('>Templates<'), 'the obsolete Templates placeholder must not return');
mustNotContain("模板库会在后续接入");
mustNotContain('title="编辑"');
assert.strictEqual((scheduledTemplateSource.match(/rrule:\s*'FREQ=/g) || []).length, 3, 'the suggestion area should contain exactly three templates');
assert.strictEqual((scheduledTemplateSource.match(/mode:\s*'(?:agent|plan|yolo)'/g) || []).length, 0, 'templates should not expose a selectable execution mode');
assert.strictEqual((scheduledTemplateSource.match(/allowShell|trustMode/g) || []).length, 0, 'templates must not expose task-level permission settings');
assert.strictEqual((scheduledTemplateSource.match(/autoApprove/g) || []).length, 0, 'approval is fixed to YOLO in the backend; the frontend must not expose or send autoApprove');
assert.strictEqual((scheduledTemplateSource.match(/paused:\s*false/g) || []).length, 3, 'templates activate immediately: no workspace prerequisite remains');
assert.strictEqual((scheduledTemplateSource.match(/workspace|cwds/g) || []).length, 0, 'templates must not carry a workspace concept');
assert.ok(
  /name: '每日早报'/.test(scheduledTemplateSource) &&
    /name: '事项督办'/.test(scheduledTemplateSource) &&
    /name: '工作周报'/.test(scheduledTemplateSource),
  'the suggestion area should use the three office-oriented task names'
);
assert.ok(
  (scheduledTemplateSource.match(/不要扫描用户目录/g) || []).length === 3 &&
    /仅查询整理，不发送、审批或修改/.test(scheduledTemplateSource) &&
    /不要扫描用户目录或自动发送/.test(scheduledTemplateSource),
  'office templates should be source-driven, read-only, and independent of user directories'
);
assert.ok(
  (scheduledTemplateSource.match(/description:\s*'/g) || []).length === 3 &&
    /\{template\.description\}/.test(indexHtml) &&
    !/>\{template\.prompt\}<\/span>/.test(indexHtml),
  'suggestion cards should show concise descriptions instead of full execution prompts'
);
assert.ok(
  /name: '每日早报'[\s\S]{0,500}重要新闻和行业动态[\s\S]{0,180}公司公告/.test(scheduledTemplateSource) &&
    !/name: '每日早报'[\s\S]{0,500}今日会议|name: '每日早报'[\s\S]{0,500}补充今日[^。']*待办/.test(scheduledTemplateSource),
  'the daily brief should own information awareness while action items remain in supervision'
);
assert.ok(
  !scheduledTasksRust.includes('requires a workspace') &&
    !scheduledTasksRust.includes('active_without_workspace'),
  'the backend workspace gate is gone: the shared workspace is assigned internally'
);
assert.ok(
  !scheduledTemplateSource.includes("id: 'project-health'") && !scheduledTemplateSource.includes("id: 'material-digest'"),
  'only the three Codex-style suggested templates should remain'
);
assert.ok(
  !/选定[^']*(项目|目录)/.test(scheduledTemplateSource) &&
    /待办|未完成/.test(scheduledTemplateSource) && /风险/.test(scheduledTemplateSource),
  'template prompts should not reference a selected project directory'
);
assert.ok(
  /function startTemplate\(template\)[\s\S]{0,900}setCreateForm\(\{[\s\S]{0,260}templateId:\s*template\.id/.test(indexHtml) &&
    /async function submitCustomTask\(event\)[\s\S]{0,1800}bridge\.scheduled\.createScheduledTask\([\s\S]{0,420}templateId:\s*createForm\.templateId \|\| undefined[\s\S]{0,420}mode:\s*'yolo'/.test(indexHtml) &&
    !/scheduled-detail-settings[\s\S]{0,1200}>权限</.test(indexHtml),
  'selecting a template should confirm through the second-level sheet and create with fixed Yolo mode and no permission UI'
);
assert.ok(
  /const visibleSuggestions\s*=\s*SCHEDULED_TASK_TEMPLATES(?:;|\.map\()/.test(indexHtml) &&
    !/const visibleSuggestions\s*=\s*SCHEDULED_TASK_TEMPLATES\.filter/.test(indexHtml) &&
    /visibleSuggestions\.map\(template/.test(indexHtml),
  'suggested templates should remain visible after users create matching scheduled tasks'
);
assert.ok(
  /scheduled-task-template-sources-v1/.test(tauriBridge) &&
    /(?:var|const|let) templateId = input && typeof input\.templateId === "string"/.test(tauriBridge) &&
    !/SCHEDULED_TASK_WRITABLE_FIELDS[^;]*templateId/.test(tauriBridge) &&
    /templateId:\s*template\.id/.test(indexHtml),
  'template source ids should persist in the frontend sidecar without leaking into the base automation request'
);

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise(function (res, rej) {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function tick() {
  return new Promise(function (resolve) { setImmediate(resolve); });
}

function createBridgeHarness(sharedStorage, runtimeOptions) {
  runtimeOptions = runtimeOptions || {};
  const bridgeKind = runtimeOptions.bridgeKind === "web" ? "web" : "tauri";
  const listeners = Object.create(null);
  const windowListeners = Object.create(null);
  const handlers = Object.create(null);
  const calls = [];
  const dialogCalls = [];
  let dialogResult = null;
  let createdSession = 0;
  let structuredCloneCalls = 0;
  const storageData = sharedStorage || Object.create(null);
  let webSupportedCommands = new Set(runtimeOptions.webSupportedCommands || []);
  let webCapabilitiesReady = runtimeOptions.webCapabilitiesReady !== false;
  let webConnectionState = Object.assign({ status: "connected", desktop_online: true }, runtimeOptions.webConnectionState || {});
  let webConnectionStateReader = null;
  const storage = {
    getItem: function (key) { return Object.prototype.hasOwnProperty.call(storageData, key) ? storageData[key] : null; },
    setItem: function (key, value) { storageData[key] = String(value); },
    removeItem: function (key) { delete storageData[key]; },
  };
  const document = {
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
    if (cmd === "list_sessions" || cmd === "web_access_list_sessions" ||
        cmd === "list_archived_sessions" || cmd === "web_access_list_archived_sessions" ||
        cmd === "list_personas" ||
        cmd === "get_session_persona_events" || cmd === "get_session_pinvou_reviews" ||
        cmd === "get_session_timeline" ||
        cmd === "list_workspace_files" || cmd === "list_scheduled_task_runs" ||
        cmd === "list_scheduled_runs") return [];
    if (cmd === "get_mode_state") return { mode: "yolo" };
    if (cmd === "get_memory_overview") return {};
    if (cmd === "session_mounted_collections_snapshot") return { revision: 0, collections: [] };
    if (cmd === "session_mounted_collections") return [];
    if (cmd === "session_mounted_collection" || cmd === "get_active_persona" ||
        cmd === "find_resumable_run" || cmd === "check_for_update") return null;
    if (cmd === "get_settings") return { theme: "genesis", language: "zh-Hans" };
    if (cmd === "get_backend_status") return {};
    if (cmd === "scheduled_task_chat_prompt") return "scheduled guide";
    if (cmd === "read_scheduled_task") return { id: args.id, name: args.id };
    if (cmd === "create_scheduled_task") {
      return Object.assign({ id: "automation-created" }, args.input || {});
    }
    if (cmd === "set_scheduled_task_pinned") {
      return { id: args.id, name: args.id, pinned: !!args.pinned, pinnedAt: args.pinned ? "2026-07-15T10:00:00Z" : null };
    }
    return null;
  }

  function invoke(cmd, args) {
    calls.push({ cmd, args: args || null });
    try {
      let handler = handlers[cmd];
      if (!handler && cmd === "web_access_list_sessions") handler = handlers.list_sessions;
      if (!handler && cmd === "web_access_list_archived_sessions") {
        handler = handlers.list_archived_sessions;
      }
      if (handler) return Promise.resolve(handler(args || {}));
      if (cmd === "web_access_load_session_chunk") {
        const saved = handlers.load_session
          ? handlers.load_session({ id: args.id })
          : defaultInvoke("load_session", { id: args.id });
        const encoded = Buffer.from(JSON.stringify(saved), "utf8");
        const offset = Number(args.offset || 0);
        return Promise.resolve({
          download_id: args.downloadId || args.requestedDownloadId || "test-download-" + args.id,
          offset,
          total: encoded.length,
          data_base64: encoded.subarray(offset).toString("base64"),
          eof: true,
        });
      }
      return Promise.resolve(defaultInvoke(cmd, args || {}));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  const window = {
    __TAURI__: {
      core: { invoke },
      event: {
        listen: function (name, fn) {
          if (!listeners[name]) listeners[name] = [];
          listeners[name].push(fn);
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
    addEventListener: function (name, listener) {
      if (!windowListeners[name]) windowListeners[name] = [];
      windowListeners[name].push(listener);
    },
    removeEventListener: function (name, listener) {
      windowListeners[name] = (windowListeners[name] || []).filter(function (candidate) {
        return candidate !== listener;
      });
    },
    dispatchEvent: function (event) {
      [...(windowListeners[event.type] || [])].forEach(function (listener) { listener(event); });
      return true;
    },
    localStorage: storage,
    sessionStorage: storage,
    location: { search: "" },
    atob: function (value) { return Buffer.from(String(value), "base64").toString("binary"); },
    btoa: function (value) { return Buffer.from(String(value), "binary").toString("base64"); },
  };
  if (bridgeKind === "web") {
    window.PinvouPlatform = {
      kind: "web",
      isWeb: true,
      capabilities: {},
      can: function () { return false; },
      canInvoke: function (command) { return webCapabilitiesReady && webSupportedCommands.has(command); },
      areInvokeCapabilitiesReady: function () { return webCapabilitiesReady; },
      getConnectionState: function () {
        return webConnectionStateReader ? webConnectionStateReader(webConnectionState) : webConnectionState;
      },
    };
  }
  window.window = window;
  window.document = document;
  const context = {
    window,
    document,
    localStorage: storage,
    sessionStorage: storage,
    console: { log: function () {}, warn: function () {}, error: function () {} },
    setTimeout: runtimeOptions.setTimeout || setTimeout,
    clearTimeout: runtimeOptions.clearTimeout || clearTimeout,
    setInterval: function () { return 0; },
    clearInterval: function () {},
    structuredClone: function (value) {
      structuredCloneCalls += 1;
      return JSON.parse(JSON.stringify(value));
    },
    TextDecoder,
    Uint8Array,
  };
  vm.runInNewContext(
    bridgeKind === "web" ? webBridge : tauriBridge,
    context,
    { filename: bridgeKind + "-bridge.js" }
  );

  const rawBridge = window.TauriBridge;
  const bridge = bridgeKind === "web" ? {
    sessions: {
      switchToSession: function (id) { return rawBridge.switchToSession(id); },
      createNewSession: function () { return rawBridge.createNewSession(); },
    },
    chat: {
      sendMessage: function (text, meta) { return rawBridge.sendMessage(text, meta); },
    },
    interaction: {
      editLastTurn: function (text) { return rawBridge.editLastTurn(text); },
    },
    scheduled: {
      openScheduledRunChat: function (run, task) { return rawBridge.openScheduledRunChat(run, task); },
    },
    state: {
      get: function () { return rawBridge.getState(); },
      getMany: function () { return rawBridge.getState(); },
    },
  } : rawBridge;

  return {
    bridge,
    bridgeKind,
    handlers,
    calls,
    storageData,
    dialogCalls,
    getStructuredCloneCalls: function () { return structuredCloneCalls; },
    setDialogResult: function (value) { dialogResult = value; },
    emit: function (name, payload) {
      assert.ok(listeners[name] && listeners[name].length, "expected listener " + name);
      const event = { payload: payload || {} };
      return Promise.all(listeners[name].map(function (listener) { return listener(event); }));
    },
    setWebCapabilities: function (commands) {
      webSupportedCommands = new Set(commands || []);
      webCapabilitiesReady = true;
      window.dispatchEvent({
        type: "pinvou:web-capabilities",
        detail: { commands: [...webSupportedCommands], events: [] },
      });
    },
    setWebConnection: function (state) {
      webConnectionState = Object.assign({}, webConnectionState, state || {});
      window.dispatchEvent({ type: "pinvou:web-connection", detail: webConnectionState });
    },
    setWebConnectionStateReader: function (reader) {
      webConnectionStateReader = typeof reader === "function" ? reader : null;
    },
  };
}

async function interruptedWebSessionDownloadReleasesItsLease() {
  const sharedStorage = Object.create(null);
  const harness = createBridgeHarness(sharedStorage, {
    bridgeKind: "web",
    webSupportedCommands: ["web_access_cancel_session_download"],
  });
  const sessionId = "chat-download-cancel";
  const saved = {
    metadata: { id: sessionId, title: "Download cancel", message_count: 0 },
    messages: [],
    artifacts: [],
  };
  const encoded = Buffer.from(JSON.stringify(saved), "utf8");
  let chunkCalls = 0;
  let interruptedDownloadId = "";
  harness.handlers.web_access_load_session_chunk = function (args) {
    chunkCalls += 1;
    if (chunkCalls === 1) {
      interruptedDownloadId = args.requestedDownloadId;
      assert.ok(interruptedDownloadId && interruptedDownloadId.indexOf("download_web_") === 0);
      const first = encoded.subarray(0, Math.max(1, Math.floor(encoded.length / 2)));
      return {
        download_id: interruptedDownloadId,
        offset: 0,
        total: encoded.length,
        data_base64: first.toString("base64"),
        eof: false,
      };
    }
    if (chunkCalls === 2) {
      harness.setWebConnection({ status: "desktop_offline", desktop_online: false });
      throw new Error("relay connection dropped");
    }
    return {
      download_id: args.requestedDownloadId,
      offset: 0,
      total: encoded.length,
      data_base64: encoded.toString("base64"),
      eof: true,
    };
  };
  let cancelCalls = 0;
  harness.handlers.web_access_cancel_session_download = function (args) {
    cancelCalls += 1;
    assert.strictEqual(args.id, sessionId);
    assert.strictEqual(args.downloadId, interruptedDownloadId);
    if (cancelCalls === 1) throw new Error("relay still disconnected");
    return true;
  };

  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), false);
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "web_access_cancel_session_download"; }).length,
    1,
    "an interrupted chunk transfer must attempt to release its desktop lease immediately"
  );
  assert.ok(sharedStorage["pinvou.web_session_download_leases.v1"],
    "a failed cancellation must remain durable for the reconnect attempt");
  harness.setWebConnection({ status: "connected", desktop_online: true });
  await tick();
  await tick();
  assert.strictEqual(cancelCalls, 2, "the connection event must retry the abandoned lease cancellation");
  assert.strictEqual(sharedStorage["pinvou.web_session_download_leases.v1"], undefined);
  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);

  sharedStorage["pinvou.web_session_download_leases.v1"] = JSON.stringify([{
    download_id: "download_reload_12345678",
    session_id: "chat-download-reload",
  }]);
  const reloaded = createBridgeHarness(sharedStorage, {
    bridgeKind: "web",
    webSupportedCommands: ["web_access_cancel_session_download"],
  });
  reloaded.handlers.web_access_cancel_session_download = function () { return false; };
  assert.strictEqual(await reloaded.bridge.sessions.switchToSession("chat-download-reload"), true);
  const transferCalls = reloaded.calls.filter(function (call) {
    return call.cmd === "web_access_cancel_session_download" || call.cmd === "web_access_load_session_chunk";
  });
  assert.deepStrictEqual(
    transferCalls.slice(0, 2).map(function (call) { return call.cmd; }),
    ["web_access_cancel_session_download", "web_access_load_session_chunk"],
    "a page reload must cancel its persisted abandoned lease before opening a replacement download"
  );
  assert.strictEqual(sharedStorage["pinvou.web_session_download_leases.v1"], undefined);
}

async function mismatchedEchoedDownloadIdCancelsBothLeases() {
  const sharedStorage = Object.create(null);
  const harness = createBridgeHarness(sharedStorage, {
    bridgeKind: "web",
    webSupportedCommands: ["web_access_cancel_session_download"],
  });
  const sessionId = "chat-download-id-mismatch";
  let requestedId = "";
  const echoedId = "download_server_ignored_requested";
  harness.handlers.web_access_load_session_chunk = function (args) {
    requestedId = args.requestedDownloadId;
    const encoded = Buffer.from(JSON.stringify({
      metadata: { id: sessionId, title: "Mismatch", message_count: 0 },
      messages: [],
      artifacts: [],
    }), "utf8");
    return {
      download_id: echoedId,
      offset: 0,
      total: encoded.length,
      data_base64: encoded.toString("base64"),
      eof: true,
    };
  };
  const cancelled = [];
  harness.handlers.web_access_cancel_session_download = function (args) {
    cancelled.push(args.downloadId);
    assert.strictEqual(args.id, sessionId);
    return true;
  };

  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), false);
  assert.ok(requestedId && requestedId !== echoedId);
  assert.deepStrictEqual(cancelled.sort(), [requestedId, echoedId].sort(), // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic order of a string array is exactly the assertion's expectation
    "a protocol mismatch must release both the requested and desktop-echoed leases");
  assert.strictEqual(sharedStorage["pinvou.web_session_download_leases.v1"], undefined);
}

async function lostFirstWebSessionChunkStillHasCancellableLease() {
  const sharedStorage = Object.create(null);
  const harness = createBridgeHarness(sharedStorage, {
    bridgeKind: "web",
    webSupportedCommands: ["web_access_cancel_session_download"],
  });
  const sessionId = "chat-download-first-response-lost";
  let requestedId = "";
  let loadCalls = 0;
  harness.handlers.web_access_load_session_chunk = function (args) {
    loadCalls += 1;
    if (loadCalls === 1) {
      requestedId = args.requestedDownloadId;
      throw new Error("first Relay response was lost");
    }
    const saved = {
      metadata: { id: sessionId, title: "Recovered", message_count: 0 },
      messages: [],
      artifacts: [],
    };
    const encoded = Buffer.from(JSON.stringify(saved), "utf8");
    return {
      download_id: args.requestedDownloadId,
      offset: 0,
      total: encoded.length,
      data_base64: encoded.toString("base64"),
      eof: true,
    };
  };
  let cancelCalls = 0;
  harness.handlers.web_access_cancel_session_download = function (args) {
    cancelCalls += 1;
    assert.strictEqual(args.downloadId, requestedId);
    if (cancelCalls === 1) throw new Error("connection unavailable for cancellation");
    return true;
  };

  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), false);
  assert.ok(requestedId, "the browser must choose and persist the lease id before the first RPC");
  assert.ok(sharedStorage["pinvou.web_session_download_leases.v1"]);
  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);
  assert.strictEqual(cancelCalls, 2);
  assert.strictEqual(sharedStorage["pinvou.web_session_download_leases.v1"], undefined);
}

async function subscriptionSnapshotsAvoidTranscriptDeepClone() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const chatUpdates = [];
  const combinedUpdates = [];
  const unsubscribeChat = bridge.state.subscribe("chat", function (snapshot) {
    chatUpdates.push(snapshot);
  });
  const unsubscribeCombined = bridge.state.subscribeMany(["sessions", "chat"], function (snapshot) {
    combinedUpdates.push(snapshot);
  });
  const cloneCallsBeforeNotify = harness.getStructuredCloneCalls();

  await bridge.sessions.createNewSession();

  assert.strictEqual(chatUpdates.length, 1, "one bridge notification should publish one chat subscription update");
  assert.strictEqual(combinedUpdates.length, 1, "one bridge notification should publish one combined subscription update");
  assert.strictEqual(
    harness.getStructuredCloneCalls(),
    cloneCallsBeforeNotify,
    "subscription notifications must not deep-clone the transcript"
  );
  assert.ok(Object.isFrozen(chatUpdates[0]) && Object.isFrozen(combinedUpdates[0]),
    "subscription envelopes should be read-only");

  assert.throws(function () {
    chatUpdates[0].chatItems.push({ type: "system", text: "subscriber-only" });
  }, /not extensible|read only|frozen/i, "subscription array containers should be read-only");
  assert.strictEqual(bridge.state.get("chat").chatItems.length, 0,
    "subscription array containers must not mutate bridge state");
  const detached = bridge.state.get("chat");
  detached.thinking.active = true;
  assert.strictEqual(bridge.state.get("chat").thinking.active, false,
    "get/getMany must retain their defensive deep-copy contract");

  unsubscribeChat();
  unsubscribeCombined();
}

async function reentrantSubscriptionNotificationsStayOrdered() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const firstOrder = [];
  const secondOrder = [];
  const unsubscribeFirst = bridge.state.subscribe("chat", function (snapshot) {
    const text = snapshot.composerPrefill.text;
    firstOrder.push(text);
    if (text === "outer") bridge.chat.prefillComposer("nested");
  });
  const unsubscribeSecond = bridge.state.subscribe("chat", function (snapshot) {
    secondOrder.push(snapshot.composerPrefill.text);
  });

  bridge.chat.prefillComposer("outer");
  assert.deepStrictEqual(firstOrder, ["outer", "nested"], "the first subscriber should receive queued revisions in order");
  assert.deepStrictEqual(secondOrder, ["outer", "nested"], "a later subscriber must not observe nested before outer");
  assert.strictEqual(bridge.state.get("chat").composerPrefill.text, "nested");
  unsubscribeFirst();
  unsubscribeSecond();

  const membershipHarness = createBridgeHarness();
  const membershipBridge = membershipHarness.bridge;
  const membershipFirst = [];
  const membershipSecond = [];
  const membershipAdded = [];
  // eslint-disable-next-line prefer-const -- kept for the test stub / post-declaration assignment
  let unsubscribeMembershipSecond;
  let unsubscribeAdded = function () {};
  const unsubscribeMembershipFirst = membershipBridge.state.subscribe("chat", function (snapshot) {
    const text = snapshot.composerPrefill.text;
    membershipFirst.push(text);
    if (text === "membership-outer") {
      unsubscribeMembershipSecond();
      unsubscribeAdded = membershipBridge.state.subscribe("chat", function (next) {
        membershipAdded.push(next.composerPrefill.text);
      });
      membershipBridge.chat.prefillComposer("membership-nested");
    }
  });
  unsubscribeMembershipSecond = membershipBridge.state.subscribe("chat", function (snapshot) {
    membershipSecond.push(snapshot.composerPrefill.text);
  });

  membershipBridge.chat.prefillComposer("membership-outer");
  assert.deepStrictEqual(membershipFirst, ["membership-outer", "membership-nested"]);
  assert.deepStrictEqual(membershipSecond, ["membership-outer"],
    "unsubscribe during a round should apply only to rounds queued afterwards");
  assert.deepStrictEqual(membershipAdded, ["membership-nested"],
    "subscribe during a round should apply only to rounds queued afterwards");
  unsubscribeMembershipFirst();
  unsubscribeAdded();

  const errorHarness = createBridgeHarness();
  const errorBridge = errorHarness.bridge;
  const interruptedSecond = [];
  const unsubscribeThrowing = errorBridge.state.subscribe("chat", function (snapshot) {
    if (snapshot.composerPrefill.text === "error-outer") {
      errorBridge.chat.prefillComposer("discarded-nested");
      throw new Error("subscriber failed");
    }
  });
  const unsubscribeInterruptedSecond = errorBridge.state.subscribe("chat", function (snapshot) {
    interruptedSecond.push(snapshot.composerPrefill.text);
  });
  assert.throws(function () { errorBridge.chat.prefillComposer("error-outer"); }, /subscriber failed/,
    "subscriber errors should retain their synchronous propagation behavior");
  assert.deepStrictEqual(interruptedSecond, [], "a callback error should interrupt the current subscriber round");
  unsubscribeThrowing();
  unsubscribeInterruptedSecond();

  const recovered = [];
  const unsubscribeRecovered = errorBridge.state.subscribe("chat", function (snapshot) {
    recovered.push(snapshot.composerPrefill.text);
  });
  errorBridge.chat.prefillComposer("recovered");
  assert.deepStrictEqual(recovered, ["recovered"],
    "queued revisions from an interrupted callback must be discarded before the next notification");
  unsubscribeRecovered();
}

async function persistentSubscriptionSnapshotsPreserveJsonEdges() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const snapshots = [];
  let negativeZero = true;
  function settingsResult() {
    const result = JSON.parse('{"language":"en","__proto__":{"marker":"own-value"}}');
    Object.defineProperty(result, "nan", { enumerable: true, value: NaN, writable: true });
    result.zero = negativeZero ? -0 : 0;
    return result;
  }
  harness.handlers.update_settings = settingsResult;
  harness.handlers.get_effective_model_config = function () { return null; };
  const unsubscribe = bridge.state.subscribe("settings", function (snapshot) { snapshots.push(snapshot); });

  assert.strictEqual(await bridge.settings.saveSettings({ language: "en" }), true);
  assert.ok(snapshots.length >= 2, "settings save should publish its effective-model and saved revisions");
  const first = snapshots[0];
  const repeated = snapshots[1];
  assert.ok(Object.prototype.hasOwnProperty.call(first.settings, "__proto__"));
  assert.strictEqual(first.settings["__proto__"].marker, "own-value");
  assert.strictEqual(Object.getPrototypeOf(first.settings), Object.getPrototypeOf(first),
    "snapshots with own __proto__ data must retain the default object prototype");
  assert.strictEqual(Object.getPrototypeOf(first.settings).marker, undefined, "snapshot prototypes must not be polluted");
  assert.strictEqual(first.settings, repeated.settings, "Object.is should reuse an unchanged subtree containing NaN");
  assert.ok(Number.isNaN(first.settings.nan));
  assert.ok(Object.is(first.settings.zero, -0));

  negativeZero = false;
  assert.strictEqual(await bridge.settings.saveSettings({ language: "en" }), true);
  const changed = snapshots[snapshots.length - 1];
  assert.notStrictEqual(changed.settings, repeated.settings, "Object.is should distinguish -0 from +0");
  assert.ok(Object.is(changed.settings.zero, 0));
  assert.ok(Object.prototype.hasOwnProperty.call(changed.settings, "__proto__"),
    "copy-on-write updates must retain an existing own __proto__ value");
  assert.strictEqual(changed.settings["__proto__"].marker, "own-value");
  assert.strictEqual(Object.getPrototypeOf(changed.settings).marker, undefined,
    "copy-on-write updates must not route __proto__ through the prototype setter");
  unsubscribe();
}

async function persistentSubscriptionSnapshotsRejectUnsupportedValues() {
  async function rejects(result, pattern) {
    const harness = createBridgeHarness();
    harness.handlers.ingest_file = function () { return result; };
    const unsubscribe = harness.bridge.state.subscribe("chat", function () {});
    await assert.rejects(harness.bridge.attachments.addAttachmentByPath("C:\\snapshot-edge.txt"), pattern);
    unsubscribe();
  }
  await rejects(new Date(0), /only supports arrays and plain objects/);
  const cyclic = { value: "cycle" };
  cyclic.self = cyclic;
  await rejects(cyclic, /must not contain cycles/);
}

async function longSessionStreamingAvoidsPerDeltaDeepClone() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-long-stream";
  const messages = [
    {
      role: "assistant",
      content: [{
        type: "tool_use",
        id: "tool-stable-history",
        name: "shell",
        input: { command: "echo stable", options: { cwd: "history", environment: { MODE: "test" } } },
      }],
    },
    {
      role: "user",
      content: [{ type: "tool_result", tool_use_id: "tool-stable-history", content: "stable output" }],
    },
  ];
  for (let index = messages.length; index < 469; index++) {
    messages.push({
      role: index % 2 === 0 ? "user" : "assistant",
      content: [{ type: "text", text: "history-" + index + "-" + "x".repeat(1024) }],
    });
  }
  harness.handlers.load_session = function () {
    return {
      metadata: { id: sessionId, title: "Long stream", message_count: messages.length },
      messages,
      artifacts: [],
    };
  };
  assert.strictEqual(await bridge.sessions.switchToSession(sessionId), true);

  let updates = 0;
  let secondSubscriberUpdates = 0;
  const snapshots = [];
  const secondSubscriberSnapshots = [];
  const unsubscribe = bridge.state.subscribeMany(["sessions", "chat"], function (snapshot) {
    updates += 1;
    snapshots.push(snapshot);
  });
  const unsubscribeSecond = bridge.state.subscribe("chat", function (snapshot) {
    secondSubscriberUpdates += 1;
    secondSubscriberSnapshots.push(snapshot);
  });
  const cloneCallsBeforeStream = harness.getStructuredCloneCalls();
  harness.emit("chat:turn_started", { session_id: sessionId });
  harness.emit("chat:delta", { session_id: sessionId, text: "abcd" });

  const firstDeltaSnapshot = snapshots[1];
  const firstDeltaItem = firstDeltaSnapshot.chatItems.filter(function (item) {
    return item.type === "assistant" && item.streaming;
  }).pop();
  assert.strictEqual(firstDeltaItem.text, "abcd", "the first delta snapshot should capture its own text");
  assert.ok(Object.isFrozen(firstDeltaItem), "nested subscription items should be immutable");
  assert.strictEqual(Reflect.set(firstDeltaItem, "text", "subscriber-only"), false,
    "a subscriber must not mutate a nested item");
  assert.strictEqual(secondSubscriberSnapshots[1].chatItems.filter(function (item) {
    return item.type === "assistant" && item.streaming;
  }).pop().text, "abcd", "one subscriber must not affect a second subscriber");
  assert.strictEqual(bridge.state.get("chat").chatItems.filter(function (item) {
    return item.type === "assistant" && item.streaming;
  }).pop().text, "abcd", "one subscriber must not affect bridge state");
  assert.strictEqual(harness.getStructuredCloneCalls(), cloneCallsBeforeStream + 1,
    "the explicit state.get isolation check should retain its defensive deep copy");
  const cloneCallsAfterDefensiveRead = harness.getStructuredCloneCalls();

  harness.emit("chat:delta", { session_id: sessionId, text: "abcd" });
  assert.strictEqual(firstDeltaItem.text, "abcd", "an older subscription snapshot must remain stable");
  assert.strictEqual(snapshots[2].chatItems.filter(function (item) {
    return item.type === "assistant" && item.streaming;
  }).pop().text, "abcdabcd", "the next snapshot should observe the next delta");

  for (let delta = 2; delta < 1000; delta++) {
    harness.emit("chat:delta", { session_id: sessionId, text: "abcd" });
  }
  await tick();

  assert.strictEqual(updates, 1001, "stream boundaries and deltas should remain immediately observable");
  assert.strictEqual(secondSubscriberUpdates, 1001,
    "all subscribers should receive one immediate update per stream boundary and delta");
  assert.strictEqual(snapshots.length, 1001, "the regression must retain every persistent snapshot");
  assert.strictEqual(secondSubscriberSnapshots.length, 1001,
    "the second subscriber must retain every same-round snapshot for identity checks");
  assert.strictEqual(
    harness.getStructuredCloneCalls(),
    cloneCallsAfterDefensiveRead,
    "a long transcript must not be deep-cloned for each streamed delta"
  );

  const representativeFrames = [0, 1, 499, 500, 999, 1000];
  const stableMessages = snapshots[0].messages;
  const stableHistoryMessage = stableMessages[0];
  const stableHistoryContent = stableHistoryMessage.content;
  const stableHistoryBlock = stableHistoryContent[0];
  const stableToolItem = snapshots[0].chatItems.find(function (item) {
    return item.type === "tool" && item.toolId === "tool-stable-history";
  });
  assert.ok(stableToolItem, "the fixture should expose a historical tool chat item");
  assert.strictEqual(stableToolItem.args.options.environment.MODE, "test");
  representativeFrames.forEach(function (frame) {
    const snapshot = snapshots[frame];
    const secondSnapshot = secondSubscriberSnapshots[frame];
    const toolItem = snapshot.chatItems.find(function (item) {
      return item.type === "tool" && item.toolId === "tool-stable-history";
    });
    assert.strictEqual(snapshot.messages, stableMessages,
      "unchanged history messages arrays must be shared across first/middle/last and adjacent frames");
    assert.strictEqual(snapshot.messages[0], stableHistoryMessage,
      "unchanged history message objects must be structurally shared");
    assert.strictEqual(snapshot.messages[0].content, stableHistoryContent,
      "unchanged history content arrays must be structurally shared");
    assert.strictEqual(snapshot.messages[0].content[0], stableHistoryBlock,
      "unchanged history content blocks must be structurally shared");
    assert.strictEqual(toolItem, stableToolItem,
      "unchanged historical tool chat items must be structurally shared");
    assert.strictEqual(toolItem.args, stableToolItem.args,
      "unchanged historical tool args must be structurally shared");
    assert.strictEqual(toolItem.args.options.environment, stableToolItem.args.options.environment,
      "unchanged historical tool deep subtrees must be structurally shared");
    assert.strictEqual(secondSnapshot.messages, snapshot.messages,
      "two subscribers in the same revision must share the messages domain subtree");
    assert.strictEqual(secondSnapshot.chatItems, snapshot.chatItems,
      "two subscribers in the same revision must share the chatItems domain subtree");
  });

  function streamingItemAt(frame) {
    return snapshots[frame].chatItems.filter(function (item) {
      return item.type === "assistant" && item.streaming;
    }).pop();
  }
  for (let frame = 1; frame < snapshots.length; frame++) {
    assert.notStrictEqual(streamingItemAt(frame - 1), streamingItemAt(frame),
      "the changed streaming item must receive a new reference in every adjacent revision");
  }
  assert.strictEqual(streamingItemAt(1).text, "abcd", "the first retained delta must remain stable");
  assert.strictEqual(streamingItemAt(500).text.length, 2000, "the middle retained delta must remain stable");
  assert.strictEqual(streamingItemAt(1000).text.length, 4000, "the final retained delta must be complete");
  const finalAssistant = bridge.state.get("chat").chatItems.filter(function (item) {
    return item.type === "assistant" && item.streaming;
  }).pop();
  assert.strictEqual(finalAssistant.text.length, 4000, "subscription snapshot optimization must not lose text");
  unsubscribe();
  unsubscribeSecond();
}

async function olderDesktopUsesServerGeneratedSessionDownloadId() {
  const harness = createBridgeHarness(null, { bridgeKind: "web", webSupportedCommands: [] });
  const sessionId = "chat-legacy-download";
  const saved = {
    metadata: { id: sessionId, title: "Legacy download", message_count: 0 },
    messages: [],
    artifacts: [],
  };
  const encoded = Buffer.from(JSON.stringify(saved), "utf8");
  const split = Math.max(1, Math.floor(encoded.length / 2));
  let calls = 0;
  harness.handlers.web_access_load_session_chunk = function (args) {
    calls += 1;
    assert.strictEqual(Object.prototype.hasOwnProperty.call(args, "requestedDownloadId"), false,
      "legacy first chunk must not send the new requestedDownloadId argument");
    if (calls === 1) {
      assert.strictEqual(args.downloadId, null);
      return {
        download_id: "download_server_legacy",
        offset: 0,
        total: encoded.length,
        data_base64: encoded.subarray(0, split).toString("base64"),
        eof: false,
      };
    }
    assert.strictEqual(args.downloadId, "download_server_legacy",
      "legacy follow-up chunks must use the desktop-generated id");
    return {
      download_id: "download_server_legacy",
      offset: split,
      total: encoded.length,
      data_base64: encoded.subarray(split).toString("base64"),
      eof: true,
    };
  };

  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);
  assert.strictEqual(calls, 2);
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_cancel_session_download"; }),
    false,
    "legacy success must not invoke the unsupported cancel command"
  );
}

async function sessionDownloadWaitsForCapabilitySnapshot() {
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
  });
  const sessionId = "chat-capabilities-late";
  const pending = harness.bridge.sessions.switchToSession(sessionId);
  await tick();
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false,
    "connected-before-snapshot must not guess the legacy protocol"
  );
  harness.setWebCapabilities(["web_access_cancel_session_download"]);
  assert.strictEqual(await pending, true);
  const load = harness.calls.find(function (call) { return call.cmd === "web_access_load_session_chunk"; });
  assert.ok(load.args.requestedDownloadId,
    "the post-snapshot request must select the newly advertised cancellable protocol");
}

async function sessionDownloadCapabilityWaitStopsOnDisconnect() {
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
  });
  const pending = harness.bridge.sessions.switchToSession("chat-capabilities-disconnected");
  await tick();
  harness.setWebConnection({ status: "desktop_offline", desktop_online: false });
  assert.strictEqual(await pending, false);
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false,
    "disconnect before the snapshot must reject without issuing a protocol-ambiguous load"
  );
}

async function sessionDownloadCapabilityWaitRejectsAlreadyOfflineDesktop() {
  let capabilityTimeouts = 0;
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
    webConnectionState: { status: "desktop_offline", desktop_online: false },
    setTimeout: function (callback, delay) {
      if (delay === 10_000) capabilityTimeouts += 1;
      return setTimeout(callback, delay);
    },
  });

  assert.strictEqual(await harness.bridge.sessions.switchToSession("chat-capabilities-already-offline"), false);
  assert.strictEqual(capabilityTimeouts, 0,
    "an already-offline desktop must reject before installing the 10 second timeout");
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false
  );
}

async function sessionDownloadCapabilityWaitClosesLostDisconnectEventRace() {
  let capabilityTimeouts = 0;
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
    setTimeout: function (callback, delay) {
      if (delay === 10_000) capabilityTimeouts += 1;
      return setTimeout(callback, delay);
    },
  });
  let reads = 0;
  harness.setWebConnectionStateReader(function () {
    reads += 1;
    return reads === 1
      ? { status: "connected", desktop_online: true }
      : { status: "desktop_offline", desktop_online: false };
  });

  assert.strictEqual(await harness.bridge.sessions.switchToSession("chat-capabilities-lost-disconnect"), false);
  assert.ok(reads >= 2, "connection state must be checked before and after listener registration");
  assert.strictEqual(capabilityTimeouts, 1,
    "the race-closing check may install the timeout but must cancel it immediately");
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false
  );
}

async function sessionDownloadCapabilityWaitHasTimeout() {
  // 能力超时的切换进入宽限等待（5s 内快照到达即自动重试），期间不报错、
  // 不落失败。能力超时（10s）压成微任务立即触发；宽限 timer（5s）只登记
  // 不自动触发，模拟真实时序中快照先于宽限到期到达。
  const graceTimers = [];
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
    setTimeout: function (callback, delay) {
      if (delay === 10_000) {
        queueMicrotask(callback);
        return 1;
      }
      if (delay === 5_000) {
        graceTimers.push(callback);
        return 10 + graceTimers.length;
      }
      return setTimeout(callback, delay);
    },
  });
  const switchedPromise = harness.bridge.sessions.switchToSession("chat-capabilities-timeout");
  for (let waitAttempt = 0; waitAttempt < 4; waitAttempt++) await tick();
  assert.strictEqual(graceTimers.length, 1,
    "the capability timeout must arm exactly one grace timer");
  harness.setWebCapabilities(["web_access_cancel_session_download"]);
  assert.strictEqual(await switchedPromise, true,
    "the retried switch must resolve through to its final outcome");
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    true,
    "a late capability snapshot must automatically retry the switch that timed out"
  );
  assert.strictEqual(
    harness.bridge.state.get("sessions").activeSessionId,
    "chat-capabilities-timeout"
  );
}

async function sessionDownloadCapabilityGraceExpiryFailsOnce() {
  // 宽限期内快照始终未到：切换按失败收口且只报一次错；随后快照到达也不得
  // 复活已被取代的等待（无新切换请求时不产生多余 RPC）。
  let graceFired = false;
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
    setTimeout: function (callback, delay) {
      if (delay === 10_000) {
        queueMicrotask(callback);
        return 1;
      }
      if (delay === 5_000) {
        graceFired = true;
        queueMicrotask(callback);
        return 2;
      }
      return setTimeout(callback, delay);
    },
  });
  assert.strictEqual(await harness.bridge.sessions.switchToSession("chat-capabilities-grace"), false,
    "the switch must fail once the grace period expires without a snapshot");
  assert.ok(graceFired, "the grace timer must fire when no snapshot arrives");
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false
  );
  harness.setWebCapabilities(["web_access_cancel_session_download"]);
  for (let settleAttempt = 0; settleAttempt < 4; settleAttempt++) await tick();
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false,
    "an expired grace wait must not be revived by a later snapshot"
  );
}

async function sessionDownloadCapabilityGraceSupersededByDraftFailsSilently() {
  // 宽限等待期间用户进入草稿（enterDraft 使切换 token 失效但不收口 pending）：
  // 宽限 timer 到期必须按失败静默收口，原调用方收到 false，且不在草稿页
  // 误报“加载对话失败”（与快照到达路径对已被取代等待的处理一致）。
  const graceTimers = [];
  const harness = createBridgeHarness(null, {
    bridgeKind: "web",
    webCapabilitiesReady: false,
    setTimeout: function (callback, delay) {
      if (delay === 10_000) {
        queueMicrotask(callback);
        return 1;
      }
      if (delay === 5_000) {
        graceTimers.push(callback);
        return 10 + graceTimers.length;
      }
      return setTimeout(callback, delay);
    },
  });
  const switchedPromise = harness.bridge.sessions.switchToSession("chat-capabilities-superseded");
  for (let waitAttempt = 0; waitAttempt < 4; waitAttempt++) await tick();
  assert.strictEqual(graceTimers.length, 1,
    "the capability timeout must arm exactly one grace timer");
  await harness.bridge.sessions.createNewSession();
  assert.strictEqual(graceTimers.length, 1,
    "entering the draft view must not arm another grace timer");
  graceTimers[0]();
  assert.strictEqual(await switchedPromise, false,
    "a grace wait superseded by entering the draft must settle as a failure");
  assert.strictEqual(
    JSON.stringify(harness.bridge.state.get("chat").chatItems).includes("capability snapshot timed out"),
    false,
    "a superseded grace wait must not report a load failure in the draft view"
  );
  harness.setWebCapabilities(["web_access_cancel_session_download"]);
  for (let settleAttempt = 0; settleAttempt < 4; settleAttempt++) await tick();
  assert.strictEqual(
    harness.calls.some(function (call) { return call.cmd === "web_access_load_session_chunk"; }),
    false,
    "a superseded grace wait must not be retried by a later snapshot"
  );
}

async function deepSeekTurnTimelineLifecycleBehavior() {
  const harness = createBridgeHarness();
  const sessionId = "chat-turn-timeline";
  harness.handlers.load_session = function () {
    return {
      metadata: { id: sessionId, title: "Turn timeline", message_count: 2 },
      messages: [
        { role: "user", content: [{ type: "text", text: "旧问题" }] },
        { role: "assistant", content: [{ type: "text", text: "旧回答" }] },
      ],
      artifacts: [],
    };
  };
  harness.handlers.get_session_timeline = function () {
    return [
      { turn_id: "turn-old", event: "user_start", timestamp: Date.now() - 5000, ts: "2026-07-24T00:00:00Z" },
      { turn_id: "turn-old", event: "assistant_done", timestamp: Date.now() - 4000, ts: "2026-07-24T00:00:01Z", status: "Completed" },
      { turn_id: "turn-current", event: "user_start", timestamp: Date.now(), ts: "2026-07-24T00:00:05Z" },
    ];
  };

  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "继续" });
  await harness.emit("chat:turn_started", { session_id: sessionId });
  const running = harness.bridge.state.get("chat").turnTimeline;
  assert.strictEqual(running.length, 3, "turn_started must reuse a freshly loaded unmatched timing event");
  assert.strictEqual(running[2].ui_turn_index, 1, "live lifecycle must bind to the visible user Turn");

  await harness.emit("chat:done", { session_id: sessionId, status: "Failed", error: "模型失败" });
  const completed = harness.bridge.state.get("chat").turnTimeline;
  assert.strictEqual(completed.length, 4);
  assert.strictEqual(completed[3].turn_id, "turn-current");
  assert.strictEqual(completed[3].status, "Failed");
  assert.strictEqual(completed[3].error, "模型失败");
}

async function internalSubagentHandoffStaysOutOfPresentation(bridgeKind) {
  const harness = createBridgeHarness(null, { bridgeKind });
  const sessionId = "chat-subagent-handoff";
  const completionText = [
    '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
    'This is an internal runtime event, not user input.',
    'child-only completion summary',
    '<codewhale:subagent.done>{"agent_id":"agent_7fb1c7be","status":"completed"}</codewhale:subagent.done>',
    '</codewhale:runtime_event>',
  ].join('\n');
  const persistedToolOutput = [
    'Error: deterministic provider failure',
    '',
    '<codewhale:runtime_event kind="stuck_guard" visibility="internal">',
    'Change strategy instead of repeating the same tool call.',
    '</codewhale:runtime_event>',
    '',
    '<codewhale:runtime_event kind="tool_error_degradation" visibility="internal">',
    'Switch to an alternate tool or source.',
    '</codewhale:runtime_event>',
  ].join('\n');
  harness.handlers.load_session = function () {
    return {
      metadata: { id: sessionId, title: "Sub-agent handoff", message_count: 5 },
      messages: [
        { role: "user", content: [{ type: "text", text: "请调研这个问题" }] },
        { role: "user", content: [
          { type: "text", text: completionText },
          { type: "text", text: "<turn_meta>\nInput provenance: subagent_handoff\nInput authority: non_authoritative\n</turn_meta>" },
        ] },
        { role: "assistant", content: [
          { type: "tool_use", id: "tool-runtime-guidance", name: "host_failure_probe", input: {} },
        ] },
        { role: "user", content: [
          { type: "tool_result", tool_use_id: "tool-runtime-guidance", content: persistedToolOutput, is_error: true },
        ] },
        { role: "assistant", content: [{ type: "text", text: "这是父智能体的最终汇总" }] },
      ],
      artifacts: [],
    };
  };

  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);
  const state = harness.bridge.state.get("chat");
  let visible = JSON.stringify(state.chatItems);
  let raw = JSON.stringify(state.messages);
  assert.ok(visible.includes("请调研这个问题"), "real user input must remain visible");
  assert.ok(visible.includes("这是父智能体的最终汇总"), "parent synthesis must remain visible");
  assert.ok(!visible.includes("child-only completion summary"), "sub-agent handoff must not render as a user bubble");
  assert.ok(!visible.includes("codewhale:runtime_event"), "internal runtime XML must stay out of the presentation");
  assert.ok(raw.includes("child-only completion summary"), "sub-agent completion must remain in the parent model context");
  assert.ok(raw.includes("subagent_handoff"), "handoff provenance must remain durable");
  assert.ok(visible.includes("Error: deterministic provider failure"), "real tool output must remain visible");
  assert.ok(!visible.includes("Change strategy instead"), "stuck guidance must stay out of restored tool cards");
  assert.ok(!visible.includes("Switch to an alternate tool"), "degradation guidance must stay out of restored tool cards");
  assert.ok(raw.includes("Change strategy instead"), "stuck guidance must remain durable for the model");
  assert.ok(raw.includes("Switch to an alternate tool"), "degradation guidance must remain durable for the model");

  await harness.emit("chat:user_message", {
    session_id: sessionId,
    content: completionText,
    operation: "append",
  });
  visible = JSON.stringify(harness.bridge.state.get("chat").chatItems);
  assert.ok(!visible.includes("child-only completion summary"), "live handoff event must not render as a user bubble");
  assert.ok(!visible.includes("codewhale:runtime_event"), "live runtime XML must stay out of the presentation");

  await harness.emit("chat:tool_start", {
    session_id: sessionId,
    id: "tool-live-runtime-guidance",
    name: "host_failure_probe",
    args: {},
  });
  await harness.emit("chat:tool_end", {
    session_id: sessionId,
    id: "tool-live-runtime-guidance",
    success: false,
    output: persistedToolOutput,
  });
  visible = JSON.stringify(harness.bridge.state.get("chat").chatItems);
  raw = JSON.stringify(harness.bridge.state.get("chat").messages);
  assert.ok(visible.includes("Error: deterministic provider failure"), bridgeKind + " live tool output must remain visible");
  assert.ok(!visible.includes("Change strategy instead"), bridgeKind + " live stuck guidance must stay out of tool cards");
  assert.ok(!visible.includes("Switch to an alternate tool"), bridgeKind + " live degradation guidance must stay out of tool cards");
  assert.ok(raw.includes("Change strategy instead"), bridgeKind + " live stuck guidance must remain durable");
  assert.ok(raw.includes("Switch to an alternate tool"), bridgeKind + " live degradation guidance must remain durable");
}

async function currentInternalProvenanceAndEnvelopeStayOutOfPresentation() {
  const completionText = [
    '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
    'This is an internal runtime event, not user input.',
    'current child-only completion summary',
    '<codewhale:subagent.done>{"agent_id":"agent_current","status":"completed"}</codewhale:subagent.done>',
    '</codewhale:runtime_event>',
  ].join('\n');
  const shellCompletionText = [
    '<codewhale:runtime_event kind="background_shell_completion" visibility="internal">',
    'internal shell completion payload',
    '</codewhale:runtime_event>',
  ].join('\n');

  for (let bridgeIndex = 0; bridgeIndex < 2; bridgeIndex++) {
    const bridgeKind = bridgeIndex === 0 ? "tauri" : "web";
    const harness = createBridgeHarness(null, { bridgeKind });
    const sessionId = "chat-current-internal-provenance-" + bridgeKind;
    harness.handlers.load_session = function () {
      return {
        metadata: { id: sessionId, title: "Current internal provenance", message_count: 5 },
        messages: [
          { role: "user", content: [{ type: "text", text: "real user request" }] },
          { role: "user", content: [
            { type: "text", text: completionText },
            { type: "text", text: [
              "<turn_meta>",
              "Current local date: 2026-08-12",
              "Current workspace: /private/workspace",
              "Current permission posture: Full Access",
              "Input provenance: subagent_handoff (non-authoritative)",
              "</turn_meta>",
            ].join("\n") },
          ] },
          { role: "user", content: [
            { type: "text", text: "current runtime recovery hint" },
            { type: "text", text: "<turn_meta>\nInput provenance: runtime (non-authoritative)\n</turn_meta>" },
          ] },
          { role: "user", content: [
            { type: "text", text: shellCompletionText },
            { type: "text", text: "<turn_meta>\nInput provenance: shell_completion (non-authoritative)\n</turn_meta>" },
          ] },
          // 无信封、仅 turn_meta 的 shell_completion：白名单必须单独兜住（遗留双行/裁剪会话形态）。
          { role: "user", content: [
            { type: "text", text: "<turn_meta>\nInput provenance: shell_completion (non-authoritative)\n</turn_meta>" },
          ] },
          { role: "assistant", content: [{ type: "text", text: "parent final answer" }] },
        ],
        artifacts: [],
      };
    };

    assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);
    const state = harness.bridge.state.get("chat");
    const visible = JSON.stringify(state.chatItems);
    const raw = JSON.stringify(state.messages);
    assert.ok(visible.includes("real user request"), bridgeKind + " must retain real user input");
    assert.ok(visible.includes("parent final answer"), bridgeKind + " must retain the parent answer");
    [
      "current child-only completion summary",
      "current runtime recovery hint",
      "internal shell completion payload",
      "codewhale:runtime_event",
      "Current workspace:",
      "Input provenance: shell_completion",
    ].forEach(function (hiddenText) {
      assert.ok(!visible.includes(hiddenText),
        bridgeKind + " must hide internal payload: " + hiddenText);
    });
    assert.ok(raw.includes("current child-only completion summary"),
      bridgeKind + " must preserve the child handoff in model context");
    assert.ok(raw.includes("current runtime recovery hint"),
      bridgeKind + " must preserve runtime recovery context");
    assert.ok(raw.includes("internal shell completion payload"),
      bridgeKind + " must preserve shell completion context");
    assert.ok(raw.includes("subagent_handoff (non-authoritative)"),
      bridgeKind + " must preserve current provenance metadata");
    assert.ok(raw.includes("shell_completion (non-authoritative)"),
      bridgeKind + " must preserve shell_completion provenance metadata");
  }
}

async function autoTitleSkipsInternalAndStripsTurnMeta() {
  for (let bridgeIndex = 0; bridgeIndex < 2; bridgeIndex++) {
    const bridgeKind = bridgeIndex === 0 ? "tauri" : "web";
    const envelopeText = [
      '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
      'This is an internal runtime event, not user input.',
      'auto-title child completion summary',
      '</codewhale:runtime_event>',
    ].join('\n');

    // 场景 1：首条 user 消息为内部信封 → 不得 rename（XML 不得进 sidebar 标题）。
    const h1 = createBridgeHarness(null, { bridgeKind });
    const sid1 = "chat-title-internal-" + bridgeKind;
    h1.handlers.list_sessions = function () {
      return [{ id: sid1, title: "New chat" }];
    };
    h1.handlers.load_session = function () {
      return {
        metadata: { id: sid1, title: "New chat" },
        messages: [
          { role: "user", content: [
            { type: "text", text: envelopeText },
            { type: "text", text: "<turn_meta>\nInput provenance: subagent_handoff (non-authoritative)\n</turn_meta>" },
          ] },
          { role: "assistant", content: [{ type: "text", text: "父汇总" }] },
        ],
        artifacts: [],
      };
    };
    assert.strictEqual(await h1.bridge.sessions.switchToSession(sid1), true);
    await h1.emit("session:list_changed", {});
    await new Promise(function (r) { setTimeout(r, 50); });
    await h1.emit("chat:done", { session_id: sid1, status: "Completed" });
    await new Promise(function (r) { setTimeout(r, 150); });
    assert.ok(h1.calls.some(function (c) { return c.cmd === "save_session_artifacts"; }),
      bridgeKind + " persistMessagesFor 应执行（前置条件成立，防止假绿）");
    const rename1 = h1.calls.filter(function (c) { return c.cmd === "rename_session"; });
    assert.strictEqual(rename1.length, 0,
      bridgeKind + " 首条内部信封不得触发自动命名（信封 XML 不得进 sidebar）：" + JSON.stringify(rename1));

    // 场景 2：首条 user 消息为普通消息（引擎标准布局：正文 + 尾随 turn_meta block）
    // → 标题应为正文，不得拼入 turn_meta/workspace XML。
    const h2 = createBridgeHarness(null, { bridgeKind });
    const sid2 = "chat-title-normal-" + bridgeKind;
    h2.handlers.list_sessions = function () {
      return [{ id: sid2, title: "New chat" }];
    };
    h2.handlers.load_session = function () {
      return {
        metadata: { id: sid2, title: "New chat" },
        messages: [
          { role: "user", content: [
            { type: "text", text: "帮我修登录页" },
            { type: "text", text: "<turn_meta>\nCurrent workspace: /private/ws\n</turn_meta>" },
          ] },
          { role: "assistant", content: [{ type: "text", text: "好的" }] },
        ],
        artifacts: [],
      };
    };
    assert.strictEqual(await h2.bridge.sessions.switchToSession(sid2), true);
    await h2.emit("session:list_changed", {});
    await new Promise(function (r) { setTimeout(r, 50); });
    await h2.emit("chat:done", { session_id: sid2, status: "Completed" });
    await new Promise(function (r) { setTimeout(r, 150); });
    const rename2 = h2.calls.filter(function (c) { return c.cmd === "rename_session"; });
    assert.strictEqual(rename2.length, 1,
      bridgeKind + " 普通首条消息应触发自动命名");
    const title2 = String(rename2[0] && rename2[0].args && rename2[0].args.title || "");
    assert.ok(title2.indexOf("帮我修登录页") === 0,
      bridgeKind + " 标题应以真实正文开头：" + JSON.stringify(title2));
    assert.ok(!title2.includes("<turn_meta>") && !title2.includes("Current workspace"),
      bridgeKind + " 标题不得包含 turn_meta/workspace XML：" + JSON.stringify(title2));
  }
}

async function webLiveEnvelopeStaysOutOfPresentation() {
  const bridgeKind = "web";
  const harness = createBridgeHarness(null, { bridgeKind });
  const sessionId = "chat-web-live-envelope";
  const envelopeText = [
    '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
    'This is an internal runtime event, not user input.',
    'web live child completion summary',
    '</codewhale:runtime_event>',
  ].join('\n');
  harness.handlers.load_session = function () {
    return {
      metadata: { id: sessionId, title: "Web live", message_count: 1 },
      messages: [{ role: "assistant", content: [{ type: "text", text: "既有回答" }] }],
      artifacts: [],
    };
  };
  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);
  await harness.emit("chat:user_message", { session_id: sessionId, content: envelopeText, operation: "append" });
  const visible = JSON.stringify(harness.bridge.state.get("chat").chatItems);
  assert.ok(!visible.includes("web live child completion summary"),
    "web live 内部信封不得渲染为用户气泡");
  assert.ok(!visible.includes("codewhale:runtime_event"),
    "web live 内部信封 XML 不得进入展示");
}

async function draftToggleFailureAbortsFirstSend() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  harness.handlers.set_multi_agent_mode = function () { throw new Error("git missing"); };

  await bridge.interaction.setMultiAgentMode(true); // 草稿态寄存意图
  await bridge.chat.sendMessage("并行调研测试");

  const calls = harness.calls.map(function (call) { return call.cmd; });
  assert.ok(!calls.includes("chat"), "开关落盘失败后首条消息不得发出（否则静默退化成普通对话）");
  assert.ok(calls.includes("delete_session"), "中止物化必须清掉刚建的空会话");
  assert.equal(bridge.state.get("sessions").activeSessionId, null, "必须回到草稿态");
  assert.ok(
    JSON.stringify(bridge.state.get("chat").chatItems).includes("git missing"),
    "失败原因要如实提示"
  );
  assert.equal(
    bridge.state.get("chat").composerPrefill.text,
    "并行调研测试",
    "被中止的输入必须回填输入框，不得静默丢字"
  );

  // 意图保留：修好依赖后再次发送，开关重试且消息正常发出。
  harness.handlers.set_multi_agent_mode = function () { return { mode: "yolo", multi_agent: true }; };
  await bridge.chat.sendMessage("再来一次");
  const after = harness.calls.map(function (call) { return call.cmd; });
  assert.ok(
    after.filter(function (cmd) { return cmd === "set_multi_agent_mode"; }).length >= 2,
    "草稿开关意图必须保留到下一次物化重试"
  );
  assert.ok(after.includes("chat"), "开关成功后消息正常发出");
}

async function multiAgentToggleFailureIsRoutedToTriggerSession() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let rejectToggle = null;
  harness.handlers.set_multi_agent_mode = function () {
    return new Promise(function (_resolve, reject) { rejectToggle = reject; });
  };
  harness.handlers.get_mode_state = function (args) {
    return { mode: "yolo", multi_agent: args.sessionId === "chat-b" };
  };

  await bridge.sessions.switchToSession("chat-a");
  const flight = bridge.interaction.setMultiAgentMode(true);
  for (let i = 0; i < 20 && !rejectToggle; i++) await Promise.resolve();
  assert.ok(rejectToggle, "toggle request must reach the backend handler");
  assert.strictEqual(bridge.state.get("chat").modeState.multiAgent, true,
    "optimistic flip must be visible on the trigger session immediately");

  // 请求还没返回，用户切到另一个开着多智能体的会话 B。
  await bridge.sessions.switchToSession("chat-b");
  assert.strictEqual(bridge.state.get("chat").modeState.multiAgent, true, "B is on");
  rejectToggle(new Error("roster boom"));
  await flight;

  const chatOnB = bridge.state.get("chat");
  assert.strictEqual(chatOnB.modeState.multiAgent, true,
    "the rollback must not clobber the session the user switched to");
  assert.ok(!JSON.stringify(chatOnB.chatItems).includes("roster boom"),
    "the failure toast must not land in the session the user switched to");

  await bridge.sessions.switchToSession("chat-a");
  assert.ok(JSON.stringify(bridge.state.get("chat").chatItems).includes("roster boom"),
    "the failure toast must be routed back to the trigger session");
  assert.strictEqual(bridge.state.get("chat").modeState.multiAgent, false,
    "the trigger session must end up rolled back to off");
}

async function scheduledRunUnreadBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const task = { id: "automation-unread", name: "Unread task", hasUnreadRuns: true };
  const runs = [
    { id: "run-1", automationId: task.id, sessionId: "sched-run-1", status: "completed", unread: true },
    { id: "run-2", automationId: task.id, sessionId: "sched-run-2", status: "completed", unread: true },
  ];
  let openedContextPublished = false;
  bridge.state.subscribe("scheduled", function (state) {
    if (state.scheduledRunContext && state.scheduledRunContext.runId) openedContextPublished = true;
  });
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.read_scheduled_task = function () { return Object.assign({}, task); };
  harness.handlers.list_scheduled_task_runs = function () {
    return runs.map(function (run) { return Object.assign({}, run); });
  };
  harness.handlers.list_scheduled_runs = function () {
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

  await bridge.sessions.switchToSession("chat-origin");
  await bridge.scheduled.loadScheduledTasks();
  bridge.scheduled.selectScheduledTask(task.id);
  await bridge.scheduled.readScheduledTask(task.id);
  await bridge.scheduled.loadScheduledTaskRuns(task.id, 20);
  await bridge.scheduled.loadScheduledTaskRecentRuns();
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length,
    0,
    "opening task details or run history must not mark any independent run conversation as viewed"
  );

  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(runs[0], task), true);
  const marks = harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; });
  assert.strictEqual(JSON.stringify(marks.map(function (call) { return call.args; })), JSON.stringify([
    { automationId: task.id, runId: "run-1" },
  ]));
  const afterFirst = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(afterFirst.scheduledTaskRuns[0].unread, false, "the opened run should become viewed");
  assert.strictEqual(afterFirst.scheduledTaskRuns[1].unread, true, "sibling runs remain independently unread");
  assert.strictEqual(afterFirst.scheduledTaskRecentRuns[0].unread, false, "the opened sidebar run should lose its dot immediately");
  assert.strictEqual(afterFirst.scheduledTaskRecentRuns[1].unread, true, "the sibling sidebar run should remain unread");
  assert.strictEqual(afterFirst.scheduledTasks[0].hasUnreadRuns, true, "task dot remains while a child run is unread");
  assert.strictEqual(afterFirst.scheduledTaskDetail.hasUnreadRuns, true);

  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(runs[1], task), true);
  const afterSecond = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.ok(afterSecond.scheduledTaskRuns.every(function (run) { return run.unread === false; }));
  assert.ok(afterSecond.scheduledTaskRecentRuns.every(function (run) { return run.unread === false; }));
  assert.strictEqual(afterSecond.scheduledTasks[0].hasUnreadRuns, false, "task dot clears only after every child run was opened");
  assert.strictEqual(afterSecond.scheduledTaskDetail.hasUnreadRuns, false);

  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  harness.emit("chat:delta", { session_id: "sched-running", text: "partial live output" });
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-running") {
      return {
        metadata: { id: "sched-running", title: "Running scheduled conversation" },
        messages: [{ role: "user", content: [
          { type: "text", text: "<system-reminder>\ninternal policy: sudo/apt/systemctl/pkexec\n</system-reminder>\n\ndurable scheduled prompt" },
          { type: "text", text: "<turn_meta>\nCurrent workspace: C:\\\\Users\\\\demo\n</turn_meta>" },
        ] }, { role: "user", content: [
          { type: "text", text: "Tool calls have failed for 2 consecutive steps (web_search)." },
          { type: "text", text: "<turn_meta>\nInput provenance: runtime\nInput authority: non_authoritative\n</turn_meta>" },
        ] }],
        artifacts: [],
      };
    }
    return {
      metadata: { id: args.id, title: "Scheduled conversation" },
      messages: [], artifacts: [],
    };
  };
  const markCount = harness.calls.filter(function (call) { return call.cmd === "mark_scheduled_run_viewed"; }).length;
  const loadCount = harness.calls.filter(function (call) { return call.cmd === "load_session"; }).length;
  assert.strictEqual(
    await bridge.scheduled.openScheduledRunChat(
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
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, "sched-running");
  assert.ok(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems.some(function (item) {
      return String(item.text || item.html || "").includes("partial live output");
    }),
    "the normal chat transcript should expose buffered live output"
  );
  assert.ok(
    JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems).includes("durable scheduled prompt"),
    "the normal chat transcript should also include the durable user prompt"
  );
  const visibleScheduledTranscript = JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems);
  assert.ok(!visibleScheduledTranscript.includes("system-reminder"), "scheduled bubbles must hide the internal reminder");
  assert.ok(!visibleScheduledTranscript.includes("turn_meta"), "scheduled bubbles must hide turn metadata");
  assert.ok(!visibleScheduledTranscript.includes("sudo/apt/systemctl/pkexec"), "scheduled bubbles must hide internal policy text");
  assert.ok(!visibleScheduledTranscript.includes("Current workspace"), "scheduled bubbles must hide internal workspace metadata");
  assert.ok(!visibleScheduledTranscript.includes("Tool calls have failed"), "runtime recovery hints must not render as user bubbles");
  assert.ok(
    JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).messages).includes("<system-reminder>"),
    "the raw scheduled message must remain intact for model context"
  );
  assert.ok(
    JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).messages).includes("Tool calls have failed"),
    "runtime recovery hints must remain intact in raw model context"
  );
  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  harness.emit("chat:delta", { session_id: "sched-buffered", text: "partial background output" });
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-buffered") throw new Error("buffered scheduled session is not durable");
    throw new Error("missing scheduled session");
  };
  assert.strictEqual(
    await bridge.scheduled.openScheduledRunChat(
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
    await bridge.scheduled.openScheduledRunChat(
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
  const harness = createBridgeHarness();
  harness.setDialogResult("D:/workspace-picked");
  assert.strictEqual(await harness.bridge.scheduled.pickFolder(), "D:/workspace-picked");
  assert.strictEqual(JSON.stringify(harness.dialogCalls[0]), JSON.stringify({
    directory: true,
    multiple: false,
    title: "选择工作目录",
  }));
  harness.setDialogResult(null);
  assert.strictEqual(await harness.bridge.scheduled.pickFolder(), null, "canceling folder selection should preserve the typed path");
}

async function scheduledRunningHydrationRaceBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const load = deferred();
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
  await bridge.sessions.switchToSession("chat-origin");
  const opening = bridge.scheduled.openScheduledRunChat(
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
  harness.emit("chat:tool_end", {
    session_id: "sched-live-race", id: "tool-hydrate", success: true, output: "hydrated result",
  });
  load.resolve({
    metadata: { id: "sched-live-race", title: "Live scheduled run" },
    messages: [
      { role: "user", content: [{ type: "text", text: "persisted scheduled prompt" }] },
      { role: "assistant", content: [
        { type: "thinking", thinking: "durable-only reasoning metadata" },
        { type: "text", text: "delta received during durable load" },
        { type: "tool_use", id: "tool-hydrate", name: "shell", input: { command: "echo hydrate" } },
      ] },
      { role: "user", content: [
        { type: "tool_result", tool_use_id: "tool-hydrate", content: "hydrated result" },
      ] },
    ],
    artifacts: [],
  });
  assert.strictEqual(await opening, true);
  const hydratedItems = bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems;
  const rendered = JSON.stringify(hydratedItems);
  assert.ok(rendered.includes("persisted scheduled prompt"), "durable history should survive live hydration");
  assert.ok(rendered.includes("delta received during durable load"), "live deltas received during load should survive hydration");
  const overlappingAssistantItems = hydratedItems.filter(function (item) {
    return item.type === "assistant" && item.text === "delta received during durable load";
  });
  assert.strictEqual(overlappingAssistantItems.length, 1, "durable and live overlap should render once");
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems.filter(function (item) {
      return item.type === "tool" && item.toolId === "tool-hydrate";
    }).length,
    1,
    "durable and live tool cards should merge by tool id"
  );
}

async function openingRunningMarksBusyBeforeHydration() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const load = deferred();
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-opening-busy") return load.promise;
    return {
      metadata: { id: args.id, title: "Origin" },
      messages: [], artifacts: [],
    };
  };
  await bridge.sessions.switchToSession("chat-origin");
  const opening = bridge.scheduled.openScheduledRunChat({
    id: "run-opening-busy",
    automationId: "automation-opening-busy",
    sessionId: "sched-opening-busy",
    status: "running",
    unread: false,
  }, { id: "automation-opening-busy", name: "Opening busy task" });
  await tick();

  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).sessionBusy["sched-opening-busy"],
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
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-origin");
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-followup",
    automationId: "automation-followup",
    sessionId: "sched-followup",
    status: "running",
    unread: false,
  }, { id: "automation-followup", name: "Follow-up task" }), true);
  harness.emit("chat:delta", { session_id: "sched-followup", text: "initial scheduled output" });
  const initialAssistantCount = bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems.filter(function (item) {
    return item.type === "assistant";
  }).length;

  await bridge.chat.sendMessage("follow up after the scheduled run");
  const queued = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
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
  const flushed = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(flushed.queued.length, 0);
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    1,
    "the queued follow-up should flush only after the scheduled terminal event"
  );
  assert.strictEqual(flushed.busy, true, "the flushed follow-up should own the next busy turn");
}

async function webFollowupQueuedUntilScheduledInitialTurnTerminal() {
  // Web-bridge mirror of the case above: the scheduled session's terminal
  // event must release the remote-authority gate (not arm it), so the queued
  // follow-up drains instead of dead-locking inside reconcileRemoteTurn.
  const harness = createBridgeHarness(Object.create(null), {
    bridgeKind: "web",
    webSupportedCommands: [
      "web_access_chat", "web_access_load_session_chunk",
      "web_access_cancel_session_download", "web_access_list_sessions",
      "web_access_list_archived_sessions",
      "web_access_create_session_and_chat", "web_access_create_session",
      "web_access_status",
    ],
  });
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-origin");
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-followup-web",
    automationId: "automation-followup-web",
    sessionId: "sched-followup-web",
    status: "running",
    unread: false,
  }, { id: "automation-followup-web", name: "Follow-up task" }), true);
  harness.emit("chat:delta", { session_id: "sched-followup-web", text: "initial scheduled output" });
  const initialAssistantCount = bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems.filter(function (item) {
    return item.type === "assistant";
  }).length;

  await bridge.chat.sendMessage("follow up after the scheduled run");
  const queued = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(queued.queued.length, 1, "web: follow-up input must queue while the initial scheduled turn is active");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "web_access_chat" || call.cmd === "chat"; }).length,
    0,
    "web: a queued follow-up must not overlap the scheduled engine turn"
  );
  assert.strictEqual(
    queued.chatItems.filter(function (item) { return item.type === "assistant"; }).length,
    initialAssistantCount,
    "web: queueing a follow-up must not create an overlapping assistant placeholder"
  );

  harness.emit("chat:done", { session_id: "sched-followup-web" });
  await tick();
  await tick();
  const flushed = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(flushed.queued.length, 0, "web: the queued follow-up must drain after the scheduled terminal event");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "web_access_chat" || call.cmd === "chat"; }).length,
    1,
    "web: exactly one follow-up chat call should be issued after the scheduled terminal"
  );
  assert.strictEqual(flushed.busy, true, "web: the flushed follow-up should own the next busy turn");
}

async function terminalEventWinsStaleRunningOpen() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const firstLoad = deferred();
  let loads = 0;
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
  const staleRun = {
    id: "run-terminal-wins",
    automationId: "automation-terminal-wins",
    sessionId: "sched-terminal-wins",
    status: "running",
    unread: false,
  };
  await bridge.sessions.switchToSession("chat-origin");
  const opening = bridge.scheduled.openScheduledRunChat(staleRun, {
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
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy, false, "terminal event should clear initial busy after hydration");

  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(staleRun, {
    id: staleRun.automationId,
    name: "Terminal wins task",
  }), true);
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy,
    false,
    "a stale running DTO must not move a terminal scheduled buffer back to active"
  );
  await bridge.chat.sendMessage("continue after terminal");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    1,
    "a follow-up after terminal should start normally instead of remaining queued"
  );

  const completedHarness = createBridgeHarness();
  const completedBridge = completedHarness.bridge;
  const completedSessionId = "owned-completed-session";
  await completedBridge.sessions.switchToSession("chat-origin");
  assert.strictEqual(await completedBridge.scheduled.openScheduledRunChat({
    id: "run-completed-owned",
    automationId: "automation-completed-owned",
    sessionId: completedSessionId,
    status: "completed",
    unread: true,
  }, { id: "automation-completed-owned", name: "Completed owned task" }), true);
  assert.strictEqual(await completedBridge.scheduled.exitScheduledRunChat(), true);
  assert.strictEqual(await completedBridge.scheduled.openScheduledRunChat({
    id: "run-completed-owned",
    automationId: "automation-completed-owned",
    sessionId: completedSessionId,
    status: "running",
    unread: false,
  }, { id: "automation-completed-owned", name: "Completed owned task" }), true);
  assert.strictEqual(
    completedBridge.state.get('chat').busy,
    false,
    "a completed durable open must remain terminal when an older running DTO arrives later"
  );
}

async function scheduledDoneBeforeBufferCreatesTerminalTombstone() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-origin");

  harness.emit("chat:done", { session_id: "sched-done-before-buffer" });
  await tick();
  await tick();
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-done-before-buffer",
    automationId: "automation-done-before-buffer",
    sessionId: "sched-done-before-buffer",
    status: "running",
    unread: false,
  }, { id: "automation-done-before-buffer", name: "Done first task" }), true);
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy,
    false,
    "a scheduled terminal event received before buffer creation must beat a later stale running DTO"
  );
  await bridge.chat.sendMessage("continue after done-first run");
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).queued.length, 0, "done-first terminal state must not strand follow-up input");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    1,
    "follow-up after a done-first run should start immediately"
  );

  harness.emit("chat:done", { session_id: "ordinary-done-without-buffer" });
  await tick();
  assert.ok(
    !Object.prototype.hasOwnProperty.call(bridge.state.getMany(['sessions', 'chat', 'scheduled']).sessionBusy, "ordinary-done-without-buffer"),
    "an ordinary unknown chat:done must not create a background session buffer"
  );
}

async function authoritativeTurnSyncDoesNotCrossSessions() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-authority-a");
  harness.emit("chat:delta", { session_id: "chat-authority-a", text: "remote tail" });
  const authorityLoad = deferred();
  harness.handlers.load_session = function (args) {
    if (args.id === "chat-authority-a") return authorityLoad.promise;
    return { metadata: { id: args.id, title: "Other" }, messages: [], artifacts: [] };
  };
  harness.emit("chat:done", { session_id: "chat-authority-a" });
  await tick();
  const pendingSend = bridge.chat.sendMessage("must stay in A");
  await tick();
  assert.strictEqual(await bridge.sessions.switchToSession("chat-authority-b"), true);
  authorityLoad.resolve({
    metadata: { id: "chat-authority-a", title: "Authority A" },
    messages: [
      { role: "user", content: [{ type: "text", text: "remote prompt" }] },
      { role: "assistant", content: [{ type: "text", text: "remote tail" }] },
    ],
    artifacts: [],
    transcript_revision: "authority-a-final",
  });
  await pendingSend;
  await tick();
  assert.strictEqual(bridge.state.get('sessions').activeSessionId, "chat-authority-b");
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "chat"; }).length,
    0,
    "a send awaiting Session A reconciliation must never drift into newly active Session B"
  );
}

async function authoritativeHydrateDropsReplayedAssistantTail() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-authority-tail");
  harness.emit("chat:delta", { session_id: "chat-authority-tail", text: "answer tail" });
  harness.handlers.load_session = function (args) {
    return {
      metadata: { id: args.id, title: "Authority tail" },
      messages: [
        { role: "user", content: [{ type: "text", text: "question" }] },
        { role: "assistant", content: [{ type: "text", text: "complete answer tail" }] },
      ],
      artifacts: [],
      transcript_revision: "authority-tail-final",
    };
  };
  harness.emit("chat:done", { session_id: "chat-authority-tail" });
  await tick();
  await tick();
  const assistantItems = bridge.state.get('chat').chatItems.filter(function (item) {
    return item.type === "assistant";
  });
  assert.strictEqual(assistantItems.length, 1, "durable full answer must replace the replayed assistant tail");
  assert.strictEqual(
    assistantItems[0].text,
    "complete answer tail",
    "the canonical assistant source must come from the authoritative full answer"
  );
  assert.strictEqual(
    assistantItems.some(function (item) { return item.interruptedDisplayOnly === true; }),
    false,
    "the mid-turn replay tail must not be appended after the authoritative full answer"
  );
}

async function interruptedTurnRetainsDisplayOnlyPartial() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-interrupted-display-only";
  let durableMessages = [];
  harness.handlers.create_session = function () {
    return { id: sessionId, title: "New chat", transcript_revision: "empty" };
  };
  harness.handlers.list_sessions = function () {
    return [{ id: sessionId, title: "Interrupted display", message_count: durableMessages.length }];
  };
  harness.handlers.load_session = function () {
    return {
      metadata: {
        id: sessionId,
        title: "Interrupted display",
        message_count: durableMessages.length,
      },
      messages: JSON.parse(JSON.stringify(durableMessages)),
      artifacts: [],
      transcript_revision: "display-" + durableMessages.length,
    };
  };

  await bridge.sessions.createNewSession();
  await bridge.chat.sendMessage("first question");
  durableMessages = [
    { role: "user", content: [{ type: "text", text: "first question" }] },
  ];
  await harness.emit("chat:delta", { session_id: sessionId, text: "partial reply" });
  await harness.emit("chat:done", {
    session_id: sessionId,
    status: "Interrupted",
    error: null,
  });
  await tick();
  await tick();

  const interruptedState = bridge.state.get("chat");
  assert.strictEqual(
    interruptedState.messages.some(function (message) {
      return JSON.stringify(message).includes("partial reply");
    }),
    false,
    "an interrupted partial response must not enter the authoritative message list"
  );
  assert.ok(
    interruptedState.chatItems.some(function (item) {
      return item.type === "assistant" && item.interruptedDisplayOnly === true &&
        String(item.html || "").includes("partial reply");
    }),
    "the interrupted partial response must remain visible after authority reconciliation"
  );

  await bridge.chat.sendMessage("follow up");
  const followupState = bridge.state.get("chat");
  const firstUserIndex = followupState.chatItems.findIndex(function (item) {
    return item.type === "user" && item.text === "first question";
  });
  const partialIndex = followupState.chatItems.findIndex(function (item) {
    return item.type === "assistant" && item.interruptedDisplayOnly === true;
  });
  const followupIndex = followupState.chatItems.findIndex(function (item) {
    return item.type === "user" && item.text === "follow up";
  });
  assert.ok(
    firstUserIndex >= 0 && firstUserIndex < partialIndex && partialIndex < followupIndex,
    "the display-only partial must stay in its original turn when the user continues"
  );

  durableMessages = [
    { role: "user", content: [{ type: "text", text: "first question" }] },
    { role: "user", content: [{ type: "text", text: "follow up" }] },
    { role: "assistant", content: [{ type: "text", text: "complete follow-up" }] },
  ];
  await harness.emit("chat:delta", { session_id: sessionId, text: "complete follow-up" });
  await harness.emit("chat:done", {
    session_id: sessionId,
    status: "Completed",
    error: null,
  });
  await tick();
  await tick();

  const completedState = bridge.state.get("chat");
  const completedPartialIndex = completedState.chatItems.findIndex(function (item) {
    return item.type === "assistant" && item.interruptedDisplayOnly === true &&
      String(item.html || "").includes("partial reply");
  });
  const completedFollowupIndex = completedState.chatItems.findIndex(function (item) {
    return item.type === "user" && item.text === "follow up";
  });
  assert.ok(
    completedPartialIndex >= 0 && completedPartialIndex < completedFollowupIndex,
    "later authoritative refreshes must not move the interrupted partial into another turn"
  );
  assert.strictEqual(
    completedState.messages.some(function (message) {
      return JSON.stringify(message).includes("partial reply");
    }),
    false,
    "later turns must not promote the display-only partial into model context"
  );
}

async function remoteInterruptedTurnKeepsItsDisplayPosition() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-remote-interrupted-display";
  let durableMessages = [
    { role: "user", content: [{ type: "text", text: "older question" }] },
    { role: "assistant", content: [{ type: "text", text: "remote partial reply" }] },
  ];
  harness.handlers.load_session = function () {
    return {
      metadata: {
        id: sessionId,
        title: "Remote interrupted display",
        message_count: durableMessages.length,
      },
      messages: JSON.parse(JSON.stringify(durableMessages)),
      artifacts: [],
      transcript_revision: "remote-" + durableMessages.length,
    };
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", {
    session_id: sessionId,
    content: "remote question",
    operation: "append",
    base_transcript_revision: "remote-2",
  });
  await harness.emit("chat:turn_started", { session_id: sessionId });
  durableMessages = [...durableMessages,
    { role: "user", content: [{ type: "text", text: "remote question" }] },
  ];
  await harness.emit("chat:delta", {
    session_id: sessionId,
    text: "remote partial reply",
  });
  await harness.emit("chat:done", {
    session_id: sessionId,
    status: "Interrupted",
    error: null,
  });
  await tick();
  await tick();

  const chatItems = bridge.state.get("chat").chatItems;
  const olderUserIndex = chatItems.findIndex(function (item) {
    return item.type === "user" && item.text === "older question";
  });
  const remoteUserIndex = chatItems.findIndex(function (item) {
    return item.type === "user" && item.text === "remote question";
  });
  const partialIndex = chatItems.findIndex(function (item) {
    return item.type === "assistant" && item.interruptedDisplayOnly === true &&
      String(item.html || "").includes("remote partial reply");
  });
  assert.ok(
    olderUserIndex >= 0 && olderUserIndex < remoteUserIndex && remoteUserIndex < partialIndex,
    "a remote interrupted partial must stay after the remote user turn, not an older turn"
  );
}

async function interruptedTurnWithoutUserItemDropsPartial() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-interrupted-no-user-anchor";
  let durableMessages = [
    { role: "assistant", content: [{ type: "text", text: "old reply" }] },
  ];
  harness.handlers.list_sessions = function () {
    return [{ id: sessionId, title: "Unanchored interrupt", message_count: durableMessages.length }];
  };
  harness.handlers.load_session = function () {
    return {
      metadata: {
        id: sessionId,
        title: "Unanchored interrupt",
        message_count: durableMessages.length,
      },
      messages: JSON.parse(JSON.stringify(durableMessages)),
      artifacts: [],
      transcript_revision: "no-user-" + durableMessages.length,
    };
  };

  // transcript 里没有任何 user 消息(无 user 气泡、无法锚定轮次)的中断:
  // 不得把全部历史 assistant 项标记为仅展示,否则权威重载会在末尾复活它们,
  // 复制出整段历史的重复副本。
  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:turn_started", { session_id: sessionId });
  await harness.emit("chat:delta", { session_id: sessionId, text: "orphan partial" });
  await harness.emit("chat:done", {
    session_id: sessionId,
    status: "Interrupted",
    error: null,
  });
  await tick();
  await tick();

  const unanchoredState = bridge.state.get("chat");
  assert.ok(
    !unanchoredState.chatItems.some(function (item) {
      return item.interruptedDisplayOnly === true;
    }),
    "without any user turn anchor, nothing may be marked display-only"
  );
  assert.strictEqual(
    unanchoredState.chatItems.filter(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("old reply");
    }).length,
    1,
    "authority reconciliation must not duplicate the unanchored history"
  );
  assert.ok(
    !unanchoredState.chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("orphan partial");
    }),
    "an unanchorable interrupted partial must not be resurrected by authority reconciliation"
  );

  durableMessages = [
    { role: "assistant", content: [{ type: "text", text: "old reply" }] },
    { role: "user", content: [{ type: "text", text: "next question" }] },
    { role: "assistant", content: [{ type: "text", text: "clean reply" }] },
  ];
  await bridge.chat.sendMessage("next question");
  await harness.emit("chat:delta", { session_id: sessionId, text: "clean reply" });
  await harness.emit("chat:done", {
    session_id: sessionId,
    status: "Completed",
    error: null,
  });
  await tick();
  await tick();

  const followupState = bridge.state.get("chat");
  const assistantMessages = followupState.messages.filter(function (message) {
    return message.role === "assistant";
  });
  assert.ok(
    assistantMessages.length > 0 &&
      !JSON.stringify(assistantMessages).includes("orphan partial"),
    "the dropped partial must not leak into the next authoritative assistant message"
  );
}

async function completedTurnWaitsForAssistantInAuthoritySnapshot() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-created-authority-barrier";
  let authorityLoads = 0;
  let remoteTurnStarted = false;
  harness.handlers.load_session = function () {
    if (!remoteTurnStarted) {
      return {
        metadata: { id: sessionId, title: "New chat", message_count: 0 },
        messages: [], artifacts: [], transcript_revision: "empty",
      };
    }
    authorityLoads += 1;
    if (authorityLoads === 1) {
      return {
        metadata: { id: sessionId, title: "Question", message_count: 1 },
        messages: [{ role: "user", content: [{ type: "text", text: "question" }] }],
        artifacts: [],
        transcript_revision: "user-only-stale",
      };
    }
    return {
      metadata: { id: sessionId, title: "Question", message_count: 2 },
      messages: [
        { role: "user", content: [{ type: "text", text: "question" }] },
        { role: "assistant", content: [{ type: "text", text: "final answer" }] },
      ],
      artifacts: [],
      transcript_revision: "turn-final",
    };
  };

  await bridge.sessions.switchToSession(sessionId);
  remoteTurnStarted = true;
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  harness.emit("chat:delta", { session_id: sessionId, text: "final answer" });
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "turn-final",
  });
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();

  assert.ok(
    bridge.state.get('chat').chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("final answer");
    }),
    "a stale user-only snapshot must not erase the completed streaming assistant bubble"
  );
  await new Promise(function (resolve) { setTimeout(resolve, 300); });
  await tick();
  assert.ok(authorityLoads >= 2, "authority reconciliation should retry until the assistant is durable");
  assert.ok(
    bridge.state.get('chat').chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("final answer");
    }),
    "the durable assistant snapshot must converge without a visible reply disappearing"
  );
}

async function localCompletedTurnNeverBlocksTheNextMessage() {
  for (const bridgeKind of ["tauri", "web"]) {
    const harness = createBridgeHarness(null, {
      bridgeKind,
      setTimeout: function (callback) { return setImmediate(callback); },
    });
    const bridge = harness.bridge;
    const sessionId = "chat-local-terminal-nonblocking-" + bridgeKind;
    const durable = {
      metadata: { id: sessionId, title: "Local nonblocking", message_count: 0 },
      messages: [],
      artifacts: [],
      transcript_revision: "revision-before-local-turn",
    };
    harness.handlers.load_session = function () {
      return JSON.parse(JSON.stringify(durable));
    };

    await bridge.sessions.switchToSession(sessionId);
    await bridge.chat.sendMessage("first local question");
    await harness.emit("chat:delta", {
      session_id: sessionId,
      text: "first local answer",
    });

    // Even if a readback would still expose an older revision, chat:done for a
    // locally owned turn must release the input immediately. Rust emits the
    // committed marker only after persisting the terminal transcript; the UI
    // must not reclassify that local completion as a remote synchronization gate.
    await harness.emit("chat:transcript_committed", {
      session_id: sessionId,
      transcript_revision: "revision-after-local-turn",
    });
    await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
    for (let settleTick = 0; settleTick < 12; settleTick++) await tick();

    const afterDone = bridge.state.get("chat");
    assert.strictEqual(afterDone.busy, false,
      bridgeKind + " completed local turn must release busy immediately");
    assert.ok(
      !afterDone.chatItems.some(function (item) { return item && item.authoritySyncNotice; }),
      bridgeKind + " completed local turn must not surface an authority-sync warning"
    );

    await bridge.chat.sendMessage("second local question");
    const chatCommand = bridgeKind === "web" ? "web_access_chat" : "chat";
    assert.strictEqual(
      harness.calls.filter(function (call) { return call.cmd === chatCommand; }).length,
      2,
      bridgeKind + " stale readback must not prevent the next local message from reaching the engine"
    );
  }

  [tauriBridge, webBridge].forEach(function (source) {
    assert.ok(source.includes("completedLocalTurn"),
      "desktop and Web bridges must distinguish local completion from remote reconciliation");
    assert.ok(source.includes("authoritySyncNotice"),
      "desktop and Web bridges must deduplicate authority-sync notices");
  });
}

async function completedTurnUsesCommittedRevisionAsAuthority() {
  const harness = createBridgeHarness(null, {
    // A broken implementation exhausts all fallback retries. Keep that path
    // deterministic and fast instead of adding more than a second to the test.
    setTimeout: function (callback) { return setImmediate(callback); },
  });
  const bridge = harness.bridge;
  const sessionId = "chat-committed-revision-authority";
  let durable = {
    metadata: { id: sessionId, title: "Revision authority", message_count: 0 },
    messages: [],
    artifacts: [],
    transcript_revision: "revision-before-turn",
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  await harness.emit("chat:delta", {
    session_id: sessionId,
    text: "stream presentation",
  });

  // Native/provider tools may normalize the terminal assistant block before
  // persistence. The committed revision, rather than byte-identical streamed
  // presentation, proves which durable snapshot belongs to this turn.
  durable = {
    metadata: { id: sessionId, title: "Revision authority", message_count: 2 },
    messages: [
      { role: "user", content: [{ type: "text", text: "question" }] },
      { role: "assistant", content: [{ type: "text", text: "canonical persisted answer" }] },
    ],
    artifacts: [],
    transcript_revision: "revision-after-turn",
  };
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-after-turn",
  });
  await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
  await tick();
  await tick();

  const state = bridge.state.get("chat");
  assert.ok(
    state.chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("canonical persisted answer");
    }),
    "the committed revision must allow canonical hydration when streamed presentation differs"
  );
  assert.ok(
    !state.chatItems.some(function (item) {
      return item.type === "system" && String(item.text || "").includes("权威记录暂未同步");
    }),
    "a matching committed revision must not produce a false unsynced warning"
  );

  [tauriBridge, webBridge].forEach(function (source) {
    assert.ok(source.includes('remoteCommittedRevision = revision'),
      "desktop and Web bridges must both retain the committed turn revision");
    assert.ok(source.includes('savedRevision !== expectedCommittedRevision'),
      "desktop and Web bridges must both reconcile by committed revision");
  });
}

async function completedTurnKeepsWarningWhenRevisionMismatches() {
  const harness = createBridgeHarness(null, {
    setTimeout: function (callback) { return setImmediate(callback); },
  });
  const bridge = harness.bridge;
  const sessionId = "chat-revision-mismatch-warning";
  let durable = {
    metadata: { id: sessionId, title: "Revision mismatch", message_count: 0 },
    messages: [],
    artifacts: [],
    transcript_revision: "revision-stale",
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  await harness.emit("chat:delta", { session_id: sessionId, text: "stream presentation" });

  // 后端提交的 revision 与快照携带的 revision 不一致:持久化快照不属于本轮,
  // 必须保留同步警告,不得假装收敛。
  durable = {
    metadata: { id: sessionId, title: "Revision mismatch", message_count: 2 },
    messages: [
      { role: "user", content: [{ type: "text", text: "question" }] },
      { role: "assistant", content: [{ type: "text", text: "canonical persisted answer" }] },
    ],
    artifacts: [],
    transcript_revision: "revision-different-turn",
  };
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-committed-this-turn",
  });
  await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
  // 负向路径会耗尽 6 次重试:用 setImmediate 加速的轮次等待对账终态。
  for (let mismatchTick = 0; mismatchTick < 20; mismatchTick++) await tick();

  const state = bridge.state.get("chat");
  assert.ok(
    state.chatItems.some(function (item) {
      return item.type === "system" && String(item.text || "").includes("权威记录暂未同步");
    }),
    "a mismatched committed revision must keep the unsynced warning"
  );
}

async function completedTurnAdoptsLateCommittedRevision() {
  const harness = createBridgeHarness(null, {
    setTimeout: function (callback) { return setImmediate(callback); },
  });
  const bridge = harness.bridge;
  const sessionId = "chat-late-committed-revision";
  let durable = {
    metadata: { id: sessionId, title: "Late committed", message_count: 0 },
    messages: [],
    artifacts: [],
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  await harness.emit("chat:delta", { session_id: sessionId, text: "stream presentation" });

  // Relay replay 可能在 chat:done 之后才送达 commit marker:对账重试窗口内
  // 应拾取迟到的 revision 并收敛,而不是停留在较弱的展示身份回退上。
  durable = {
    metadata: { id: sessionId, title: "Late committed", message_count: 2 },
    messages: [
      { role: "user", content: [{ type: "text", text: "question" }] },
      { role: "assistant", content: [{ type: "text", text: "canonical persisted answer" }] },
    ],
    artifacts: [],
    transcript_revision: "revision-after-done",
  };
  await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
  await tick();
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-after-done",
  });
  for (let i = 0; i < 8; i++) await tick();

  const state = bridge.state.get("chat");
  assert.ok(
    state.chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("canonical persisted answer");
    }),
    "a commit marker delivered after done must be adopted during retry"
  );
  assert.ok(
    !state.chatItems.some(function (item) {
      return item.type === "system" && String(item.text || "").includes("权威记录暂未同步");
    }),
    "adopting the late committed revision must not warn"
  );
}

async function completedTurnFallsBackWhenSnapshotLacksRevision() {
  const harness = createBridgeHarness(null, {
    setTimeout: function (callback) { return setImmediate(callback); },
  });
  const bridge = harness.bridge;
  const sessionId = "chat-snapshot-no-revision";
  let durable = {
    metadata: { id: sessionId, title: "No revision", message_count: 0 },
    messages: [],
    artifacts: [],
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  await harness.emit("chat:delta", { session_id: sessionId, text: "final answer" });

  // 旧契约/旧后端快照不含 transcript_revision 字段:即使已收到 committed 事件,
  // 也不能因「期望非空但快照无字段」而必然失败,应回退到消息身份校验。
  durable = {
    metadata: { id: sessionId, title: "No revision", message_count: 2 },
    messages: [
      { role: "user", content: [{ type: "text", text: "question" }] },
      { role: "assistant", content: [{ type: "text", text: "final answer" }] },
    ],
    artifacts: [],
  };
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-committed-this-turn",
  });
  await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
  await tick();
  await tick();

  const state = bridge.state.get("chat");
  assert.ok(
    state.chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("final answer");
    }),
    "a snapshot without a revision field must fall back to identity reconciliation"
  );
  assert.ok(
    !state.chatItems.some(function (item) {
      return item.type === "system" && String(item.text || "").includes("权威记录暂未同步");
    }),
    "the fallback path must not produce a false unsynced warning"
  );
}

async function completedTurnAdoptsRevisionBumpDuringRetry() {
  const harness = createBridgeHarness(null, {
    setTimeout: function (callback) { return setImmediate(callback); },
  });
  const bridge = harness.bridge;
  const sessionId = "chat-revision-bump-during-retry";
  let durable = {
    metadata: { id: sessionId, title: "Revision bump", message_count: 0 },
    messages: [],
    artifacts: [],
    transcript_revision: "revision-before-turn",
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  await harness.emit("chat:delta", { session_id: sessionId, text: "stream presentation" });

  // 回合 1 提交 revision-after-turn 并完成;但此刻持久化快照仍停在
  // revision-before-turn(模拟落盘延迟),reconcile 第一次尝试必然失败。
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-after-turn",
  });
  await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
  await tick();

  // 在回合 1 的 reconcile 重试窗口内,回合 2 已提交并落盘:快照推进到
  // revision-2,committed 事件也到达。每 attempt 重读 live revision 的修复
  // 应收敛;若只在期望为空时重读(旧实现),6 次重试会一直拿 revision-after-turn
  // 比较 revision-2 → 全部失败 → 误报「权威记录暂未同步」。
  durable = {
    metadata: { id: sessionId, title: "Revision bump", message_count: 4 },
    messages: [
      { role: "user", content: [{ type: "text", text: "question" }] },
      { role: "assistant", content: [{ type: "text", text: "canonical answer 1" }] },
      { role: "user", content: [{ type: "text", text: "followup" }] },
      { role: "assistant", content: [{ type: "text", text: "canonical answer 2" }] },
    ],
    artifacts: [],
    transcript_revision: "revision-2",
  };
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-2",
  });
  for (let bumpTick = 0; bumpTick < 12; bumpTick++) await tick();

  const state = bridge.state.get("chat");
  assert.ok(
    state.chatItems.some(function (item) {
      return item.type === "assistant" && String(item.html || "").includes("canonical answer 2");
    }),
    "a revision bump during the retry window must be adopted by the running reconcile"
  );
  assert.ok(
    !state.chatItems.some(function (item) {
      return item.type === "system" && String(item.text || "").includes("权威记录暂未同步");
    }),
    "a revision bump during retry must not produce a false unsynced warning"
  );
}

async function editLastTurnBlockedWhileAuthorityReconcilePending() {
  const harness = createBridgeHarness(null, {
    setTimeout: function (callback) { return setImmediate(callback); },
  });
  const bridge = harness.bridge;
  const sessionId = "chat-edit-blocked-unsynced";
  let durable = {
    metadata: { id: sessionId, title: "Edit blocked", message_count: 0 },
    messages: [],
    artifacts: [],
    transcript_revision: "revision-before-turn",
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  await bridge.sessions.switchToSession(sessionId);
  await harness.emit("chat:user_message", { session_id: sessionId, content: "question" });
  await harness.emit("chat:delta", { session_id: sessionId, text: "stream answer" });

  // 快照 revision 与 committed 不匹配:权威对账必然失败,remoteTurnActive
  // 保持 true。此时编辑应被拦截(remoteTurnSyncing),而不是清掉 revision
  // 继续编辑——否则陈旧 committed 事件会在编辑中重武装旧 revision。
  durable = {
    metadata: { id: sessionId, title: "Edit blocked", message_count: 2 },
    messages: [
      { role: "user", content: [{ type: "text", text: "question" }] },
      { role: "assistant", content: [{ type: "text", text: "persisted answer" }] },
    ],
    artifacts: [],
    transcript_revision: "revision-stale",
  };
  await harness.emit("chat:transcript_committed", {
    session_id: sessionId,
    transcript_revision: "revision-committed-this-turn",
  });
  await harness.emit("chat:done", { session_id: sessionId, status: "Completed" });
  // 负向路径耗尽 6 次重试:setImmediate 加速的轮次等待对账终态。
  for (let mismatchTick = 0; mismatchTick < 20; mismatchTick++) await tick();

  const editCallsBefore = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; }).length;
  await bridge.interaction.editLastTurn("edited question");
  await bridge.interaction.editLastTurn("edited question again");
  const editCallsAfter = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; }).length;
  assert.strictEqual(
    editCallsAfter, editCallsBefore,
    "editing must be blocked while the authority reconcile is still pending"
  );

  const blockedState = bridge.state.get("chat");
  assert.strictEqual(
    blockedState.chatItems.filter(function (item) {
      return item && item.authoritySyncNotice;
    }).length,
    1,
    "repeated blocked actions must keep a single sync-pending notice"
  );
}

// Tool results also use role="user" in state.messages. Editing the last turn
// must cut at the genuine prompt and remove its complete tool round-trip.
async function editLastTurnCutsAtRealUserMessageBeforeToolResult(bridgeKind) {
  const harness = createBridgeHarness(null, { bridgeKind });
  const bridge = harness.bridge;
  const sessionId = "chat-edit-tool-result-" + bridgeKind;
  harness.handlers.load_session = function () {
    return {
      metadata: { id: sessionId, title: "Edit with tool result", message_count: 4 },
      messages: [
        { role: "user", content: [{ type: "text", text: "original question" }] },
        { role: "assistant", content: [{ type: "tool_use", id: "call_1", name: "Bash", input: {} }] },
        { role: "user", content: [{ type: "tool_result", tool_use_id: "call_1", content: "tool output" }] },
        { role: "assistant", content: [{ type: "text", text: "final answer" }] },
      ],
      artifacts: [],
    };
  };

  assert.strictEqual(await bridge.sessions.switchToSession(sessionId), true);
  const before = bridge.state.get("chat").messages;
  assert.strictEqual(before.length, 4, "seeded transcript must include the tool round-trip");

  await bridge.interaction.editLastTurn("edited question");
  const after = bridge.state.get("chat").messages;
  assert.deepStrictEqual(
    after,
    [{ role: "user", content: [{ type: "text", text: "edited question" }] }],
    "edit must cut the whole last turn including the trailing tool_result"
  );
  const editCalls = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; });
  assert.strictEqual(editCalls.length, 1, "edit_last_turn must be invoked exactly once");
}

async function rejectedEditRestoresAuthoritativeTranscript(bridgeKind) {
  const harness = createBridgeHarness(null, { bridgeKind });
  const bridge = harness.bridge;
  const sessionId = "chat-edit-rejected-" + bridgeKind;
  const durable = {
    metadata: { id: sessionId, title: "Rejected edit", message_count: 3 },
    messages: [
      { role: "user", content: [{ type: "text", text: "older editable prompt" }] },
      { role: "assistant", content: [{ type: "text", text: "older response" }] },
      { role: "user", content: [{ type: "image_url", image_url: { url: "data:image/png;base64,AAAA" } }] },
    ],
    artifacts: [],
    transcript_revision: "revision-before-rejected-edit",
  };
  harness.handlers.load_session = function () {
    return JSON.parse(JSON.stringify(durable));
  };

  assert.strictEqual(await bridge.sessions.switchToSession(sessionId), true);
  await bridge.interaction.editLastTurn("must not replace the older prompt");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(bridge.state.get("chat").messages)),
    [{ role: "user", content: [{ type: "text", text: "must not replace the older prompt" }] }],
    "the edit remains optimistic until the engine publishes its terminal decision"
  );

  await harness.emit("chat:done", {
    session_id: sessionId,
    status: "Failed",
    error: "Cannot edit the latest user content",
    operation_rejected: true,
  });
  for (let reconcileTick = 0; reconcileTick < 12; reconcileTick++) await tick();

  const state = bridge.state.get("chat");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(state.messages)),
    durable.messages,
    "a rejected edit must hydrate the unchanged durable transcript"
  );
  assert.strictEqual(state.busy, false, "a rejected edit must release the busy state");
}

async function remoteAcceptPlanConvergesAcrossClients() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let backendMode = "plan";
  const planSnapshot = {
    explanation: "execute the approved work",
    items: [{ step: "ship it", status: "pending" }],
  };
  harness.handlers.get_mode_state = function () { return { mode: backendMode }; };
  await bridge.sessions.switchToSession("chat-remote-plan-accept");
  harness.emit("chat:plan_ready", {
    session_id: "chat-remote-plan-accept",
    plan_id: "plan-ticket-remote-accept",
    mode_state: { mode: "plan" },
    plan_snapshot: planSnapshot,
  });
  const activePlan = bridge.state.get('chat').chatItems.find(function (item) {
    return item.type === "plan_card" && item.cardState === "active";
  });
  assert.ok(activePlan, "the remote-accept regression needs one actionable plan card");
  assert.strictEqual(activePlan.planId, "plan-ticket-remote-accept", "plan_ready must retain its backend ticket");

  backendMode = "yolo";
  harness.emit("chat:user_message", {
    session_id: "chat-remote-plan-accept",
    content: "✅ 就这么干",
    operation: "append",
    action: "accept_plan",
    plan_id: "plan-ticket-remote-accept",
    mode_state: { mode: "yolo" },
    base_transcript_revision: "plan-before-accept",
  });
  const admitted = bridge.state.getMany(['sessions', 'chat']);
  const admittedPlan = admitted.chatItems.find(function (item) { return item.id === activePlan.id; });
  assert.ok(admittedPlan && admittedPlan.resolved, "a remote accept must resolve the local active plan immediately");
  assert.notStrictEqual(admittedPlan.cardState, "active", "a remote accepted plan must stop being actionable");
  assert.strictEqual(admitted.modeState.mode, "yolo", "the accept event must synchronize the shared mode");
  assert.strictEqual(
    admitted.chatItems.filter(function (item) { return item.type === "user" && item.text === "✅ 就这么干"; }).length,
    1,
    "the remote admission should render exactly one user echo"
  );

  harness.handlers.load_session = function (args) {
    return {
      metadata: { id: args.id, title: "Remote accepted plan" },
      messages: [
        {
          role: "assistant",
          content: [{ type: "tool_use", id: "plan-tool-remote", name: "update_plan", input: {} }],
        },
        {
          role: "user",
          content: [{
            type: "tool_result",
            tool_use_id: "plan-tool-remote",
            content: "Plan updated:\n" + JSON.stringify(planSnapshot),
          }],
        },
        { role: "user", content: [{ type: "text", text: "✅ 就这么干" }] },
        { role: "assistant", content: [{ type: "text", text: "approved work complete" }] },
      ],
      artifacts: [],
      transcript_revision: "plan-after-accept",
    };
  };
  harness.emit("chat:transcript_committed", {
    session_id: "chat-remote-plan-accept",
    transcript_revision: "plan-after-accept",
  });
  harness.emit("chat:done", { session_id: "chat-remote-plan-accept" });
  await tick();
  await tick();

  const terminal = bridge.state.getMany(['sessions', 'chat']);
  const terminalPlans = terminal.chatItems.filter(function (item) { return item.type === "plan_card"; });
  assert.strictEqual(terminalPlans.length, 1, "terminal authority hydrate must not duplicate the resolved plan card");
  assert.ok(
    terminalPlans.every(function (item) { return item.resolved && item.cardState !== "active"; }),
    "the authoritative plan history must contain no stale action button"
  );
  assert.strictEqual(
    terminalPlans[0].planId,
    "plan-ticket-remote-accept",
    "terminal authority hydrate must carry the resolved ticket onto the durable plan card"
  );
  assert.strictEqual(terminal.modeState.mode, "yolo", "terminal hydrate must retain the accepted Yolo mode");
  assert.strictEqual(
    terminal.chatItems.filter(function (item) { return item.type === "user" && item.text === "✅ 就这么干"; }).length,
    1,
    "authority hydrate must replace, not duplicate, the remote user echo"
  );
}

async function activePlanSurvivesUnrelatedTerminalHydrate() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  harness.handlers.get_mode_state = function () { return { mode: "plan" }; };
  await bridge.sessions.switchToSession("chat-active-plan-survives");
  harness.emit("chat:plan_ready", {
    session_id: "chat-active-plan-survives",
    plan_id: "plan-ticket-active-survives",
    mode_state: { mode: "plan" },
    plan_snapshot: { items: [{ step: "still awaiting approval", status: "pending" }] },
  });
  harness.emit("chat:user_message", {
    session_id: "chat-active-plan-survives",
    content: "one more clarification",
    operation: "append",
    base_transcript_revision: "active-plan-before-followup",
  });
  harness.handlers.load_session = function (args) {
    return {
      metadata: { id: args.id, title: "Active plan survives" },
      messages: [
        { role: "user", content: [{ type: "text", text: "one more clarification" }] },
        { role: "assistant", content: [{ type: "text", text: "clarification noted" }] },
      ],
      artifacts: [],
      transcript_revision: "active-plan-after-followup",
    };
  };
  harness.emit("chat:done", { session_id: "chat-active-plan-survives" });
  await tick();
  await tick();
  const plans = bridge.state.get('chat').chatItems.filter(function (item) { return item.type === "plan_card"; });
  assert.strictEqual(plans.length, 1, "an unrelated terminal hydrate must retain the genuinely active plan");
  assert.strictEqual(plans[0].cardState, "active");
  assert.strictEqual(plans[0].resolved, false);
}

async function activePlanHydrateMigratesTicketWithoutDuplicate() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-active-plan-hydrate";
  const planId = "plan-ticket-active-hydrate";
  const planSnapshot = {
    explanation: "hydrate the canonical plan card",
    items: [{ step: "keep one actionable card", status: "pending" }],
  };
  harness.handlers.get_mode_state = function () { return { mode: "plan" }; };
  await bridge.sessions.switchToSession(sessionId);
  harness.emit("chat:turn_started", { session_id: sessionId });
  harness.emit("chat:plan_ready", {
    session_id: sessionId,
    plan_id: planId,
    mode_state: { mode: "plan" },
    plan_snapshot: planSnapshot,
  });
  harness.handlers.load_session = function (args) {
    return {
      metadata: { id: args.id, title: "Active plan hydrate" },
      messages: [
        { role: "user", content: [{ type: "text", text: "make a plan" }] },
        {
          role: "assistant",
          content: [{ type: "tool_use", id: "plan-tool-active-hydrate", name: "update_plan", input: {} }],
        },
        {
          role: "user",
          content: [{
            type: "tool_result",
            tool_use_id: "plan-tool-active-hydrate",
            content: "Plan updated:\n" + JSON.stringify(planSnapshot),
          }],
        },
      ],
      artifacts: [],
      transcript_revision: "active-plan-hydrate-final",
    };
  };
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();

  const hydratedPlans = bridge.state.get('chat').chatItems.filter(function (item) {
    return item.type === "plan_card";
  });
  assert.strictEqual(
    hydratedPlans.length,
    1,
    "plan_ready followed by its durable transcript must render one canonical plan card"
  );
  assert.strictEqual(hydratedPlans[0].planId, planId, "hydrate must migrate the live plan ticket");
  assert.strictEqual(hydratedPlans[0].cardState, "active", "the canonical hydrated card must remain actionable");
  assert.strictEqual(hydratedPlans[0].resolved, false);

  harness.handlers.accept_plan = function (args) {
    assert.strictEqual(args.sessionId, sessionId);
    assert.strictEqual(args.planId, planId, "the migrated ticket must reach accept_plan unchanged");
    return { mode: "yolo" };
  };
  await bridge.interaction.acceptPlan(
    hydratedPlans[0].id,
    hydratedPlans[0].planMarkdown,
    undefined,
    hydratedPlans[0].planId
  );
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "accept_plan"; }).length,
    1,
    "the deduplicated canonical plan card must still be executable"
  );
}

async function planNotActiveRollbackFreezesStaleCard() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let backendMode = "plan";
  harness.handlers.get_mode_state = function () { return { mode: backendMode }; };
  harness.handlers.accept_plan = function (args) {
    assert.strictEqual(args.planId, "plan-ticket-stale", "accept_plan must carry the exact plan ticket");
    throw new Error("accept_plan: plan_not_active");
  };
  await bridge.sessions.switchToSession("chat-stale-plan");
  harness.emit("chat:plan_ready", {
    session_id: "chat-stale-plan",
    plan_id: "plan-ticket-stale",
    mode_state: { mode: "plan" },
    plan_snapshot: { items: [{ step: "stale work", status: "pending" }] },
  });
  const plan = bridge.state.get('chat').chatItems.find(function (item) { return item.type === "plan_card"; });
  assert.ok(plan && plan.cardState === "active");
  backendMode = "yolo";
  await bridge.interaction.acceptPlan(plan.id, plan.planMarkdown, undefined, plan.planId);
  const rolledBack = bridge.state.getMany(['sessions', 'chat']);
  const stalePlan = rolledBack.chatItems.find(function (item) { return item.id === plan.id; });
  assert.ok(stalePlan && stalePlan.resolved, "plan_not_active must freeze the stale local card");
  assert.notStrictEqual(stalePlan.cardState, "active", "plan_not_active must never restore an old action button");
  assert.strictEqual(rolledBack.modeState.mode, "yolo", "plan_not_active must resynchronize backend mode");
}

async function planTicketCommandsAndRemoteDiscardConverge() {
  const localHarness = createBridgeHarness();
  const localBridge = localHarness.bridge;
  localHarness.handlers.get_mode_state = function () { return { mode: "plan" }; };
  localHarness.handlers.discard_plan = function (args) {
    assert.strictEqual(args.sessionId, "chat-local-plan-discard");
    assert.strictEqual(args.planId, "plan-ticket-local-discard", "discard_plan must carry the exact plan ticket");
    return { mode: "plan" };
  };
  await localBridge.sessions.switchToSession("chat-local-plan-discard");
  localHarness.emit("chat:plan_ready", {
    session_id: "chat-local-plan-discard",
    plan_id: "plan-ticket-local-discard",
    mode_state: { mode: "plan" },
    plan_snapshot: { items: [{ step: "discard locally", status: "pending" }] },
  });
  const localPlan = localBridge.state.get('chat').chatItems.find(function (item) { return item.type === "plan_card"; });
  await localBridge.interaction.discardPlan(localPlan.id, localPlan.planId);
  assert.strictEqual(
    localHarness.calls.filter(function (call) { return call.cmd === "discard_plan"; }).length,
    1,
    "one local discard should issue exactly one ticketed command"
  );
  await localBridge.interaction.acceptPlan(localPlan.id, localPlan.planMarkdown, undefined, localPlan.planId);
  assert.strictEqual(
    localHarness.calls.filter(function (call) { return call.cmd === "accept_plan"; }).length,
    0,
    "a locally discarded card must not be executable again"
  );

  const remoteHarness = createBridgeHarness();
  const remoteBridge = remoteHarness.bridge;
  remoteHarness.handlers.get_mode_state = function () { return { mode: "plan" }; };
  await remoteBridge.sessions.switchToSession("chat-remote-plan-discard");
  remoteHarness.emit("chat:plan_ready", {
    session_id: "chat-remote-plan-discard",
    plan_id: "plan-ticket-remote-discard",
    mode_state: { mode: "plan" },
    plan_snapshot: { items: [{ step: "discard remotely", status: "pending" }] },
  });
  const remotePlan = remoteBridge.state.get('chat').chatItems.find(function (item) { return item.type === "plan_card"; });
  const userCountBefore = remoteBridge.state.get('chat').chatItems.filter(function (item) { return item.type === "user"; }).length;
  remoteHarness.emit("chat:plan_resolved", {
    session_id: "chat-remote-plan-discard",
    plan_id: "plan-ticket-remote-discard",
    mode_state: { mode: "plan" },
  });
  const remotelyResolved = remoteBridge.state.get('chat');
  const resolvedCard = remotelyResolved.chatItems.find(function (item) { return item.id === remotePlan.id; });
  assert.ok(resolvedCard && resolvedCard.resolved && resolvedCard.cardState === "frozen",
    "chat:plan_resolved must immediately freeze the matching remote card");
  assert.strictEqual(remotelyResolved.busy, false, "discard resolution must not create a model turn");
  assert.strictEqual(
    remotelyResolved.chatItems.filter(function (item) { return item.type === "user"; }).length,
    userCountBefore,
    "discard resolution must not synthesize a user bubble"
  );
  assert.strictEqual(remotelyResolved.modeState.mode, "plan", "discard resolution must apply its mode snapshot");
  await remoteBridge.interaction.acceptPlan(remotePlan.id, remotePlan.planMarkdown, undefined, remotePlan.planId);
  assert.strictEqual(
    remoteHarness.calls.filter(function (call) { return call.cmd === "accept_plan"; }).length,
    0,
    "a remotely discarded ticket must not reach accept_plan"
  );
}

async function discardPlanFailureRollbackFollowsTicketAuthority() {
  const transientHarness = createBridgeHarness();
  const transientBridge = transientHarness.bridge;
  transientHarness.handlers.get_mode_state = function () { return { mode: "plan" }; };
  transientHarness.handlers.discard_plan = function () { throw new Error("relay temporarily unavailable"); };
  await transientBridge.sessions.switchToSession("chat-discard-transient");
  transientHarness.emit("chat:plan_ready", {
    session_id: "chat-discard-transient",
    plan_id: "plan-ticket-discard-transient",
    mode_state: { mode: "plan" },
    plan_snapshot: { items: [{ step: "retry discard", status: "pending" }] },
  });
  const transientPlan = transientBridge.state.get('chat').chatItems.find(function (item) { return item.type === "plan_card"; });
  await transientBridge.interaction.discardPlan(transientPlan.id, transientPlan.planId);
  const restoredPlan = transientBridge.state.get('chat').chatItems.find(function (item) { return item.id === transientPlan.id; });
  assert.ok(restoredPlan && restoredPlan.cardState === "active" && restoredPlan.resolved === false,
    "a definite transient discard failure must restore the still-valid ticket");

  const staleHarness = createBridgeHarness();
  const staleBridge = staleHarness.bridge;
  let backendMode = "plan";
  staleHarness.handlers.get_mode_state = function () { return { mode: backendMode }; };
  staleHarness.handlers.discard_plan = function () { throw new Error("discard_plan: plan_not_active"); };
  await staleBridge.sessions.switchToSession("chat-discard-stale");
  staleHarness.emit("chat:plan_ready", {
    session_id: "chat-discard-stale",
    plan_id: "plan-ticket-discard-stale",
    mode_state: { mode: "plan" },
    plan_snapshot: { items: [{ step: "already resolved", status: "pending" }] },
  });
  const stalePlan = staleBridge.state.get('chat').chatItems.find(function (item) { return item.type === "plan_card"; });
  backendMode = "yolo";
  await staleBridge.interaction.discardPlan(stalePlan.id, stalePlan.planId);
  const frozenPlan = staleBridge.state.get('chat').chatItems.find(function (item) { return item.id === stalePlan.id; });
  assert.ok(frozenPlan && frozenPlan.cardState === "frozen" && frozenPlan.resolved,
    "plan_not_active must keep the stale discard ticket frozen");
  assert.strictEqual(staleBridge.state.get('chat').modeState.mode, "yolo",
    "plan_not_active discard must resynchronize the authoritative mode");
}

async function failedRunningOpenRollsBackOnlyItsProvisionalBusy() {
  const failedLoadHarness = createBridgeHarness();
  await failedLoadHarness.bridge.sessions.switchToSession("chat-origin");
  failedLoadHarness.handlers.load_session = function (args) {
    if (args.id === "sched-open-load-fails") throw new Error("scheduled load failed");
    return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
  };
  assert.strictEqual(await failedLoadHarness.bridge.scheduled.openScheduledRunChat({
    id: "run-open-load-fails",
    automationId: "automation-open-load-fails",
    sessionId: "sched-open-load-fails",
    status: "running",
  }, { name: "Load failure task" }), false);
  assert.strictEqual(failedLoadHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, "chat-origin");
  assert.ok(
    !failedLoadHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).sessionBusy["sched-open-load-fails"],
    "a failed running open must roll back the provisional busy flag it introduced"
  );

  const staleRequestHarness = createBridgeHarness();
  const targetLoad = deferred();
  staleRequestHarness.handlers.load_session = function (args) {
    if (args.id === "sched-open-stale-request") return targetLoad.promise;
    return { metadata: { id: args.id, title: "Other" }, messages: [], artifacts: [] };
  };
  await staleRequestHarness.bridge.sessions.switchToSession("chat-origin");
  const staleOpening = staleRequestHarness.bridge.scheduled.openScheduledRunChat({
    id: "run-open-stale-request",
    automationId: "automation-open-stale-request",
    sessionId: "sched-open-stale-request",
    status: "running",
  }, { name: "Stale open task" });
  await tick();
  assert.strictEqual(await staleRequestHarness.bridge.sessions.switchToSession("chat-other"), true);
  targetLoad.resolve({
    metadata: { id: "sched-open-stale-request", title: "Stale scheduled load" },
    messages: [], artifacts: [],
  });
  assert.strictEqual(await staleOpening, false);
  assert.strictEqual(staleRequestHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, "chat-other");
  assert.ok(
    !staleRequestHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).sessionBusy["sched-open-stale-request"],
    "an invalidated running open must roll back only its provisional busy flag"
  );

  const liveHarness = createBridgeHarness();
  await liveHarness.bridge.sessions.switchToSession("chat-origin");
  const liveRun = {
    id: "run-open-live",
    automationId: "automation-open-live",
    sessionId: "sched-open-live",
    status: "running",
  };
  assert.strictEqual(await liveHarness.bridge.scheduled.openScheduledRunChat(liveRun, { name: "Live task" }), true);
  liveHarness.emit("chat:delta", { session_id: liveRun.sessionId, text: "real live output" });
  assert.strictEqual(await liveHarness.bridge.scheduled.exitScheduledRunChat(), true);
  liveHarness.handlers.load_session = function (args) {
    if (args.id === liveRun.sessionId) throw new Error("reopen failed");
    return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
  };
  assert.strictEqual(await liveHarness.bridge.scheduled.openScheduledRunChat(liveRun, { name: "Live task" }), false);
  assert.strictEqual(
    liveHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).sessionBusy[liveRun.sessionId],
    true,
    "failure rollback must not clear a busy phase that existed before this open attempt"
  );
}

async function concurrentFailedRunningOpensShareRollback() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let loadCalls = 0;
  await bridge.sessions.switchToSession("chat-origin");
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-double-open-fails") {
      loadCalls += 1;
      throw new Error("double open load failed");
    }
    return { metadata: { id: args.id, title: "Origin" }, messages: [], artifacts: [] };
  };
  const run = {
    id: "run-double-open-fails",
    automationId: "automation-double-open-fails",
    sessionId: "sched-double-open-fails",
    status: "running",
  };
  const first = bridge.scheduled.openScheduledRunChat(run, { name: "Double open task" });
  const second = bridge.scheduled.openScheduledRunChat(run, { name: "Double open task" });
  const results = await Promise.all([first, second]);

  assert.deepStrictEqual(results, [false, false]);
  assert.strictEqual(loadCalls, 1, "concurrent opens for one scheduled session must share one durable load");
  assert.ok(
    !bridge.state.getMany(['sessions', 'chat', 'scheduled']).sessionBusy[run.sessionId],
    "the shared failed open must roll back provisional busy after its final caller settles"
  );
}

async function scheduledOwnerRegistryIsBoundedAndProtectsLive() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-origin");
  const liveRun = {
    id: "run-owner-live",
    automationId: "automation-owner-live",
    sessionId: "owned-owner-live",
    status: "running",
  };
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(liveRun, { name: "Owner live task" }), true);
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
  bridge.scheduled.selectScheduledTask("automation-owner-history");
  await bridge.scheduled.loadScheduledTaskRuns("automation-owner-history", 80);
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, liveRun.sessionId);
  assert.ok(
    JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems).includes("protected owner live output"),
    "owner pruning must preserve the current live scheduled conversation"
  );
  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);

  harness.emit("chat:done", { session_id: "owned-owner-79" });
  await tick();
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-owner-79",
    automationId: "automation-owner-history",
    sessionId: "owned-owner-79",
    status: "running",
  }, { name: "Pruned owner task" }), true);
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy,
    true,
    "hard cap must evict lower-priority visible owners once current/context consume registry slots"
  );
  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);

  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-owner-0",
    automationId: "automation-owner-history",
    sessionId: "owned-owner-0",
    status: "running",
  }, { name: "Visible owner task" }), true);
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy,
    false,
    "the most recent visible terminal run owner must survive registry pruning"
  );

  const busyHarness = createBridgeHarness();
  await busyHarness.bridge.sessions.switchToSession("chat-origin");
  for (let busyIndex = 0; busyIndex < 70; busyIndex++) {
    assert.strictEqual(await busyHarness.bridge.scheduled.openScheduledRunChat({
      id: "run-owner-busy-" + busyIndex,
      automationId: "automation-owner-busy-" + busyIndex,
      sessionId: "owned-owner-busy-" + busyIndex,
      status: "running",
    }, { name: "Busy owner " + busyIndex }), true);
    assert.strictEqual(await busyHarness.bridge.scheduled.exitScheduledRunChat(), true);
  }
  busyHarness.emit("chat:done", { session_id: "owned-owner-busy-0" });
  await tick();
  assert.strictEqual(await busyHarness.bridge.scheduled.openScheduledRunChat({
    id: "run-owner-busy-0",
    automationId: "automation-owner-busy-0",
    sessionId: "owned-owner-busy-0",
    status: "running",
  }, { name: "Busy owner 0" }), true);
  assert.strictEqual(
    busyHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy,
    false,
    "a live buffer must remain recognizable after its separate owner registry entry is hard-capped"
  );
}

async function scheduledBufferLruNeverEvictsLive() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  await bridge.sessions.switchToSession("chat-origin");

  harness.emit("chat:delta", {
    session_id: "sched-lru-cold",
    text: "cold buffer should be evicted",
  });
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
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
  await bridge.chat.sendMessage("queued live follow-up");
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).queued.length, 1);
  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);

  for (let i = 0; i < 70; i++) {
    harness.emit("chat:delta", {
      session_id: "sched-lru-cold-" + i,
      text: "cold " + i,
    });
  }

  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-lru-cold",
    automationId: "automation-lru-cold",
    sessionId: "sched-lru-cold",
    status: "running",
    unread: false,
  }, { id: "automation-lru-cold", name: "LRU cold task" }), true);
  assert.ok(
    !JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems).includes("cold buffer should be evicted"),
    "an inactive scheduled buffer older than the 64-entry cap should be evicted"
  );
  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);

  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-lru-live",
    automationId: "automation-lru-live",
    sessionId: "sched-lru-live",
    status: "running",
    unread: false,
  }, { id: "automation-lru-live", name: "LRU live task" }), true);
  const live = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.ok(
    JSON.stringify(live.chatItems).includes("live buffer must survive"),
    "LRU must never evict a busy scheduled buffer"
  );
  assert.strictEqual(live.queued.length, 1, "LRU must never evict a scheduled buffer with queued input");

  const saturatedHarness = createBridgeHarness();
  await saturatedHarness.bridge.sessions.switchToSession("chat-origin");
  for (let protectedIndex = 0; protectedIndex < 64; protectedIndex++) {
    assert.strictEqual(await saturatedHarness.bridge.scheduled.openScheduledRunChat({
      id: "run-protected-" + protectedIndex,
      automationId: "automation-protected-" + protectedIndex,
      sessionId: "sched-protected-" + protectedIndex,
      status: "running",
    }, { name: "Protected task " + protectedIndex }), true);
    assert.strictEqual(await saturatedHarness.bridge.scheduled.exitScheduledRunChat(), true);
  }
  assert.strictEqual(await saturatedHarness.bridge.scheduled.openScheduledRunChat({
    id: "run-protected-new",
    automationId: "automation-protected-new",
    sessionId: "sched-protected-new",
    status: "running",
  }, { name: "New protected task" }), true);
  assert.strictEqual(
    saturatedHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy,
    true,
    "when all 64 older buffers are live, LRU must retain the newly opened running buffer too"
  );
}

async function scheduledTemplateSourcePersistenceBehavior() {
  const sharedStorage = Object.create(null);
  const first = createBridgeHarness(sharedStorage);
  let backendInput = null;
  first.handlers.create_scheduled_task = function (args) {
    backendInput = args.input;
    return Object.assign({ id: "automation-template" }, args.input);
  };
  // createScheduledTask 成功后会立即重拉任务列表;真实后端此时必然已包含新任务。
  first.handlers.list_scheduled_tasks = function () {
    return backendInput ? [Object.assign({ id: "automation-template" }, backendInput)] : [];
  };
  const created = await first.bridge.scheduled.createScheduledTask({
    name: "Completely renamed",
    prompt: "Completely edited prompt",
    rrule: "FREQ=HOURLY;INTERVAL=3",
    templateId: "weekly-review",
  });
  assert.ok(!Object.prototype.hasOwnProperty.call(backendInput, "templateId"), "UI-only template ids must not leak into the base request");
  assert.strictEqual(created.templateId, "weekly-review");

  const second = createBridgeHarness(sharedStorage);
  second.handlers.list_scheduled_tasks = function () {
    return [{
      id: "automation-template",
      name: "Completely renamed",
      prompt: "Completely edited prompt",
      rrule: "FREQ=HOURLY;INTERVAL=3",
    }];
  };
  await second.bridge.scheduled.loadScheduledTasks();
  assert.strictEqual(
    second.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTasks[0].templateId,
    "weekly-review",
    "template source must survive a bridge reload even when every visible template field was customized"
  );
}

async function scheduledUnreadPollingRaceBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const task = { id: "automation-race", name: "Race task", hasUnreadRuns: true };
  const run = {
    id: "run-race", automationId: task.id, sessionId: "sched-race",
    status: "completed", unread: true,
  };
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.read_scheduled_task = function () { return Object.assign({}, task); };
  harness.handlers.list_scheduled_task_runs = function () { return [Object.assign({}, run)]; };
  harness.handlers.mark_scheduled_run_viewed = function () {
    return { automationId: task.id, runId: run.id, hasUnreadRuns: false };
  };
  await bridge.scheduled.loadScheduledTasks();
  bridge.scheduled.selectScheduledTask(task.id);
  await bridge.scheduled.readScheduledTask(task.id);
  await bridge.scheduled.loadScheduledTaskRuns(task.id, 20);

  const staleTasks = deferred();
  const staleDetail = deferred();
  const staleRuns = deferred();
  harness.handlers.list_scheduled_tasks = function () { return staleTasks.promise; };
  harness.handlers.read_scheduled_task = function () { return staleDetail.promise; };
  harness.handlers.list_scheduled_task_runs = function () { return staleRuns.promise; };
  const staleRefresh = bridge.scheduled.refreshScheduledTaskData(20);
  await tick();

  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(run, task), true);
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRuns[0].unread, false);
  staleTasks.resolve([Object.assign({}, task)]);
  staleDetail.resolve(Object.assign({}, task));
  staleRuns.resolve([Object.assign({}, run)]);
  await staleRefresh;
  const finalState = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(finalState.scheduledTaskRuns[0].unread, false, "an older poll must not resurrect a viewed run dot");
  assert.strictEqual(finalState.scheduledTasks[0].hasUnreadRuns, false, "an older poll must not resurrect the task aggregate dot");
  assert.strictEqual(finalState.scheduledTaskDetail.hasUnreadRuns, false);
}

async function scheduledRunNavigationBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;

  assert.strictEqual(await bridge.sessions.switchToSession("chat-origin"), true);
  bridge.scheduled.selectScheduledTask("automation-1");
  await bridge.scheduled.readScheduledTask("automation-1");
  harness.handlers.list_scheduled_task_runs = function () {
    return [{ id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" }];
  };
  await bridge.scheduled.loadScheduledTaskRuns("automation-1", 20);
  assert.strictEqual(
    await bridge.scheduled.openScheduledRunChat(
      { id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" },
      { id: "automation-1", name: "Nightly report", model: "/locked-model", mode: "plan" }
    ),
    true
  );
  await bridge.chat.sendMessage("continue the scheduled conversation");
  const followup = harness.calls.filter(function (call) { return call.cmd === "chat"; }).pop();
  assert.strictEqual(followup.args.sessionId, "sched-run-1");
  assert.strictEqual(followup.args.restrictTools, false);
  await harness.emit("chat:done", { session_id: "sched-run-1", status: "Completed", error: null });
  const editCallsBefore = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; }).length;
  await bridge.interaction.editLastTurn("rewrite scheduled output");
  const editCalls = harness.calls.filter(function (call) { return call.cmd === "edit_last_turn"; });
  assert.strictEqual(editCalls.length, editCallsBefore + 1);
  assert.strictEqual(editCalls[editCalls.length - 1].args.sessionId, "sched-run-1");
  const opened = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
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

  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  const restored = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(restored.activeSessionId, "chat-origin");
  assert.strictEqual(restored.scheduledRunContext, null);
  assert.strictEqual(restored.selectedScheduledTaskId, "automation-1");
  assert.strictEqual(restored.scheduledTaskDetail.id, "automation-1");
  assert.strictEqual(restored.scheduledTaskRuns[0].id, "run-1");

  await bridge.scheduled.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" },
    { id: "automation-1", name: "Nightly report" }
  );
  await bridge.sessions.createNewSession();
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, null);
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledRunContext, null);

  await bridge.sessions.switchToSession("chat-origin");
  await bridge.scheduled.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId: "sched-run-1", status: "completed" },
    { id: "automation-1", name: "Nightly report", model: "/locked-model", mode: "plan" }
  );
  await bridge.sessions.switchToSession("chat-origin");
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledRunContext, null);

  harness.handlers.load_session = function (args) {
    if (args.id === "sched-missing") throw new Error("missing scheduled session");
    return {
      metadata: { id: args.id, title: "New chat" },
      messages: [],
      artifacts: [],
    };
  };
  const chatItemsBeforeFailure = JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems);
  assert.strictEqual(
    await bridge.scheduled.openScheduledRunChat(
      { id: "run-missing", automationId: "automation-1", sessionId: "sched-missing", status: "completed" },
      { name: "Missing run" }
    ),
    false
  );
  const failedOpen = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(failedOpen.activeSessionId, "chat-origin");
  assert.strictEqual(failedOpen.scheduledRunContext, null);
  assert.ok(String(failedOpen.scheduledTaskError).includes("missing scheduled session"));
  assert.strictEqual(JSON.stringify(failedOpen.chatItems), chatItemsBeforeFailure, "scheduled load errors must not pollute chat");
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({ id: "no-session", status: "completed" }, {}), false);
  assert.ok(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskError, "missing run sessions should expose a scheduled-scoped error");
}

async function scheduledSelectionGenerationBehavior() {
  const harness = createBridgeHarness();
  const listA = deferred();
  const listB = deferred();
  let listCalls = 0;
  harness.handlers.list_scheduled_tasks = function () {
    listCalls += 1;
    return listCalls === 1 ? listA.promise : listB.promise;
  };

  harness.bridge.scheduled.selectScheduledTask("automation-a");
  const tasksA = harness.bridge.scheduled.loadScheduledTasks();
  harness.bridge.scheduled.selectScheduledTask("automation-b");
  const tasksB = harness.bridge.scheduled.loadScheduledTasks();
  listB.resolve([{ id: "automation-b", name: "B" }]);
  await tasksB;
  assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskLoading, false, "an old task-list generation must not keep the current selection loading");
  listA.resolve([{ id: "automation-a", name: "A" }]);
  await tasksA;
  let state = harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(state.selectedScheduledTaskId, "automation-b");
  assert.strictEqual(state.scheduledTasks[0].id, "automation-b", "a stale task list must not replace the current generation");

  const detailA = deferred();
  const detailB = deferred();
  const runsA = deferred();
  const runsB = deferred();
  harness.handlers.read_scheduled_task = function (args) {
    return args.id === "automation-a" ? detailA.promise : detailB.promise;
  };
  harness.handlers.list_scheduled_task_runs = function (args) {
    return args.id === "automation-a" ? runsA.promise : runsB.promise;
  };

  harness.bridge.scheduled.selectScheduledTask("automation-a");
  const readA = harness.bridge.scheduled.readScheduledTask("automation-a");
  const loadRunsA = harness.bridge.scheduled.loadScheduledTaskRuns("automation-a", 20);
  harness.bridge.scheduled.selectScheduledTask("automation-b");
  const readB = harness.bridge.scheduled.readScheduledTask("automation-b");
  const loadRunsB = harness.bridge.scheduled.loadScheduledTaskRuns("automation-b", 20);
  detailB.resolve({ id: "automation-b", name: "B detail" });
  runsB.resolve([{ id: "run-b", automationId: "automation-b" }]);
  await Promise.all([readB, loadRunsB]);
  assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskLoading, false, "old detail/run requests must not keep the current selection loading");
  detailA.resolve({ id: "automation-a", name: "A detail" });
  runsA.resolve([{ id: "run-a", automationId: "automation-a" }]);
  await Promise.all([readA, loadRunsA]);
  state = harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(state.selectedScheduledTaskId, "automation-b");
  assert.strictEqual(state.scheduledTaskDetail.id, "automation-b");
  assert.strictEqual(state.scheduledTaskRuns[0].id, "run-b");
  assert.strictEqual(state.scheduledTaskLoading, false);

  let refreshes = 0;
  let aggregateRefreshes = 0;
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
  harness.handlers.list_scheduled_runs = function () {
    aggregateRefreshes += 1;
    return [];
  };
  harness.emit("scheduled_task:run_updated", { automationId: "automation-a" });
  harness.emit("scheduled_task:run_updated", { automationId: "automation-b" });
  await new Promise(function (resolve) { setTimeout(resolve, 450); });
  await tick();
  assert.strictEqual(listCalls, 3, "burst run events should debounce to one global task refresh");
  assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTasks[0].hasUnreadRuns, true, "the unselected task unread summary should enter global state");
  assert.strictEqual(refreshes, 1, "the selected task detail should refresh once after the event burst");
  assert.strictEqual(aggregateRefreshes, 1, "the global scheduled-run sidebar should refresh once after the event burst");
}

async function scheduledRefreshDoesNotOverlap() {
  const harness = createBridgeHarness();
  const pendingTasks = deferred();
  const pendingDetail = deferred();
  const pendingRuns = deferred();
  const counts = { tasks: 0, detail: 0, runs: 0 };
  harness.handlers.list_scheduled_tasks = function () { counts.tasks += 1; return pendingTasks.promise; };
  harness.handlers.read_scheduled_task = function () { counts.detail += 1; return pendingDetail.promise; };
  harness.handlers.list_scheduled_task_runs = function () { counts.runs += 1; return pendingRuns.promise; };
  harness.bridge.scheduled.selectScheduledTask("automation-b");

  const first = harness.bridge.scheduled.refreshScheduledTaskData(20);
  const overlapping = harness.bridge.scheduled.refreshScheduledTaskData(20);
  await tick();
  assert.deepStrictEqual(counts, { tasks: 1, detail: 1, runs: 1 }, "overlapping polls must share one refresh");
  pendingTasks.resolve([{ id: "automation-b", name: "B" }]);
  pendingDetail.resolve({ id: "automation-b", name: "B" });
  pendingRuns.resolve([{ id: "run-b", automationId: "automation-b" }]);
  await Promise.all([first, overlapping]);

  harness.handlers.list_scheduled_tasks = function () { counts.tasks += 1; return [{ id: "automation-b", name: "B2" }]; };
  harness.handlers.read_scheduled_task = function () { counts.detail += 1; return { id: "automation-b", name: "B2" }; };
  harness.handlers.list_scheduled_task_runs = function () { counts.runs += 1; return [{ id: "run-b2", automationId: "automation-b" }]; };
  await harness.bridge.scheduled.refreshScheduledTaskData(20);
  assert.deepStrictEqual(counts, { tasks: 2, detail: 2, runs: 2 }, "the next poll should run after the prior one settles");
}

async function scheduledMutationErrorBehavior() {
  const cases = [
    ["pauseScheduledTask", "pause_scheduled_task", ["automation-1"]],
    ["resumeScheduledTask", "resume_scheduled_task", ["automation-1"]],
    ["deleteScheduledTask", "delete_scheduled_task", ["automation-1"]],
    ["runScheduledTaskNow", "run_scheduled_task_now", ["automation-1"]],
    ["createScheduledTask", "create_scheduled_task", [{ name: "X", prompt: "Y", rrule: "FREQ=DAILY" }]],
  ];
  for (let i = 0; i < cases.length; i++) {
    const harness = createBridgeHarness();
    const entry = cases[i];
    harness.handlers[entry[1]] = function () { throw new Error("visible scheduled failure"); };
    await assert.rejects(function () { return harness.bridge.scheduled[entry[0]].apply(null, entry[2]); }, /visible scheduled failure/);
    const state = harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']);
    assert.ok(String(state.scheduledTaskError).includes("visible scheduled failure"), entry[0] + " should expose its error");
    assert.strictEqual(state.scheduledTaskBusyAction, null, entry[0] + " should clear busy after failure");
    harness.bridge.scheduled.dismissScheduledTaskError();
    assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskError, null, entry[0] + " errors should be dismissible");
  }

  const chatHarness = createBridgeHarness();
  chatHarness.handlers.scheduled_task_chat_prompt = function () { throw new Error("chat creation failed"); };
  await assert.rejects(function () { return chatHarness.bridge.scheduled.startScheduledTaskChat(); }, /chat creation failed/);
  assert.ok(String(chatHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskError).includes("chat creation failed"));
  assert.strictEqual(chatHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, null, "failed chat creation must not change sessions");
  assert.strictEqual(chatHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskBusyAction, null);
}

async function scheduledDeletePurgesOnlyReportedSessionBuffers() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  harness.emit("chat:delta", { session_id: "sched-delete-exact", text: "purge this exact buffer" });
  harness.emit("chat:delta", { session_id: "sched-delete-retain", text: "retain this sibling buffer" });
  harness.handlers.delete_scheduled_task = function () {
    return {
      id: "automation-delete",
      deletedSessionIds: ["sched-delete-exact"],
    };
  };
  await bridge.scheduled.deleteScheduledTask("automation-delete");

  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-delete-exact",
    automationId: "automation-delete",
    sessionId: "sched-delete-exact",
    status: "running",
  }, { id: "automation-delete", name: "Deleted task" }), true);
  assert.ok(
    !JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems).includes("purge this exact buffer"),
    "a backend-reported deleted session id must purge exactly that scheduled buffer"
  );
  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat({
    id: "run-delete-retain",
    automationId: "automation-other",
    sessionId: "sched-delete-retain",
    status: "running",
  }, { id: "automation-other", name: "Sibling task" }), true);
  assert.ok(
    JSON.stringify(bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems).includes("retain this sibling buffer"),
    "deleting one task must not guess at or purge unreported scheduled session ids"
  );

  const noIdsHarness = createBridgeHarness();
  noIdsHarness.handlers.list_scheduled_tasks = function () {
    return [{ id: "automation-no-ids", name: "No ids task" }];
  };
  noIdsHarness.handlers.list_scheduled_runs = function () {
    return [{
      id: "run-delete-no-ids",
      automationId: "automation-no-ids",
      sessionId: "sched-delete-no-ids",
      status: "completed",
      archived: false,
    }];
  };
  noIdsHarness.emit("chat:delta", {
    session_id: "sched-delete-no-ids",
    text: "retain when backend reports no ids",
  });
  noIdsHarness.handlers.delete_scheduled_task = function () {
    return { id: "automation-no-ids" };
  };
  await noIdsHarness.bridge.scheduled.loadScheduledTasks();
  await noIdsHarness.bridge.scheduled.loadScheduledTaskRecentRuns();
  await noIdsHarness.bridge.scheduled.deleteScheduledTask("automation-no-ids");
  assert.strictEqual(
    noIdsHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.length,
    0,
    "deleting a task must remove its sidebar rows even when the backend reports no session ids"
  );
  assert.strictEqual(await noIdsHarness.bridge.scheduled.openScheduledRunChat({
    id: "run-delete-no-ids",
    automationId: "automation-no-ids",
    sessionId: "sched-delete-no-ids",
    status: "running",
  }, { id: "automation-no-ids", name: "No ids task" }), true);
  assert.ok(
    JSON.stringify(noIdsHarness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems).includes("retain when backend reports no ids"),
    "a deletion response without deletedSessionIds must not trigger heuristic purging"
  );
}

async function scheduledRecentRunsIgnoreStaleAggregate() {
  const harness = createBridgeHarness();
  const staleRuns = deferred();
  harness.handlers.list_scheduled_tasks = function () {
    return [{ id: "automation-stale", name: "Stale task" }];
  };
  harness.handlers.list_scheduled_runs = function () { return staleRuns.promise; };
  harness.handlers.delete_scheduled_task = function () {
    return { id: "automation-stale", deletedSessionIds: ["sched-stale"] };
  };
  await harness.bridge.scheduled.loadScheduledTasks();
  const loading = harness.bridge.scheduled.loadScheduledTaskRecentRuns();
  await tick();
  await harness.bridge.scheduled.deleteScheduledTask("automation-stale");
  staleRuns.resolve([{
    id: "run-stale",
    automationId: "automation-stale",
    sessionId: "sched-stale",
    status: "completed",
    archived: false,
  }]);
  await loading;
  assert.strictEqual(
    harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.length,
    0,
    "an older aggregate response must not resurrect a deleted scheduled run"
  );
}

async function scheduledRunRecordSessionActionsBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "sched-run-record-actions";
  let sessionTitle = "每天给我推送时尚新闻";
  let pinned = false;
  let pinnedAt = null;
  const task = {
    id: "automation-record",
    name: "Fashion brief",
    prompt: "Run",
    rrule: "FREQ=HOURLY;INTERVAL=1",
  };
  let archivedIds = [];
  harness.handlers.list_sessions = function () { return []; };
  harness.handlers.list_archived_sessions = function () {
    return archivedIds.map(function (id) {
      return { id, title: sessionTitle, hidden_at: "2026-07-15T11:00:00Z", archived_at: "2026-07-15T11:00:00Z" };
    });
  };
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  function listRecordRuns() {
    return [{
      id: "run-record",
      automationId: task.id,
      sessionId,
      sessionTitle,
      status: "completed",
      unread: true,
      pinned,
      pinnedAt,
      archived: archivedIds.includes(sessionId),
    }];
  }
  harness.handlers.list_scheduled_task_runs = listRecordRuns;
  harness.handlers.list_scheduled_runs = listRecordRuns;
  // 定时运行会话与普通会话共用同一批 session 命令(后端按 SessionKind 分发)。
  harness.handlers.rename_session = function (args) {
    assert.strictEqual(args.id, sessionId);
    sessionTitle = args.title;
    return null;
  };
  harness.handlers.set_session_pinned = function (args) {
    assert.strictEqual(args.id, sessionId);
    pinned = !!args.pinned;
    pinnedAt = args.pinned ? "2026-07-15T10:00:00Z" : null;
    return null;
  };
  harness.handlers.set_session_archived = function (args) {
    assert.strictEqual(args.id, sessionId);
    if (args.archived) archivedIds.push(args.id);
    else archivedIds = archivedIds.filter(function (id) { return id !== args.id; });
    return null;
  };
  harness.handlers.delete_session = function (args) {
    assert.strictEqual(args.id, sessionId);
    return null;
  };

  await bridge.lifecycle.init();
  await bridge.scheduled.loadScheduledTasks();
  await bridge.scheduled.loadScheduledTaskRecentRuns();

  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns[0].sessionId, sessionId);
  await bridge.sessions.renameSession(sessionId, "重命名后的定时任务记录");
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns[0].sessionTitle, "重命名后的定时任务记录");
  assert.strictEqual(
    harness.calls.some(function (call) {
      return call.cmd === "rename_session" && call.args.id === sessionId;
    }),
    true,
    "renaming a scheduled run record should rename the backing session"
  );

  await bridge.sessions.toggleSessionPinned(sessionId, true);
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns[0].pinned, true);
  assert.strictEqual(
    harness.calls.some(function (call) {
      return call.cmd === "set_session_pinned" && call.args.id === sessionId && call.args.pinned === true;
    }),
    true,
    "pinning a scheduled run record should pin the backing session"
  );

  await bridge.sessions.archiveSession(sessionId);
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.some(function (run) { return run.sessionId === sessionId; }),
    false,
    "archiving a scheduled run record should remove it from the sidebar shortcut list"
  );
  assert.strictEqual(
    harness.calls.some(function (call) {
      return call.cmd === "set_session_archived" && call.args.id === sessionId && call.args.archived === true;
    }),
    true,
    "archiving a scheduled run record should archive the backing session"
  );
  // 归档后的运行不再回流侧边栏(archived 由后端 run DTO 携带)。
  await bridge.scheduled.loadScheduledTaskRecentRuns();
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.some(function (run) { return run.sessionId === sessionId; }),
    false,
    "archived scheduled runs must stay out of the sidebar list after a reload"
  );
  await bridge.sessions.restoreArchivedSession(sessionId);

  await bridge.scheduled.loadScheduledTaskRecentRuns();
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns[0].sessionId, sessionId);
  await bridge.sessions.deleteSession(sessionId);
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.some(function (run) { return run.sessionId === sessionId; }),
    false,
    "deleting a scheduled run record should remove it from the sidebar shortcut list"
  );
  assert.strictEqual(
    harness.calls.some(function (call) {
      return call.cmd === "delete_session" && call.args.id === sessionId;
    }),
    true,
    "deleting a scheduled run record goes through delete_session (backend dispatches by SessionKind)"
  );
  assert.strictEqual(harness.calls.some(function (call) { return call.cmd === "delete_scheduled_task"; }), false);
}

async function scheduledSessionPersistenceBehavior() {
  const harness = createBridgeHarness();
  const sessionId = "owned-run-session-1";
  await harness.bridge.scheduled.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId, status: "running" },
    { name: "Nightly report" }
  );
  await harness.bridge.scheduled.exitScheduledRunChat();
  harness.calls.length = 0;
  assert.strictEqual(await harness.bridge.sessions.switchToSession(sessionId), true);

  await harness.bridge.sessions.renameSession(sessionId, "用户重命名的定时任务记录");
  await harness.bridge.chat.cancelGeneration();
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();
  const scheduledCalls = harness.calls.filter(function (call) {
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
    scheduledCalls.some(function (call) { return call.cmd === "rename_session"; }),
    "scheduled run record titles may be user-renamed through the sidebar session action"
  );
  assert.ok(
    !scheduledCalls.some(function (call) { return call.cmd === "list_workspace_files"; }),
    "scheduled sessions must not run the ordinary frontend artifact reconciliation path"
  );
}

async function scheduledDraftModelBehavior() {
  const harness = createBridgeHarness();
  let capturedInput = null;
  let rejectCreate = true;
  harness.handlers.create_scheduled_task = function (args) {
    if (rejectCreate) throw new Error("cannot create scheduled draft");
    capturedInput = args.input;
    return Object.assign({ id: "automation-created" }, args.input);
  };
  await harness.bridge.lifecycle.init();
  await harness.bridge.scheduled.startScheduledTaskChat();
  await harness.bridge.chat.sendMessage("Create a report schedule");
  let sessionId = harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId;
  harness.emit("chat:delta", {
    session_id: sessionId,
    text: "```scheduled-task-draft\n{\"name\":\"Report\",\"prompt\":\"Run report\",\"rrule\":\"FREQ=DAILY\"}\n```",
  });
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();
  assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskDraft, null, "chat-generated parameters must not create a confirmation-card state");
  assert.ok(String(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskError).includes("cannot create scheduled draft"));
  assert.ok(
    harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).chatItems.some(function (item) {
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
  await harness.bridge.scheduled.startScheduledTaskChat();
  await harness.bridge.chat.sendMessage("Create the edited report schedule");
  sessionId = harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId;
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
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "cwds"), "the draft flow no longer sends a workspace");
  assert.strictEqual(capturedInput.mode, "yolo");
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "allowShell"), "the draft flow no longer sends permission settings");
  assert.strictEqual(capturedInput.model, "/wire-active");
  assert.strictEqual(capturedInput.modelId, "model-active");
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "sourceSessionId"));
  assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).selectedScheduledTaskId, "automation-created");
  assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskAutoOpenId, "automation-created");
}

async function completedRunReopenPreservesStreamingFollowup() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let scheduledSessionLoads = 0;
  const run = {
    id: "run-streaming-followup",
    automationId: "automation-streaming-followup",
    sessionId: "sched-streaming-followup",
    status: "completed",
    unread: false,
  };
  harness.handlers.load_session = function (args) {
    if (args.id === run.sessionId) scheduledSessionLoads += 1;
    const messages = [
      { role: "user", content: [{ type: "text", text: "durable scheduled prompt" }] },
      { role: "assistant", content: [{ type: "text", text: "durable scheduled answer" }] },
    ];
    if (args.id === run.sessionId && scheduledSessionLoads > 1) {
      messages.push({
        role: "user",
        content: [
          { type: "text", text: "<system-reminder>internal scheduled context</system-reminder>\ncontinue this completed run" },
          { type: "text", text: "<turn_meta>persisted metadata</turn_meta>" },
        ],
      });
    }
    return {
      metadata: { id: args.id, title: "Completed scheduled run" },
      messages,
      artifacts: [],
    };
  };

  await bridge.sessions.switchToSession("chat-origin");
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(run, {
    id: run.automationId,
    name: "Streaming follow-up task",
  }), true);
  await bridge.chat.sendMessage("continue this completed run");
  harness.emit("chat:delta", {
    session_id: run.sessionId,
    text: "partial follow-up output",
  });
  assert.strictEqual(bridge.state.getMany(['sessions', 'chat', 'scheduled']).busy, true);

  assert.strictEqual(await bridge.scheduled.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.scheduled.openScheduledRunChat(run, {
    id: run.automationId,
    name: "Streaming follow-up task",
  }), true);
  const reopened = bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(
    scheduledSessionLoads,
    1,
    "reopening an active follow-up should reuse its hydrated live buffer without loading disk again"
  );
  assert.strictEqual(reopened.busy, true, "reopening must preserve the live follow-up busy state");
  assert.ok(
    JSON.stringify(reopened.chatItems).includes("partial follow-up output"),
    "reopening must preserve partial output instead of replacing the live buffer"
  );
  assert.ok(
    JSON.stringify(reopened.chatItems).includes("durable scheduled prompt"),
    "reopening should still hydrate the durable transcript around the live buffer"
  );
  assert.strictEqual(
    reopened.chatItems.filter(function (item) {
      return item.type === "user" && item.text === "continue this completed run";
    }).length,
    1,
    "reopening must not duplicate a persisted follow-up that is still in the live buffer"
  );
}

async function scheduledTaskWriteSanitizationBehavior() {
  const harness = createBridgeHarness();
  let createInput = null;
  let updateInput = null;
  harness.handlers.create_scheduled_task = function (args) {
    createInput = args.input;
    return Object.assign({ id: "automation-sanitized" }, args.input);
  };
  harness.handlers.update_scheduled_task = function (args) {
    updateInput = args.input;
    return Object.assign({ id: args.id }, args.input);
  };

  await harness.bridge.scheduled.createScheduledTask({
    name: "Sanitized task",
    prompt: "Run safely",
    rrule: "FREQ=DAILY",
    model: "/wire-active",
    modelId: "model-active",
    paused: false,
    mode: "plan",
    cwds: ["D:/external"],
    allowShell: false,
    trustMode: false,
    autoApprove: false,
    unexpected: "drop-me",
  });
  assert.strictEqual(JSON.stringify(createInput), JSON.stringify({
    mode: "yolo",
    name: "Sanitized task",
    prompt: "Run safely",
    rrule: "FREQ=DAILY",
    model: "/wire-active",
    modelId: "model-active",
    paused: false,
  }), "create must strip legacy permission, directory, and unknown fields");

  await harness.bridge.scheduled.updateScheduledTask("automation-sanitized", {
    prompt: "Run safely again",
    model: "/wire-active-2",
    modelId: "model-second",
    mode: "agent",
    cwds: ["D:/external-2"],
    allowShell: true,
    trustMode: true,
    autoApprove: true,
  });
  assert.strictEqual(JSON.stringify(updateInput), JSON.stringify({
    mode: "yolo",
    prompt: "Run safely again",
    model: "/wire-active-2",
    modelId: "model-second",
  }), "update must force Yolo and strip legacy permission or directory fields");
}

// 修复1:立即运行返回时 run 还没有 sessionId。bridge 只轮询该任务的运行列表,
// 匹配到 sessionId 后把记录并入侧边栏并停止轮询。
async function scheduledRunNowSidebarLinkBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const task = { id: "automation-poll", name: "Poll task" };
  let linked = false;
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.run_scheduled_task_now = function (args) {
    assert.strictEqual(args.id, task.id);
    return {
      id: "run-now-1",
      automationId: task.id,
      sessionId: null,
      status: "queued",
      scheduledFor: "2026-07-15T08:00:00Z",
      createdAt: "2026-07-15T08:00:00Z",
    };
  };
  harness.handlers.list_scheduled_task_runs = function (args) {
    assert.strictEqual(args.id, task.id, "run-now polling must only query the task that was run");
    return [{
      id: "run-now-1",
      automationId: task.id,
      sessionId: linked ? "sched-run-now-1" : null,
      status: linked ? "running" : "queued",
      scheduledFor: "2026-07-15T08:00:00Z",
      createdAt: "2026-07-15T08:00:00Z",
    }];
  };
  await bridge.scheduled.loadScheduledTasks();
  await bridge.scheduled.runScheduledTaskNow(task.id);
  await tick();
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.some(function (run) { return run && run.id === "run-now-1"; }),
    false,
    "a run without a sessionId must not enter the sidebar list"
  );
  linked = true;
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns[0] && bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns[0].sessionId,
    "sched-run-now-1",
    "once the run links its session it must appear in the sidebar list"
  );
  const pollsAfterLink = harness.calls.filter(function (call) { return call.cmd === "list_scheduled_task_runs"; }).length;
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "list_scheduled_task_runs"; }).length,
    pollsAfterLink,
    "polling must stop once the run has a sessionId"
  );
}

// 修复2:侧边栏聚合所有任务的所有现存运行(不再有 8 条总量 / 12 任务 / 每任务 3 条截断)。
async function scheduledRecentRunsShowAllBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const tasks = [];
  for (let index = 1; index <= 14; index++) tasks.push({ id: "auto-" + index, name: "任务" + index });
  harness.handlers.list_scheduled_tasks = function () {
    return tasks.map(function (task) { return Object.assign({}, task); });
  };
  harness.handlers.list_scheduled_runs = function () {
    const runs = [];
    tasks.forEach(function (task, taskIndex) {
      for (let i = 1; i <= 4; i++) {
        runs.push({
          id: task.id + "-run-" + i,
          automationId: task.id,
          sessionId: "sched-" + task.id + "-" + i,
          status: "completed",
          unread: false,
          archived: false,
          scheduledFor: "2026-07-" + String(taskIndex + 1).padStart(2, "0") + "T0" + i + ":00:00Z",
          createdAt: "2026-07-" + String(taskIndex + 1).padStart(2, "0") + "T0" + i + ":00:00Z",
        });
      }
    });
    return runs;
  };
  await bridge.scheduled.loadScheduledTasks();
  const rows = await bridge.scheduled.loadScheduledTaskRecentRuns();
  assert.strictEqual(rows.length, 14 * 4, "every existing run conversation must be listed");
  for (let check = 1; check < rows.length; check++) {
    assert.ok(
      new Date(rows[check - 1].scheduledFor).getTime() >= new Date(rows[check].scheduledFor).getTime(),
      "sidebar runs must be sorted by time, newest first"
    );
  }
  assert.ok(
    rows.some(function (run) { return run.automationId === "auto-14"; }),
    "tasks beyond the old 12-task window must be included"
  );
  assert.ok(
    rows.filter(function (run) { return run.automationId === "auto-1"; }).length === 4,
    "runs beyond the old 3-per-task window must be included"
  );
}

// 修复4:聊天/页面创建任务必须等 create_scheduled_task 返回真实 ID 才算成功,
// 且创建成功后立即重拉任务列表,旧的在途 list 响应不能覆盖新任务。
async function scheduledCreateListRefreshBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let created = null;
  let staleResolve = null;
  let listCalls = 0;
  harness.handlers.list_scheduled_tasks = function () {
    listCalls += 1;
    if (listCalls === 1) return new Promise(function (resolve) { staleResolve = resolve; });
    return created ? [Object.assign({}, created)] : [];
  };
  harness.handlers.create_scheduled_task = function (args) {
    created = Object.assign({ id: "automation-fresh" }, args.input || {});
    return Object.assign({}, created);
  };

  const stale = bridge.scheduled.loadScheduledTasks();
  const createdTask = await bridge.scheduled.createScheduledTask({
    name: "新任务",
    prompt: "run",
    rrule: "FREQ=HOURLY;INTERVAL=1",
  });
  assert.strictEqual(createdTask.id, "automation-fresh");
  assert.ok(listCalls >= 2, "creation must refresh the task list immediately");
  staleResolve([]);
  await stale;
  assert.ok(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTasks.some(function (task) { return task.id === "automation-fresh"; }),
    "a stale in-flight task list response must not clobber the newly created task"
  );

  // 后端没有返回真实 ID 时不能算创建成功。
  harness.handlers.create_scheduled_task = function () { return null; };
  let threw = false;
  try {
    await bridge.scheduled.createScheduledTask({ name: "坏任务", prompt: "run", rrule: "FREQ=HOURLY;INTERVAL=1" });
  } catch (error) {
    threw = true;
    assert.ok(String(error && error.message || error).includes("任务 ID"));
  }
  assert.strictEqual(threw, true, "a create response without a real id must be treated as a failure");
  assert.ok(
    !bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTasks.some(function (task) { return task.id === undefined || task.id === null; }),
    "failed creations must not leave phantom tasks in the list"
  );
}

// 删除/收纳正在查看的那次定时运行,必须退出该会话视图。
// main.jsx 只按 scheduledRunContext 的真值决定渲染 ChatView 还是 ScheduledTasksView,
// 而 ChatView 内部还要求 sessionId===activeSessionId 才渲染返回按钮 —— 只清
// activeSessionId 会卡在「定时路由下的空白页且没有返回按钮」。
async function scheduledRunViewExitBehavior() {
  const task = { id: "automation-exit", name: "Exit task" };
  const run = {
    id: "run-exit",
    automationId: task.id,
    sessionId: "sched-exit-1",
    sessionTitle: "要被处理掉的运行",
    status: "completed",
    unread: false,
    archived: false,
  };

  async function openedHarness() {
    const harness = createBridgeHarness();
    harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
    harness.handlers.list_scheduled_task_runs = function () { return [Object.assign({}, run)]; };
    harness.handlers.list_scheduled_runs = function () { return [Object.assign({}, run)]; };
    harness.handlers.list_sessions = function () { return []; };
    await harness.bridge.scheduled.loadScheduledTasks();
    await harness.bridge.scheduled.loadScheduledTaskRecentRuns();
    assert.strictEqual(await harness.bridge.scheduled.openScheduledRunChat(run, task), true);
    assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledRunContext.sessionId, run.sessionId);
    assert.strictEqual(harness.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, run.sessionId);
    return harness;
  }

  const deleting = await openedHarness();
  await deleting.bridge.sessions.deleteSession(run.sessionId);
  assert.strictEqual(
    deleting.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledRunContext,
    null,
    "删除正在查看的定时运行后必须清掉 scheduledRunContext,否则界面回不到定时任务列表"
  );
  assert.strictEqual(deleting.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, null);

  const archiving = await openedHarness();
  await archiving.bridge.sessions.archiveSession(run.sessionId);
  assert.strictEqual(
    archiving.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledRunContext,
    null,
    "收纳正在查看的定时运行后同样必须退出视图(与普通对话收纳一致)"
  );
  assert.strictEqual(archiving.bridge.state.getMany(['sessions', 'chat', 'scheduled']).activeSessionId, null);
  assert.strictEqual(
    archiving.bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.some(function (item) {
      return item && item.sessionId === run.sessionId;
    }),
    false,
    "收纳后记录应离开侧边栏"
  );

  // 收纳失败要把视图和侧边栏一起回滚,不能留下「active 有值但 context 空」的错位态。
  const failing = await openedHarness();
  failing.handlers.set_session_archived = function () { throw new Error("archive failed"); };
  await failing.bridge.sessions.archiveSession(run.sessionId);
  const rolledBack = failing.bridge.state.getMany(['sessions', 'chat', 'scheduled']);
  assert.strictEqual(rolledBack.activeSessionId, run.sessionId, "收纳失败必须回到原会话");
  assert.ok(rolledBack.scheduledRunContext, "收纳失败必须恢复定时运行上下文");
  assert.strictEqual(rolledBack.scheduledRunContext.sessionId, run.sessionId);
  assert.strictEqual(
    rolledBack.scheduledTaskRecentRuns.some(function (item) {
      return item && item.sessionId === run.sessionId;
    }),
    true,
    "收纳失败必须把记录放回侧边栏"
  );
}

// 立即运行后的轮询按 run 自身状态收工,不用固定次数:worker_count=1 时排队几分钟是常态。
async function scheduledRunNowPollStopsOnTerminalBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const task = { id: "automation-terminal", name: "Terminal task" };
  let status = "queued";
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.run_scheduled_task_now = function () {
    return { id: "run-terminal", automationId: task.id, sessionId: null, status: "queued" };
  };
  harness.handlers.list_scheduled_task_runs = function () {
    // 会话始终没建起来(例如 create_session 失败),run 最终失败收场。
    return [{ id: "run-terminal", automationId: task.id, sessionId: null, status }];
  };
  await bridge.scheduled.loadScheduledTasks();
  await bridge.scheduled.runScheduledTaskNow(task.id);
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  const pollsWhileQueued = harness.calls.filter(function (c) { return c.cmd === "list_scheduled_task_runs"; }).length;
  assert.ok(pollsWhileQueued >= 2, "queued 且无会话时应继续轮询");

  status = "failed";
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  const pollsAtTerminal = harness.calls.filter(function (c) { return c.cmd === "list_scheduled_task_runs"; }).length;
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  assert.strictEqual(
    harness.calls.filter(function (c) { return c.cmd === "list_scheduled_task_runs"; }).length,
    pollsAtTerminal,
    "run 进入终态且仍无会话时必须停止轮询(再等也不会有会话)"
  );
  assert.strictEqual(
    bridge.state.getMany(['sessions', 'chat', 'scheduled']).scheduledTaskRecentRuns.some(function (item) { return item && item.id === "run-terminal"; }),
    false,
    "没有会话的运行不进侧边栏"
  );
}

async function presentationReconciliationUsesStableEventIdentity() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const sessionId = "chat-presentation-identity";
  let durable = {
    metadata: { id: sessionId, title: "Presentation identity", message_count: 0 },
    messages: [], artifacts: [], transcript_revision: "rev-0",
  };
  harness.handlers.load_session = function () { return durable; };
  harness.handlers.chat = function () { return true; };

  await bridge.sessions.switchToSession(sessionId);
  await bridge.chat.sendMessage("写一个贪吃蛇游戏");

  const writePayload = {
    session_id: sessionId,
    id: "tool-write-snake",
    name: "File",
    args: { action: "write", path: "snake-game.html", content: "<!doctype html>" },
  };
  harness.emit("chat:tool_start", writePayload);
  harness.emit("chat:tool_start", writePayload);
  assert.strictEqual(
    bridge.state.get('chat').chatItems.filter(function (item) {
      return item.type === "tool" && item.toolId === writePayload.id;
    }).length,
    1,
    "a replayed tool_start must not create a second tool card"
  );

  const absoluteArtifactPath = "C:\\Users\\tester\\.pinvou3\\sessions\\" + sessionId +
    "\\workspace\\snake-game.html";
  harness.emit("artifact:disk", {
    session_id: sessionId,
    path: absoluteArtifactPath,
    event: "created",
  });
  harness.emit("chat:tool_end", {
    session_id: sessionId,
    id: writePayload.id,
    success: true,
    output: "Wrote snake-game.html",
  });
  harness.emit("chat:tool_end", {
    session_id: sessionId,
    id: writePayload.id,
    success: true,
    output: "Wrote snake-game.html",
  });
  assert.strictEqual(
    bridge.state.get('chat').messages.filter(function (message) {
      return (message.content || []).some(function (block) {
        return block.type === "tool_result" && block.tool_use_id === writePayload.id;
      });
    }).length,
    1,
    "a replayed tool_end must not persist a second tool result"
  );

  harness.emit("chat:delta", { session_id: sessionId, text: "游戏已经写好。" });
  durable = {
    metadata: { id: sessionId, title: "Presentation identity", message_count: 4 },
    messages: [
      { role: "user", content: [{ type: "text", text: "写一个贪吃蛇游戏" }] },
      { role: "assistant", content: [{
        type: "tool_use", id: writePayload.id, name: "File", input: writePayload.args,
      }] },
      { role: "user", content: [{
        type: "tool_result", tool_use_id: writePayload.id, content: "Wrote snake-game.html",
      }] },
      { role: "assistant", content: [{ type: "text", text: "游戏已经写好。" }] },
    ],
    artifacts: ["snake-game.html"],
    transcript_revision: "rev-1",
  };
  harness.emit("chat:done", { session_id: sessionId });
  await tick();
  await tick();

  const reconciled = bridge.state.get('chat');
  const artifactCards = reconciled.chatItems.filter(function (item) {
    return item.type === "artifact_card" && /snake-game\.html$/.test(item.path || "");
  });
  assert.strictEqual(artifactCards.length, 1,
    "relative durable and absolute live artifact paths must reconcile to one card");
  assert.strictEqual(artifactCards[0].path, absoluteArtifactPath,
    "the reconciled artifact card should keep the absolute openable path");
  assert.strictEqual(
    reconciled.artifacts.filter(function (item) { return item.basename === "snake-game.html"; }).length,
    1,
    "the artifact panel must use the same semantic identity as chat cards"
  );

  function presentArtifact(id, artifactPath, title) {
    const payload = {
      session_id: sessionId,
      id,
      name: "present_artifact",
      args: { path: artifactPath, title, description: title + " description" },
    };
    harness.emit("chat:tool_start", payload);
    harness.emit("chat:tool_end", {
      session_id: sessionId,
      id,
      success: true,
      output: JSON.stringify({ ok: true, abs_path: artifactPath }),
    });
  }

  const reportPath = "C:\\Users\\tester\\.pinvou3\\sessions\\" + sessionId +
    "\\workspace\\report.md";
  const summaryPath = "C:\\Users\\tester\\.pinvou3\\sessions\\" + sessionId +
    "\\workspace\\summary.pptx";
  presentArtifact("tool-present-report", reportPath, "Report");
  presentArtifact("tool-present-summary", summaryPath, "Summary");

  const threeCardState = bridge.state.get("chat");
  const threeCards = threeCardState.chatItems.filter(function (item) {
    return item.type === "artifact_card";
  });
  assert.strictEqual(threeCards.length, 3, "the setup must contain exactly three artifact cards");
  const originalSnakeCard = threeCards.find(function (item) {
    return /snake-game\.html$/.test(item.path || "");
  });
  const originalSnakeIndex = threeCardState.chatItems.findIndex(function (item) {
    return item.id === originalSnakeCard.id;
  });

  const editPayload = {
    session_id: sessionId,
    id: "tool-edit-snake",
    name: "File",
    args: { action: "edit", path: "snake-game.html", old_string: "v1", new_string: "v2" },
  };
  harness.emit("chat:tool_start", editPayload);
  harness.emit("chat:tool_end", {
    session_id: sessionId,
    id: editPayload.id,
    success: true,
    output: "Updated snake-game.html",
  });
  presentArtifact("tool-present-snake-update", absoluteArtifactPath, "Snake game v2");

  const updatedCardState = bridge.state.get("chat");
  const updatedCards = updatedCardState.chatItems.filter(function (item) {
    return item.type === "artifact_card";
  });
  const updatedSnakeCards = updatedCards.filter(function (item) {
    return /snake-game\.html$/.test(item.path || "");
  });
  assert.strictEqual(updatedCards.length, 3,
    "updating one of three artifact cards must not create a fourth card");
  assert.strictEqual(updatedSnakeCards.length, 1,
    "the updated artifact must still have exactly one card");
  assert.strictEqual(updatedSnakeCards[0].id, originalSnakeCard.id,
    "the updated artifact must preserve its card id");
  assert.strictEqual(updatedCardState.chatItems.findIndex(function (item) {
    return item.id === originalSnakeCard.id;
  }), originalSnakeIndex, "the updated artifact must preserve its card position");
  assert.strictEqual(updatedSnakeCards[0].title, "Snake game v2",
    "the existing card must receive the latest presentation metadata");

  const questions = [{ id: "choice", header: "选择", question: "继续吗？", options: [] }];
  harness.emit("chat:user_input_required", {
    session_id: sessionId, id: "tool-question", questions,
  });
  harness.emit("chat:user_input_required", {
    session_id: sessionId, id: "tool-question", questions,
  });
  assert.strictEqual(
    bridge.state.get('chat').chatItems.filter(function (item) {
      return item.type === "user_input" && item.toolCallId === "tool-question";
    }).length,
    1,
    "a replayed user-input event must not create a second question card"
  );

  const planPayload = {
    session_id: sessionId,
    plan_id: "plan-stable-1",
    plan_snapshot: [{ step: "实现游戏", status: "pending" }],
  };
  harness.emit("chat:plan_ready", planPayload);
  harness.emit("chat:plan_ready", planPayload);
  assert.strictEqual(
    bridge.state.get('chat').chatItems.filter(function (item) {
      return item.type === "plan_card" && item.planId === planPayload.plan_id;
    }).length,
    1,
    "a replayed plan_ready event must not create a second plan card"
  );
}

async function remoteSessionDeletionConvergesPresentationState() {
  const harness = createBridgeHarness();
  const deletedId = "chat-deleted-remotely";
  const retainedId = "chat-retained-locally";
  harness.handlers.list_sessions = function () {
    return [
      { id: deletedId, title: "Delete me" },
      { id: retainedId, title: "Keep me" },
    ];
  };
  harness.handlers.list_archived_sessions = function () {
    return [{ id: deletedId, title: "Archived duplicate" }];
  };

  await harness.bridge.lifecycle.init();
  assert.strictEqual(await harness.bridge.sessions.switchToSession(deletedId), true);
  await harness.emit("session:deleted", { id: deletedId });

  const state = harness.bridge.state.get('sessions');
  assert.strictEqual(state.activeSessionId, null,
    "a remotely deleted active session must return the other client to draft state");
  assert.deepStrictEqual(Array.from(state.sessions, function (session) { return session.id; }), [retainedId]);
  assert.strictEqual(state.archivedSessions.some(function (session) { return session.id === deletedId; }), false);
}

async function multipleKnowledgeMountBehavior() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  assert.strictEqual(await bridge.sessions.switchToSession("chat-multi-kb"), true);
  let serverMounted = [];
  let serverRemoteMounted = [];
  let revision = 0;
  function snapshot() {
    revision += 1;
    return { revision, collections: serverMounted };
  }
  harness.handlers.session_add_mounted_collection = function (args) {
    const existing = serverMounted.find(function (entry) { return entry.collectionId === args.collectionId; });
    if (existing) existing.enabled = true;
    else serverMounted.push({ collectionId: args.collectionId, enabled: true });
    return snapshot();
  };
  harness.handlers.session_set_mounted_collection_enabled = function (args) {
    const existing = serverMounted.find(function (entry) { return entry.collectionId === args.collectionId; });
    if (existing) existing.enabled = !!args.enabled;
    return snapshot();
  };
  harness.handlers.session_remove_mounted_collection = function (args) {
    serverMounted = serverMounted.filter(function (entry) { return entry.collectionId !== args.collectionId; });
    return snapshot();
  };
  harness.handlers.session_unmount_collection = function () { serverMounted = []; return snapshot(); };
  harness.handlers.session_add_mounted_remote_collection = function (args) {
    const existing = serverRemoteMounted.find(function (entry) {
      return entry.serverId === args.serverId && entry.collectionId === args.collectionId;
    });
    if (existing) existing.enabled = true;
    else serverRemoteMounted.push({ serverId: args.serverId, collectionId: args.collectionId, enabled: true });
    return serverRemoteMounted;
  };
  harness.handlers.session_remove_mounted_remote_collection = function (args) {
    serverRemoteMounted = serverRemoteMounted.filter(function (entry) {
      return entry.serverId !== args.serverId || entry.collectionId !== args.collectionId;
    });
    return serverRemoteMounted;
  };

  await bridge.knowledge.mountCollection(7);
  await bridge.knowledge.mountCollection(8);
  let mounted = bridge.state.get('knowledge');
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(mounted.mountedCollections)),
    [
      { collectionId: 7, enabled: true },
      { collectionId: 8, enabled: true },
    ],
    "adding a knowledge base must preserve existing mounts"
  );
  assert.strictEqual(mounted.mountedCollection, 7, "legacy field keeps the first enabled mount");

  await bridge.knowledge.setCollectionEnabled(7, false);
  mounted = bridge.state.get('knowledge');
  assert.strictEqual(mounted.mountedCollections[0].enabled, false);
  assert.strictEqual(mounted.mountedCollection, 8, "disabled mounts must not remain active for retrieval");

  await bridge.knowledge.removeCollection(8);
  mounted = bridge.state.get('knowledge');
  assert.strictEqual(mounted.mountedCollections.length, 1);
  assert.strictEqual(mounted.mountedCollections[0].collectionId, 7);
  assert.strictEqual(mounted.mountedCollection, null, "a disabled-only mount list has no legacy active id");

  await bridge.knowledge.mountCollection(7);
  await bridge.knowledge.mountRemoteCollection("cube", 101);
  mounted = bridge.state.get('knowledge');
  assert.strictEqual(mounted.mountedCollections[0].enabled, true, "mounting again re-enables in place");
  assert.strictEqual(mounted.mountedCollection, 7);
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(mounted.mountedRemoteCollections)),
    [{ serverId: "cube", collectionId: 101, enabled: true }],
    "remote and local mounts must coexist before clearing all"
  );

  await bridge.knowledge.unmountCollection();
  mounted = bridge.state.get('knowledge');
  assert.deepStrictEqual(JSON.parse(JSON.stringify(mounted.mountedCollections)), []);
  assert.deepStrictEqual(JSON.parse(JSON.stringify(mounted.mountedRemoteCollections)), [],
    "clearing all mounts must also remove remote collections");
  assert.deepStrictEqual(serverRemoteMounted, [], "the remote mount fact source must be cleared");
  const remoteRemove = harness.calls.find(function (call) {
    return call.cmd === "session_remove_mounted_remote_collection";
  });
  assert.strictEqual(remoteRemove.args.sessionId, "chat-multi-kb",
    "remove-all must keep remote cleanup bound to the clicked session");
  assert.strictEqual(mounted.mountedCollection, null, "clearing all mounts updates both state shapes");
}

async function queuedKnowledgeMountKeepsOriginalSession() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  assert.strictEqual(await bridge.sessions.switchToSession("chat-kb-origin"), true);
  const pending = deferred();
  harness.handlers.session_add_mounted_collection = function () { return pending.promise; };

  const mount = bridge.knowledge.mountCollection(7);
  await tick();
  assert.strictEqual(await bridge.sessions.switchToSession("chat-kb-other"), true);
  pending.resolve({ revision: 1, collections: [{ collectionId: 7, enabled: true }] });
  await mount;

  const addCall = harness.calls.find(function (call) { return call.cmd === "session_add_mounted_collection"; });
  assert.strictEqual(addCall.args.sessionId, "chat-kb-origin",
    "a queued mutation must stay bound to the session active when the user clicked");
  const mounted = bridge.state.get("knowledge");
  assert.deepStrictEqual(JSON.parse(JSON.stringify(mounted.mountedCollections)), [],
    "a late response for another session must not overwrite the active session view");
}

async function staleKnowledgeSnapshotDoesNotCrossSessions() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const staleSnapshot = deferred();
  const staleReadStarted = deferred();
  harness.handlers.session_mounted_collections_snapshot = function (args) {
    if (args.sessionId === "chat-kb-stale") {
      staleReadStarted.resolve();
      return staleSnapshot.promise;
    }
    return { revision: 1, collections: [{ collectionId: 8, enabled: true }] };
  };

  const staleSwitch = bridge.sessions.switchToSession("chat-kb-stale");
  await staleReadStarted.promise;
  assert.strictEqual(await bridge.sessions.switchToSession("chat-kb-current"), true);
  staleSnapshot.resolve({ revision: 9, collections: [{ collectionId: 7, enabled: true }] });
  assert.strictEqual(await staleSwitch, false);

  const mounted = bridge.state.get("knowledge");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(mounted.mountedCollections)),
    [{ collectionId: 8, enabled: true }],
    "a late snapshot from the previous session must not overwrite the current session"
  );
  assert.strictEqual(mounted.mountedCollectionsRevision, 1);
}

async function remoteKnowledgeMountSnapshotDeduplicatesCollections() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  assert.strictEqual(await bridge.sessions.switchToSession("chat-kb-remote"), true);
  harness.handlers.session_mounted_collections_snapshot = function () {
    return {
      revision: 2,
      collections: [
        { collectionId: 7, enabled: false },
        { collection_id: 7, enabled: true },
        { collection_id: 8, enabled: true },
      ],
    };
  };

  await harness.emit("remote_control:kb_mount_changed", {
    session_id: "chat-kb-remote",
    revision: 2,
    collections: [
      { collection_id: 7, enabled: false },
      { collection_id: 7, enabled: true },
      { collection_id: 8, enabled: true },
    ],
  });
  await tick();
  await tick();

  const mounted = bridge.state.get("knowledge");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(mounted.mountedCollections)),
    [
      { collectionId: 7, enabled: false },
      { collectionId: 8, enabled: true },
    ],
    "remote mount snapshots must use the same first-entry-wins normalization as local mutations"
  );
  assert.strictEqual(mounted.mountedCollection, 8);
  assert.strictEqual(mounted.mountedCollectionsRevision, 2);
}

async function remoteKnowledgeDeletionEventRefreshesRemoteMounts() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  let serverRemoteMounted = [
    { serverId: "cube", collectionId: 7, enabled: true },
    { serverId: "cube", collectionId: 8, enabled: true },
    { serverId: "other", collectionId: 7, enabled: false },
  ];
  harness.handlers.session_mounted_remote_collections = function () {
    return serverRemoteMounted;
  };
  assert.strictEqual(await bridge.sessions.switchToSession("chat-kb-remote-delete"), true);
  const readsBeforePayload = harness.calls.filter(function (call) {
    return call.cmd === "session_mounted_remote_collections";
  }).length;

  await harness.emit("remote_control:kb_mount_changed", {
    session_id: "chat-kb-remote-delete",
    revision: 0,
    collections: [],
    remote_collections: [
      { serverId: "cube", collectionId: 8, enabled: true },
      { serverId: "other", collectionId: 7, enabled: false },
    ],
  });

  let mounted = bridge.state.get("knowledge");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(mounted.mountedRemoteCollections)),
    [
      { serverId: "cube", collectionId: 8, enabled: true },
      { serverId: "other", collectionId: 7, enabled: false },
    ],
    "the post-delete event must immediately remove only the exact server and collection mount"
  );
  assert.strictEqual(
    harness.calls.filter(function (call) {
      return call.cmd === "session_mounted_remote_collections";
    }).length,
    readsBeforePayload,
    "an authoritative event payload must take priority over an extra IPC read"
  );

  serverRemoteMounted = [{ serverId: "other", collectionId: 7, enabled: false }];
  await harness.emit("remote_control:kb_mount_changed", {
    session_id: "chat-kb-remote-delete",
    revision: 0,
    collections: [],
  });
  await tick();
  await tick();
  mounted = bridge.state.get("knowledge");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(mounted.mountedRemoteCollections)),
    [{ serverId: "other", collectionId: 7, enabled: false }],
    "events from older producers must re-read the remote mount fact source"
  );
}

async function draftKnowledgeMountsCreateOneSession() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const create = deferred();
  const serverMounted = [];
  let revision = 0;
  harness.handlers.create_session = function () { return create.promise; };
  harness.handlers.session_add_mounted_collection = function (args) {
    serverMounted.push({ collectionId: args.collectionId, enabled: true });
    revision += 1;
    return { revision, collections: serverMounted };
  };

  const first = bridge.knowledge.mountCollection(7);
  const second = bridge.knowledge.mountCollection(8);
  await tick();
  assert.strictEqual(harness.calls.filter(function (call) { return call.cmd === "create_session"; }).length, 1,
    "rapid draft mounts must share one serialized session creation");
  create.resolve({ id: "chat-kb-created" });
  await Promise.all([first, second]);

  const addCalls = harness.calls.filter(function (call) { return call.cmd === "session_add_mounted_collection"; });
  assert.strictEqual(addCalls.length, 2);
  assert.ok(addCalls.every(function (call) { return call.args.sessionId === "chat-kb-created"; }),
    "all draft mounts must target the one materialized session");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(bridge.state.get("knowledge").mountedCollections)),
    [{ collectionId: 7, enabled: true }, { collectionId: 8, enabled: true }]
  );
}

async function draftKnowledgeQueueStaysOnMaterializedSessionAfterSwitch() {
  const harness = createBridgeHarness();
  const bridge = harness.bridge;
  const firstMutation = deferred();
  const firstMutationStarted = deferred();
  let mutationCount = 0;
  harness.handlers.session_add_mounted_collection = function (args) {
    mutationCount += 1;
    if (mutationCount === 1) {
      firstMutationStarted.resolve();
      return firstMutation.promise;
    }
    return {
      revision: 2,
      collections: [
        { collectionId: 7, enabled: true },
        { collectionId: args.collectionId, enabled: true },
      ],
    };
  };

  const first = bridge.knowledge.mountCollection(7);
  const second = bridge.knowledge.mountCollection(8);
  await firstMutationStarted.promise;
  const materializedSessionId = harness.calls.find(function (call) {
    return call.cmd === "session_add_mounted_collection";
  }).args.sessionId;
  assert.strictEqual(await bridge.sessions.switchToSession("chat-kb-navigated"), true);
  firstMutation.resolve({ revision: 1, collections: [{ collectionId: 7, enabled: true }] });
  await Promise.all([first, second]);

  const addCalls = harness.calls.filter(function (call) { return call.cmd === "session_add_mounted_collection"; });
  assert.strictEqual(addCalls.length, 2);
  assert.ok(addCalls.every(function (call) { return call.args.sessionId === materializedSessionId; }),
    "queued draft mounts must remain on their shared materialized session after navigation");
  assert.deepStrictEqual(
    JSON.parse(JSON.stringify(bridge.state.get("knowledge").mountedCollections)),
    [],
    "background draft mutations must not leak into the newly active session"
  );
}

Promise.resolve()
  .then(subscriptionSnapshotsAvoidTranscriptDeepClone)
  .then(reentrantSubscriptionNotificationsStayOrdered)
  .then(persistentSubscriptionSnapshotsPreserveJsonEdges)
  .then(persistentSubscriptionSnapshotsRejectUnsupportedValues)
  .then(longSessionStreamingAvoidsPerDeltaDeepClone)
  .then(multipleKnowledgeMountBehavior)
  .then(queuedKnowledgeMountKeepsOriginalSession)
  .then(staleKnowledgeSnapshotDoesNotCrossSessions)
  .then(remoteKnowledgeMountSnapshotDeduplicatesCollections)
  .then(remoteKnowledgeDeletionEventRefreshesRemoteMounts)
  .then(draftKnowledgeMountsCreateOneSession)
  .then(draftKnowledgeQueueStaysOnMaterializedSessionAfterSwitch)
  .then(interruptedWebSessionDownloadReleasesItsLease)
  .then(lostFirstWebSessionChunkStillHasCancellableLease)
  .then(mismatchedEchoedDownloadIdCancelsBothLeases)
  .then(olderDesktopUsesServerGeneratedSessionDownloadId)
  .then(sessionDownloadWaitsForCapabilitySnapshot)
  .then(sessionDownloadCapabilityWaitStopsOnDisconnect)
  .then(sessionDownloadCapabilityWaitRejectsAlreadyOfflineDesktop)
  .then(sessionDownloadCapabilityWaitClosesLostDisconnectEventRace)
  .then(sessionDownloadCapabilityWaitHasTimeout)
  .then(sessionDownloadCapabilityGraceExpiryFailsOnce)
  .then(sessionDownloadCapabilityGraceSupersededByDraftFailsSilently)
  .then(deepSeekTurnTimelineLifecycleBehavior)
  .then(function () { return internalSubagentHandoffStaysOutOfPresentation("tauri"); })
  .then(function () { return internalSubagentHandoffStaysOutOfPresentation("web"); })
  .then(currentInternalProvenanceAndEnvelopeStayOutOfPresentation)
  .then(autoTitleSkipsInternalAndStripsTurnMeta)
  .then(webLiveEnvelopeStaysOutOfPresentation)
  .then(multiAgentToggleFailureIsRoutedToTriggerSession)
  .then(draftToggleFailureAbortsFirstSend)
  .then(scheduledRunViewExitBehavior)
  .then(scheduledRunNowPollStopsOnTerminalBehavior)
  .then(scheduledRunNowSidebarLinkBehavior)
  .then(scheduledRecentRunsShowAllBehavior)
  .then(scheduledCreateListRefreshBehavior)
  .then(scheduledRunNavigationBehavior)
  .then(scheduledRunUnreadBehavior)
  .then(openingRunningMarksBusyBeforeHydration)
  .then(followupQueuedUntilScheduledInitialTurnTerminal)
  .then(webFollowupQueuedUntilScheduledInitialTurnTerminal)
  .then(terminalEventWinsStaleRunningOpen)
  .then(completedRunReopenPreservesStreamingFollowup)
  .then(scheduledDoneBeforeBufferCreatesTerminalTombstone)
  .then(authoritativeTurnSyncDoesNotCrossSessions)
  .then(authoritativeHydrateDropsReplayedAssistantTail)
  .then(interruptedTurnRetainsDisplayOnlyPartial)
  .then(remoteInterruptedTurnKeepsItsDisplayPosition)
  .then(interruptedTurnWithoutUserItemDropsPartial)
  .then(localCompletedTurnNeverBlocksTheNextMessage)
  .then(completedTurnWaitsForAssistantInAuthoritySnapshot)
  .then(completedTurnUsesCommittedRevisionAsAuthority)
  .then(completedTurnKeepsWarningWhenRevisionMismatches)
  .then(completedTurnAdoptsLateCommittedRevision)
  .then(completedTurnFallsBackWhenSnapshotLacksRevision)
  .then(completedTurnAdoptsRevisionBumpDuringRetry)
  .then(editLastTurnBlockedWhileAuthorityReconcilePending)
  .then(function () { return editLastTurnCutsAtRealUserMessageBeforeToolResult("tauri"); })
  .then(function () { return editLastTurnCutsAtRealUserMessageBeforeToolResult("web"); })
  .then(function () { return rejectedEditRestoresAuthoritativeTranscript("tauri"); })
  .then(function () { return rejectedEditRestoresAuthoritativeTranscript("web"); })
  .then(remoteAcceptPlanConvergesAcrossClients)
  .then(activePlanSurvivesUnrelatedTerminalHydrate)
  .then(activePlanHydrateMigratesTicketWithoutDuplicate)
  .then(planNotActiveRollbackFreezesStaleCard)
  .then(planTicketCommandsAndRemoteDiscardConverge)
  .then(discardPlanFailureRollbackFollowsTicketAuthority)
  .then(failedRunningOpenRollsBackOnlyItsProvisionalBusy)
  .then(concurrentFailedRunningOpensShareRollback)
  .then(scheduledOwnerRegistryIsBoundedAndProtectsLive)
  .then(scheduledBufferLruNeverEvictsLive)
  .then(scheduledRunningHydrationRaceBehavior)
  .then(presentationReconciliationUsesStableEventIdentity)
  .then(scheduledUnreadPollingRaceBehavior)
  .then(scheduledFolderPickerBehavior)
  .then(scheduledTemplateSourcePersistenceBehavior)
  .then(scheduledSelectionGenerationBehavior)
  .then(scheduledRefreshDoesNotOverlap)
  .then(scheduledMutationErrorBehavior)
  .then(scheduledDeletePurgesOnlyReportedSessionBuffers)
  .then(scheduledRecentRunsIgnoreStaleAggregate)
  .then(scheduledRunRecordSessionActionsBehavior)
  .then(scheduledSessionPersistenceBehavior)
  .then(scheduledDraftModelBehavior)
  .then(scheduledTaskWriteSanitizationBehavior)
  .then(remoteSessionDeletionConvergesPresentationState)
  .then(function () { console.log('PASS scheduled tasks unit'); })
  // eslint-disable-next-line unicorn/prefer-top-level-await -- the smoke script already has an async main() structure
  .catch(function (error) {
    console.error(error && error.stack || error);
    process.exitCode = 1;
  });
