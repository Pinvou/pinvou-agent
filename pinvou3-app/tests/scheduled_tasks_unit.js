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
  'app/main.jsx',
  'shared/i18n.js',
  'shared/model-options.js',
  'components/layout/NavigationComponents.jsx',
  'features/chat/ChatView.jsx',
  'features/scheduled/ScheduledTasksView.jsx'
].map(file => fs.readFileSync(path.join(__dirname, '..', 'src', file), 'utf8')).join('\n');
const tauriBridgeFeatureNames = [
  'artifact-tracker', 'chat', 'chat-events', 'sessions', 'terminal', 'scheduled', 'monitor', 'settings', 'memory', 'artifacts', 'personas', 'updater',
  'remote-control', 'dependencies', 'voice', 'knowledge-model', 'workflow-runtime', 'workflow'
];
const tauriBridge = tauriBridgeFeatureNames
  .map(name => fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge', `${name}.js`), 'utf8'))
  .concat(fs.readFileSync(path.join(__dirname, '..', 'src', 'platform', 'tauri', 'bridge.js'), 'utf8'))
  .join('\n');
const modelOptionsSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'shared', 'model-options.js'), 'utf8');
const settingsViewSource = fs.readFileSync(path.join(__dirname, '..', 'src', 'features', 'settings', 'SettingsView.jsx'), 'utf8');
const scheduledTasksRust = fs.readFileSync(path.join(__dirname, '..', 'src-tauri', 'src', 'features', 'scheduled', 'tasks.rs'), 'utf8');
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
    /var lockedModelId = state\.scheduledTaskDraft\.modelId \|\| \(active && active\.id\)/.test(tauriBridge),
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
    /async function init\(\)[\s\S]{0,420}loadScheduledTasks\(\)\.catch/.test(tauriBridge),
  'run updates should debounce a global task and run refresh regardless of the current page'
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
assert(
  /scheduled-task-next-run/.test(indexHtml) && /font-semibold/.test(indexHtml) && /text-\[#1769B0\]/.test(indexHtml),
  'active task rows should visually distinguish the next run from the schedule label'
);
assert.ok(
  /function scheduleRepeatLabel\(/.test(indexHtml) &&
    /editor\.interval/.test(indexHtml) &&
    /editor\.repeat === 'hourly' \? '起始时间' : '时间'/.test(indexHtml) &&
    /const hasTimeAnchor = fields\.BYHOUR != null \|\| fields\.BYMINUTE != null/.test(indexHtml) &&
    /previousEditor\.hasTimeAnchor/.test(indexHtml) &&
    /placeholder=.*设置起点/.test(indexHtml) &&
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
    /\{renderTemplateSuggestions\(\)\}[\s\S]{0,120}<MyTasksSection className="mb-0" \/>/.test(indexHtml) &&
    /const DetailTaskDialog = \(\) => !\(selected && detailForm\) \? null/.test(indexHtml) &&
    /const renderModal = node => modalPortalTarget \? createPortal\(node, modalPortalTarget\) : node/.test(indexHtml) &&
    /DetailTaskDialog = \(\) => !\(selected && detailForm\) \? null : renderModal\(/.test(indexHtml) &&
    /role="dialog"/.test(indexHtml) &&
    /data-testid="scheduled-detail-toolbar"/.test(indexHtml) &&
    /data-testid="scheduled-detail-close"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-menu"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-menu-popover"/.test(indexHtml) &&
    !/data-testid="scheduled-detail-toggle"/.test(indexHtml) &&
    /flex shrink-0 flex-wrap items-center justify-between/.test(indexHtml) &&
    /data-testid="scheduled-run-now"[\s\S]{0,520}立即运行/.test(indexHtml) &&
    /data-testid="scheduled-open-folder"[\s\S]{0,520}打开文件夹/.test(indexHtml) &&
    !/data-testid="scheduled-detail-cancel"/.test(indexHtml) &&
    /data-testid="scheduled-detail-save"[\s\S]{0,320}保存/.test(indexHtml) &&
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
  /async function openRunChat\(run\)[\s\S]*?bridge\.openScheduledRunChat\(run,\s*detail \|\| selected\)/.test(indexHtml),
  'scheduled run history rows should open the run chat session'
);
assert.ok(
  !/data-testid="scheduled-run-mode"/.test(indexHtml) &&
    !/data-testid="scheduled-live-mode"/.test(indexHtml) &&
    /function scheduledTaskBackendInput\(input\)/.test(tauriBridge) &&
    /var backendInput = \{ mode: "yolo" \}/.test(tauriBridge) &&
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
  /builtin_llmapi/.test(modelOptionsSource) &&
    /const savedModels = visibleUserModels\(appState\.savedModels \|\| \[\]\)/.test(indexHtml) &&
    /const userModels = visibleSortedModels\(savedModels \|\| \[\], bs\)/.test(settingsViewSource) &&
    /const allowBuiltin = hasLlmApiBackendUser\(bs\)/.test(settingsViewSource),
  'scheduled tasks should hide the built-in model option while chat keeps the account-gated built-in model'
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
  /editable=\{!busy && item\.id === lastUserId\}/.test(indexHtml) &&
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
    /<span[^>]*>任务名称<\/span>/.test(indexHtml) &&
    /disabled=\{!!busyAction \|\| !String\(createForm\.name/.test(indexHtml) &&
    /async function startBlankTask\(\)[\s\S]{0,1200}setCreateForm\(/.test(indexHtml) &&
    /selectAfterCreate:\s*false/.test(indexHtml) &&
    /async function submitCustomTask\(event\)[\s\S]{0,1600}bridge\.createScheduledTask\(/.test(indexHtml),
  'custom creation should collect a valid task in a dialog before creating it'
);
assert.ok(
  /var selectAfterCreate = !input \|\| input\.selectAfterCreate !== false/.test(tauriBridge) &&
    /if \(!created \|\| !created\.id\)/.test(tauriBridge) &&
    /if \(selectAfterCreate\) selectScheduledTask\(created\.id\)/.test(tauriBridge) &&
    /if \(selectAfterCreate\) state\.scheduledTaskDetail = created/.test(tauriBridge),
  'scheduled creation dialogs should be able to create without immediately opening the edit sheet'
);
assert.ok(
  /fn should_sync_session\([\s\S]{0,120}is_scheduled \|\| has_messages/.test(enginePoolRust) &&
    /should_sync_session\(is_scheduled, !saved\.messages\.is_empty\(\)\)/.test(enginePoolRust),
  'scheduled sessions must SyncSession even when their durable message list is empty'
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
mustNotContain("Templates");
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
    /async function submitCustomTask\(event\)[\s\S]{0,1800}bridge\.createScheduledTask\([\s\S]{0,420}templateId:\s*createForm\.templateId \|\| undefined[\s\S]{0,420}mode:\s*'yolo'/.test(indexHtml) &&
    !/scheduled-detail-settings[\s\S]{0,1200}>权限</.test(indexHtml),
  'selecting a template should confirm through the second-level sheet and create with fixed Yolo mode and no permission UI'
);
assert.ok(
  /const visibleSuggestions\s*=\s*SCHEDULED_TASK_TEMPLATES;/.test(indexHtml) &&
    !/const visibleSuggestions\s*=\s*SCHEDULED_TASK_TEMPLATES\.filter/.test(indexHtml) &&
    /visibleSuggestions\.map\(template/.test(indexHtml),
  'suggested templates should remain visible after users create matching scheduled tasks'
);
assert.ok(
  /scheduled-task-template-sources-v1/.test(tauriBridge) &&
    /var templateId = input && typeof input\.templateId === "string"/.test(tauriBridge) &&
    !/SCHEDULED_TASK_WRITABLE_FIELDS[^;]*templateId/.test(tauriBridge) &&
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

function createBridgeHarness(sharedStorage, runtimeOptions) {
  runtimeOptions = runtimeOptions || {};
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
        cmd === "list_workspace_files" || cmd === "list_scheduled_task_runs" ||
        cmd === "list_scheduled_runs") return [];
    if (cmd === "get_mode_state") return { mode: "yolo" };
    if (cmd === "get_memory_overview") return {};
    if (cmd === "get_llmapi_status") {
      return { backend_user_exists: false, backend_user_state: "not_exists", stale: false };
    }
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
    setTimeout: runtimeOptions.setTimeout || setTimeout,
    clearTimeout: runtimeOptions.clearTimeout || clearTimeout,
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

async function llmApiStartupRetryBehavior() {
  var timers = [];
  var harness = createBridgeHarness(null, {
    setTimeout: function (callback, delay) {
      timers.push({ callback: callback, delay: delay });
      return timers.length;
    },
    clearTimeout: function () {},
  });
  var statusCalls = 0;
  var modelCalls = 0;
  harness.handlers.get_llmapi_status = function () {
    statusCalls += 1;
    if (statusCalls === 1) {
      return { backend_user_exists: false, backend_user_state: "unknown", stale: true };
    }
    if (statusCalls === 2) throw new Error("temporary network failure");
    return { backend_user_exists: true, backend_user_state: "exists", stale: false };
  };
  harness.handlers.get_llmapi_models = function () {
    modelCalls += 1;
    return { available_models: ["deepseek-v4-flash"], default_model: "deepseek-v4-flash" };
  };

  await harness.bridge.init();
  await tick();
  assert.strictEqual(statusCalls, 1, "startup should query the built-in account immediately");
  var firstRetry = timers.find(function (timer) { return timer.delay === 2000; });
  assert.ok(firstRetry, "an unknown account result should schedule the first retry");

  firstRetry.callback();
  await tick();
  await tick();
  assert.strictEqual(statusCalls, 2, "a transport failure should keep the startup retry active");
  var secondRetry = timers.find(function (timer) { return timer.delay === 5000; });
  assert.ok(secondRetry, "startup retries should back off after another inconclusive result");

  secondRetry.callback();
  await tick();
  await tick();
  assert.strictEqual(statusCalls, 3, "startup should retry until the account result is authoritative");
  assert.strictEqual(modelCalls, 1, "a known existing account should refresh built-in models once");
  assert.strictEqual(harness.bridge.getState().llmApiStatus.backend_user_state, "exists");
  assert.strictEqual(harness.bridge.getState().llmApiModels.default_model, "deepseek-v4-flash");
  assert.strictEqual(
    timers.filter(function (timer) { return timer.delay === 10000; }).length,
    0,
    "startup retries should stop after an existing account is confirmed"
  );

  var missingTimers = [];
  var missingHarness = createBridgeHarness(null, {
    setTimeout: function (callback, delay) {
      missingTimers.push({ callback: callback, delay: delay });
      return missingTimers.length;
    },
    clearTimeout: function () {},
  });
  var missingStatusCalls = 0;
  missingHarness.handlers.get_llmapi_status = function () {
    missingStatusCalls += 1;
    return { backend_user_exists: false, backend_user_state: "not_exists", stale: false };
  };

  await missingHarness.bridge.init();
  await tick();
  await tick();
  assert.strictEqual(missingStatusCalls, 1, "a confirmed missing account should not be retried");
  assert.strictEqual(missingHarness.bridge.getState().llmApiStatus.backend_user_state, "not_exists");
  assert.strictEqual(
    missingTimers.filter(function (timer) { return timer.delay === 2000; }).length,
    0,
    "startup retries should stop after account absence is confirmed"
  );
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

  await bridge.switchToSession("chat-origin");
  await bridge.loadScheduledTasks();
  bridge.selectScheduledTask(task.id);
  await bridge.readScheduledTask(task.id);
  await bridge.loadScheduledTaskRuns(task.id, 20);
  await bridge.loadScheduledTaskRecentRuns();
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
  assert.strictEqual(afterFirst.scheduledTaskRecentRuns[0].unread, false, "the opened sidebar run should lose its dot immediately");
  assert.strictEqual(afterFirst.scheduledTaskRecentRuns[1].unread, true, "the sibling sidebar run should remain unread");
  assert.strictEqual(afterFirst.scheduledTasks[0].hasUnreadRuns, true, "task dot remains while a child run is unread");
  assert.strictEqual(afterFirst.scheduledTaskDetail.hasUnreadRuns, true);

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.openScheduledRunChat(runs[1], task), true);
  var afterSecond = bridge.getState();
  assert.ok(afterSecond.scheduledTaskRuns.every(function (run) { return run.unread === false; }));
  assert.ok(afterSecond.scheduledTaskRecentRuns.every(function (run) { return run.unread === false; }));
  assert.strictEqual(afterSecond.scheduledTasks[0].hasUnreadRuns, false, "task dot clears only after every child run was opened");
  assert.strictEqual(afterSecond.scheduledTaskDetail.hasUnreadRuns, false);

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  harness.emit("chat:delta", { session_id: "sched-running", text: "partial live output" });
  harness.handlers.load_session = function (args) {
    if (args.id === "sched-running") {
      return {
        metadata: { id: "sched-running", title: "Running scheduled conversation" },
        messages: [{ role: "user", content: [
          { type: "text", text: "<system-reminder>\ninternal policy: sudo/apt/systemctl/pkexec\n</system-reminder>\n\ndurable scheduled prompt" },
          { type: "text", text: "<turn_meta>\nCurrent workspace: C:\\\\Users\\\\demo\n</turn_meta>" },
        ] }],
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
  var visibleScheduledTranscript = JSON.stringify(bridge.getState().chatItems);
  assert.ok(!visibleScheduledTranscript.includes("system-reminder"), "scheduled bubbles must hide the internal reminder");
  assert.ok(!visibleScheduledTranscript.includes("turn_meta"), "scheduled bubbles must hide turn metadata");
  assert.ok(!visibleScheduledTranscript.includes("sudo/apt/systemctl/pkexec"), "scheduled bubbles must hide internal policy text");
  assert.ok(!visibleScheduledTranscript.includes("Current workspace"), "scheduled bubbles must hide internal workspace metadata");
  assert.ok(
    JSON.stringify(bridge.getState().messages).includes("<system-reminder>"),
    "the raw scheduled message must remain intact for model context"
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
  // createScheduledTask 成功后会立即重拉任务列表;真实后端此时必然已包含新任务。
  first.handlers.list_scheduled_tasks = function () {
    return backendInput ? [Object.assign({ id: "automation-template" }, backendInput)] : [];
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
  var aggregateRefreshes = 0;
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
  assert.strictEqual(harness.bridge.getState().scheduledTasks[0].hasUnreadRuns, true, "the unselected task unread summary should enter global state");
  assert.strictEqual(refreshes, 1, "the selected task detail should refresh once after the event burst");
  assert.strictEqual(aggregateRefreshes, 1, "the global scheduled-run sidebar should refresh once after the event burst");
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
    harness.bridge.dismissScheduledTaskError();
    assert.strictEqual(harness.bridge.getState().scheduledTaskError, null, entry[0] + " errors should be dismissible");
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
  await noIdsHarness.bridge.loadScheduledTasks();
  await noIdsHarness.bridge.loadScheduledTaskRecentRuns();
  await noIdsHarness.bridge.deleteScheduledTask("automation-no-ids");
  assert.strictEqual(
    noIdsHarness.bridge.getState().scheduledTaskRecentRuns.length,
    0,
    "deleting a task must remove its sidebar rows even when the backend reports no session ids"
  );
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

async function scheduledRecentRunsIgnoreStaleAggregate() {
  var harness = createBridgeHarness();
  var staleRuns = deferred();
  harness.handlers.list_scheduled_tasks = function () {
    return [{ id: "automation-stale", name: "Stale task" }];
  };
  harness.handlers.list_scheduled_runs = function () { return staleRuns.promise; };
  harness.handlers.delete_scheduled_task = function () {
    return { id: "automation-stale", deletedSessionIds: ["sched-stale"] };
  };
  await harness.bridge.loadScheduledTasks();
  var loading = harness.bridge.loadScheduledTaskRecentRuns();
  await tick();
  await harness.bridge.deleteScheduledTask("automation-stale");
  staleRuns.resolve([{
    id: "run-stale",
    automationId: "automation-stale",
    sessionId: "sched-stale",
    status: "completed",
    archived: false,
  }]);
  await loading;
  assert.strictEqual(
    harness.bridge.getState().scheduledTaskRecentRuns.length,
    0,
    "an older aggregate response must not resurrect a deleted scheduled run"
  );
}

async function scheduledRunRecordSessionActionsBehavior() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var sessionId = "sched-run-record-actions";
  var sessionTitle = "每天给我推送时尚新闻";
  var pinned = false;
  var pinnedAt = null;
  var task = {
    id: "automation-record",
    name: "Fashion brief",
    prompt: "Run",
    rrule: "FREQ=HOURLY;INTERVAL=1",
  };
  var archivedIds = [];
  harness.handlers.list_sessions = function () { return []; };
  harness.handlers.list_archived_sessions = function () {
    return archivedIds.map(function (id) {
      return { id: id, title: sessionTitle, hidden_at: "2026-07-15T11:00:00Z", archived_at: "2026-07-15T11:00:00Z" };
    });
  };
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  function listRecordRuns() {
    return [{
      id: "run-record",
      automationId: task.id,
      sessionId: sessionId,
      sessionTitle: sessionTitle,
      status: "completed",
      unread: true,
      pinned: pinned,
      pinnedAt: pinnedAt,
      archived: archivedIds.indexOf(sessionId) >= 0,
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

  await bridge.init();
  await bridge.loadScheduledTasks();
  await bridge.loadScheduledTaskRecentRuns();

  assert.strictEqual(bridge.getState().scheduledTaskRecentRuns[0].sessionId, sessionId);
  await bridge.renameSession(sessionId, "重命名后的定时任务记录");
  assert.strictEqual(bridge.getState().scheduledTaskRecentRuns[0].sessionTitle, "重命名后的定时任务记录");
  assert.strictEqual(
    harness.calls.some(function (call) {
      return call.cmd === "rename_session" && call.args.id === sessionId;
    }),
    true,
    "renaming a scheduled run record should rename the backing session"
  );

  await bridge.toggleSessionPinned(sessionId, true);
  assert.strictEqual(bridge.getState().scheduledTaskRecentRuns[0].pinned, true);
  assert.strictEqual(
    harness.calls.some(function (call) {
      return call.cmd === "set_session_pinned" && call.args.id === sessionId && call.args.pinned === true;
    }),
    true,
    "pinning a scheduled run record should pin the backing session"
  );

  await bridge.archiveSession(sessionId);
  assert.strictEqual(
    bridge.getState().scheduledTaskRecentRuns.some(function (run) { return run.sessionId === sessionId; }),
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
  await bridge.loadScheduledTaskRecentRuns();
  assert.strictEqual(
    bridge.getState().scheduledTaskRecentRuns.some(function (run) { return run.sessionId === sessionId; }),
    false,
    "archived scheduled runs must stay out of the sidebar list after a reload"
  );
  await bridge.restoreArchivedSession(sessionId);

  await bridge.loadScheduledTaskRecentRuns();
  assert.strictEqual(bridge.getState().scheduledTaskRecentRuns[0].sessionId, sessionId);
  await bridge.deleteSession(sessionId);
  assert.strictEqual(
    bridge.getState().scheduledTaskRecentRuns.some(function (run) { return run.sessionId === sessionId; }),
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
  var harness = createBridgeHarness();
  var sessionId = "owned-run-session-1";
  await harness.bridge.openScheduledRunChat(
    { id: "run-1", automationId: "automation-1", sessionId: sessionId, status: "running" },
    { name: "Nightly report" }
  );
  await harness.bridge.exitScheduledRunChat();
  harness.calls.length = 0;
  assert.strictEqual(await harness.bridge.switchToSession(sessionId), true);

  await harness.bridge.renameSession(sessionId, "用户重命名的定时任务记录");
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
    scheduledCalls.some(function (call) { return call.cmd === "rename_session"; }),
    "scheduled run record titles may be user-renamed through the sidebar session action"
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
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "cwds"), "the draft flow no longer sends a workspace");
  assert.strictEqual(capturedInput.mode, "yolo");
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "allowShell"), "the draft flow no longer sends permission settings");
  assert.strictEqual(capturedInput.model, "/wire-active");
  assert.strictEqual(capturedInput.modelId, "model-active");
  assert.ok(!Object.prototype.hasOwnProperty.call(capturedInput, "sourceSessionId"));
  assert.strictEqual(harness.bridge.getState().selectedScheduledTaskId, "automation-created");
  assert.strictEqual(harness.bridge.getState().scheduledTaskAutoOpenId, "automation-created");
}

async function completedRunReopenPreservesStreamingFollowup() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var scheduledSessionLoads = 0;
  var run = {
    id: "run-streaming-followup",
    automationId: "automation-streaming-followup",
    sessionId: "sched-streaming-followup",
    status: "completed",
    unread: false,
  };
  harness.handlers.load_session = function (args) {
    if (args.id === run.sessionId) scheduledSessionLoads += 1;
    var messages = [
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
      messages: messages,
      artifacts: [],
    };
  };

  await bridge.switchToSession("chat-origin");
  assert.strictEqual(await bridge.openScheduledRunChat(run, {
    id: run.automationId,
    name: "Streaming follow-up task",
  }), true);
  await bridge.sendMessage("continue this completed run");
  harness.emit("chat:delta", {
    session_id: run.sessionId,
    text: "partial follow-up output",
  });
  assert.strictEqual(bridge.getState().busy, true);

  assert.strictEqual(await bridge.exitScheduledRunChat(), true);
  assert.strictEqual(await bridge.openScheduledRunChat(run, {
    id: run.automationId,
    name: "Streaming follow-up task",
  }), true);
  var reopened = bridge.getState();
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
  var harness = createBridgeHarness();
  var createInput = null;
  var updateInput = null;
  harness.handlers.create_scheduled_task = function (args) {
    createInput = args.input;
    return Object.assign({ id: "automation-sanitized" }, args.input);
  };
  harness.handlers.update_scheduled_task = function (args) {
    updateInput = args.input;
    return Object.assign({ id: args.id }, args.input);
  };

  await harness.bridge.createScheduledTask({
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

  await harness.bridge.updateScheduledTask("automation-sanitized", {
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
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var task = { id: "automation-poll", name: "Poll task" };
  var linked = false;
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
  await bridge.loadScheduledTasks();
  await bridge.runScheduledTaskNow(task.id);
  await tick();
  assert.strictEqual(
    bridge.getState().scheduledTaskRecentRuns.some(function (run) { return run && run.id === "run-now-1"; }),
    false,
    "a run without a sessionId must not enter the sidebar list"
  );
  linked = true;
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  assert.strictEqual(
    bridge.getState().scheduledTaskRecentRuns[0] && bridge.getState().scheduledTaskRecentRuns[0].sessionId,
    "sched-run-now-1",
    "once the run links its session it must appear in the sidebar list"
  );
  var pollsAfterLink = harness.calls.filter(function (call) { return call.cmd === "list_scheduled_task_runs"; }).length;
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  assert.strictEqual(
    harness.calls.filter(function (call) { return call.cmd === "list_scheduled_task_runs"; }).length,
    pollsAfterLink,
    "polling must stop once the run has a sessionId"
  );
}

// 修复2:侧边栏聚合所有任务的所有现存运行(不再有 8 条总量 / 12 任务 / 每任务 3 条截断)。
async function scheduledRecentRunsShowAllBehavior() {
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var tasks = [];
  for (var index = 1; index <= 14; index++) tasks.push({ id: "auto-" + index, name: "任务" + index });
  harness.handlers.list_scheduled_tasks = function () {
    return tasks.map(function (task) { return Object.assign({}, task); });
  };
  harness.handlers.list_scheduled_runs = function () {
    var runs = [];
    tasks.forEach(function (task, taskIndex) {
      for (var i = 1; i <= 4; i++) {
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
  await bridge.loadScheduledTasks();
  var rows = await bridge.loadScheduledTaskRecentRuns();
  assert.strictEqual(rows.length, 14 * 4, "every existing run conversation must be listed");
  for (var check = 1; check < rows.length; check++) {
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
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var created = null;
  var staleResolve = null;
  var listCalls = 0;
  harness.handlers.list_scheduled_tasks = function () {
    listCalls += 1;
    if (listCalls === 1) return new Promise(function (resolve) { staleResolve = resolve; });
    return created ? [Object.assign({}, created)] : [];
  };
  harness.handlers.create_scheduled_task = function (args) {
    created = Object.assign({ id: "automation-fresh" }, args.input || {});
    return Object.assign({}, created);
  };

  var stale = bridge.loadScheduledTasks();
  var createdTask = await bridge.createScheduledTask({
    name: "新任务",
    prompt: "run",
    rrule: "FREQ=HOURLY;INTERVAL=1",
  });
  assert.strictEqual(createdTask.id, "automation-fresh");
  assert.ok(listCalls >= 2, "creation must refresh the task list immediately");
  staleResolve([]);
  await stale;
  assert.ok(
    bridge.getState().scheduledTasks.some(function (task) { return task.id === "automation-fresh"; }),
    "a stale in-flight task list response must not clobber the newly created task"
  );

  // 后端没有返回真实 ID 时不能算创建成功。
  harness.handlers.create_scheduled_task = function () { return null; };
  var threw = false;
  try {
    await bridge.createScheduledTask({ name: "坏任务", prompt: "run", rrule: "FREQ=HOURLY;INTERVAL=1" });
  } catch (error) {
    threw = true;
    assert.ok(String(error && error.message || error).includes("任务 ID"));
  }
  assert.strictEqual(threw, true, "a create response without a real id must be treated as a failure");
  assert.ok(
    !bridge.getState().scheduledTasks.some(function (task) { return task.id === undefined || task.id === null; }),
    "failed creations must not leave phantom tasks in the list"
  );
}

// 删除/收纳正在查看的那次定时运行,必须退出该会话视图。
// main.jsx 只按 scheduledRunContext 的真值决定渲染 ChatView 还是 ScheduledTasksView,
// 而 ChatView 内部还要求 sessionId===activeSessionId 才渲染返回按钮 —— 只清
// activeSessionId 会卡在「定时路由下的空白页且没有返回按钮」。
async function scheduledRunViewExitBehavior() {
  var task = { id: "automation-exit", name: "Exit task" };
  var run = {
    id: "run-exit",
    automationId: task.id,
    sessionId: "sched-exit-1",
    sessionTitle: "要被处理掉的运行",
    status: "completed",
    unread: false,
    archived: false,
  };

  async function openedHarness() {
    var harness = createBridgeHarness();
    harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
    harness.handlers.list_scheduled_task_runs = function () { return [Object.assign({}, run)]; };
    harness.handlers.list_scheduled_runs = function () { return [Object.assign({}, run)]; };
    harness.handlers.list_sessions = function () { return []; };
    await harness.bridge.loadScheduledTasks();
    await harness.bridge.loadScheduledTaskRecentRuns();
    assert.strictEqual(await harness.bridge.openScheduledRunChat(run, task), true);
    assert.strictEqual(harness.bridge.getState().scheduledRunContext.sessionId, run.sessionId);
    assert.strictEqual(harness.bridge.getState().activeSessionId, run.sessionId);
    return harness;
  }

  var deleting = await openedHarness();
  await deleting.bridge.deleteSession(run.sessionId);
  assert.strictEqual(
    deleting.bridge.getState().scheduledRunContext,
    null,
    "删除正在查看的定时运行后必须清掉 scheduledRunContext,否则界面回不到定时任务列表"
  );
  assert.strictEqual(deleting.bridge.getState().activeSessionId, null);

  var archiving = await openedHarness();
  await archiving.bridge.archiveSession(run.sessionId);
  assert.strictEqual(
    archiving.bridge.getState().scheduledRunContext,
    null,
    "收纳正在查看的定时运行后同样必须退出视图(与普通对话收纳一致)"
  );
  assert.strictEqual(archiving.bridge.getState().activeSessionId, null);
  assert.strictEqual(
    archiving.bridge.getState().scheduledTaskRecentRuns.some(function (item) {
      return item && item.sessionId === run.sessionId;
    }),
    false,
    "收纳后记录应离开侧边栏"
  );

  // 收纳失败要把视图和侧边栏一起回滚,不能留下「active 有值但 context 空」的错位态。
  var failing = await openedHarness();
  failing.handlers.set_session_archived = function () { throw new Error("archive failed"); };
  await failing.bridge.archiveSession(run.sessionId);
  var rolledBack = failing.bridge.getState();
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
  var harness = createBridgeHarness();
  var bridge = harness.bridge;
  var task = { id: "automation-terminal", name: "Terminal task" };
  var status = "queued";
  harness.handlers.list_scheduled_tasks = function () { return [Object.assign({}, task)]; };
  harness.handlers.run_scheduled_task_now = function () {
    return { id: "run-terminal", automationId: task.id, sessionId: null, status: "queued" };
  };
  harness.handlers.list_scheduled_task_runs = function () {
    // 会话始终没建起来(例如 create_session 失败),run 最终失败收场。
    return [{ id: "run-terminal", automationId: task.id, sessionId: null, status: status }];
  };
  await bridge.loadScheduledTasks();
  await bridge.runScheduledTaskNow(task.id);
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  var pollsWhileQueued = harness.calls.filter(function (c) { return c.cmd === "list_scheduled_task_runs"; }).length;
  assert.ok(pollsWhileQueued >= 2, "queued 且无会话时应继续轮询");

  status = "failed";
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  var pollsAtTerminal = harness.calls.filter(function (c) { return c.cmd === "list_scheduled_task_runs"; }).length;
  await new Promise(function (resolve) { setTimeout(resolve, 1300); });
  assert.strictEqual(
    harness.calls.filter(function (c) { return c.cmd === "list_scheduled_task_runs"; }).length,
    pollsAtTerminal,
    "run 进入终态且仍无会话时必须停止轮询(再等也不会有会话)"
  );
  assert.strictEqual(
    bridge.getState().scheduledTaskRecentRuns.some(function (item) { return item && item.id === "run-terminal"; }),
    false,
    "没有会话的运行不进侧边栏"
  );
}

Promise.resolve()
  .then(llmApiStartupRetryBehavior)
  .then(scheduledRunViewExitBehavior)
  .then(scheduledRunNowPollStopsOnTerminalBehavior)
  .then(scheduledRunNowSidebarLinkBehavior)
  .then(scheduledRecentRunsShowAllBehavior)
  .then(scheduledCreateListRefreshBehavior)
  .then(scheduledRunNavigationBehavior)
  .then(scheduledRunUnreadBehavior)
  .then(openingRunningMarksBusyBeforeHydration)
  .then(followupQueuedUntilScheduledInitialTurnTerminal)
  .then(terminalEventWinsStaleRunningOpen)
  .then(completedRunReopenPreservesStreamingFollowup)
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
  .then(scheduledRecentRunsIgnoreStaleAggregate)
  .then(scheduledRunRecordSessionActionsBehavior)
  .then(scheduledSessionPersistenceBehavior)
  .then(scheduledDraftModelBehavior)
  .then(scheduledTaskWriteSanitizationBehavior)
  .then(function () { console.log('PASS scheduled tasks unit'); })
  .catch(function (error) {
    console.error(error && error.stack || error);
    process.exitCode = 1;
  });
