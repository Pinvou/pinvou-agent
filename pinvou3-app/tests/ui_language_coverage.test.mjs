import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from '../src/shared/i18n.js';

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

for (const language of ['zh', 'en', 'ja']) {
  assert.ok(dict[language].expertPoolIndividualTab, `${language}.expertPoolIndividualTab must exist`);
  assert.ok(dict[language].expertPoolTeamTab, `${language}.expertPoolTeamTab must exist`);
  for (const section of [
    'uiRemote',
    'uiMonitor',
    'uiSettings',
    'uiSettingsDetail',
    'uiPetSettings',
    'uiScheduled',
    'uiChat',
    'uiChatExtra',
    'uiAccount',
    'uiWorkflow',
    'uiToolStore',
    'uiPet',
    'uiWebConnection',
  ]) {
    assert.ok(dict[language][section], `${language}.${section} must exist`);
  }
  assert.ok(dict[language].uiScheduled.createFromTemplate, `${language}.uiScheduled.createFromTemplate must exist`);
  assert.ok(dict[language].uiScheduled.runHistory, `${language}.uiScheduled.runHistory must exist`);
  assert.ok(dict[language].uiSettingsDetail.restartNow, `${language}.uiSettingsDetail.restartNow must exist`);
  assert.ok(dict[language].uiSettingsDetail.deleteModelTitle, `${language}.uiSettingsDetail.deleteModelTitle must exist`);
  assert.ok(dict[language].uiChat.asrDownloadTitle, `${language}.uiChat.asrDownloadTitle must exist`);
  assert.ok(dict[language].uiChat.memoryMeta.preference, `${language}.uiChat.memoryMeta.preference must exist`);
  assert.ok(dict[language].uiChatExtra.draftingScheduled, `${language}.uiChatExtra.draftingScheduled must exist`);
  assert.ok(dict[language].uiAccount.availableQuota, `${language}.uiAccount.availableQuota must exist`);
  assert.ok(dict[language].uiAccount.settingsLoadFailed, `${language}.uiAccount.settingsLoadFailed must exist`);
}

const main = source('app/main.jsx');
assert.match(main, /emit\(['"]ui:language_changed['"], \{ language: lang \}\)/);
assert.match(main, /<ToolStoreView[^>]*t=\{t\}/);
assert.match(main, /<WebConnectionStatus[^>]*t=\{t\}/);
assert.match(main, /<SettingsErrorBoundary[^>]*t=\{t\}/);
assert.match(main, /accountCopy\.settingsLoadFailed/);
assert.doesNotMatch(main, />设置页加载失败</);

const petWindow = source('features/pet/PetWindow.jsx');
assert.match(petWindow, /invokeTauri\(['"]get_settings['"]\)/);
assert.match(petWindow, /listen\(['"]ui:language_changed['"]/);
assert.match(petWindow, /const petCopy = t\.uiPet/);

assert.match(source('features/monitor/MonitorView.jsx'), /t\.uiMonitor/);
const scheduledTasks = source('features/scheduled/ScheduledTasksView.jsx');
assert.match(scheduledTasks, /const scheduledCopy = t\.uiScheduled/);
assert.match(scheduledTasks, /scheduledCopy\.taskName/);
assert.match(scheduledTasks, /scheduledCopy\.runHistory/);
assert.doesNotMatch(scheduledTasks, />立即运行</);
assert.match(source('features/workflow/WorkflowView.jsx'), /t\.uiWorkflow/);
assert.match(source('features/tools/ToolStoreView.jsx'), /const storeCopy = t\.uiToolStore/);
const settings = source('features/settings/SettingsView.jsx');
assert.match(settings, /t\.uiSettings/);
assert.match(settings, /const settingsCopy = t\.uiSettingsDetail/);
assert.match(settings, /settingsCopy\.addSearch/);
assert.match(settings, /settingsCopy\.deleteModelTitle/);
assert.doesNotMatch(settings, />添加搜索源</);
const chat = source('features/chat/ChatView.jsx');
assert.match(chat, /const chatCopy = t\.uiChat/);
assert.match(chat, /chatCopy\.asrDownloadTitle/);
assert.match(chat, /chatCopy\.memoryMeta/);
assert.doesNotMatch(chat, />下载语音识别模型</);
assert.match(source('features/pet/PetSettingsSection.jsx'), /t\.uiPetSettings/);
const personas = source('features/personas/Personas.jsx');
assert.match(personas, /label: t\.expertPoolIndividualTab/);
assert.doesNotMatch(personas, /expertPoolIndividualTab \|\|/);

console.log('UI language coverage tests passed');
