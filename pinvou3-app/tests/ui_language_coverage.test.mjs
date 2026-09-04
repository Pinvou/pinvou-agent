import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from '../src/shared/i18n-all.js'; // 三语全量断言:浏览器入口用 i18n.js 惰性装载,测试用聚合 shim

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

for (const language of ['zh', 'en', 'ja']) {
  for (const section of [
    'uiRemote',
    'uiMonitor',
    'uiSettings',
    'uiSettingsDetail',
    'uiPetSettings',
    'uiScheduled',
    'uiChat',
    'uiChatExtra',
    'uiChatScenes',
    'artifactPreview',
    'uiToolStore',
    'uiPet',
    'uiWebConnection',
    'uiConversation',
    'uiHomeMode',
    'uiAttachments',
    'uiCodex',
    'uiCodexWorkspace',
    'uiAcpProviders',
    'uiArtifacts',
    'uiToolDetails',
  ]) {
    assert.ok(dict[language][section], `${language}.${section} must exist`);
  }
  assert.ok(dict[language].uiSettings.providers, `${language}.uiSettings.providers must exist`);
  for (const key of [
    'addProvider', 'switch', 'official', 'current', 'export', 'import',
    'envConflictTitle', 'uninstallTitle', 'sessionProvider', 'faultManage',
    'thirdPartyWarning', 'deleteTitle', 'secretSet', 'notEnabled', 'restoreOfficial',
    'login', 'logout', 'loginWaiting', 'openLoginUrl', 'loginCodePlaceholder', 'submitCode', 'logoutRelayDisabled',
    'cancelInstall', 'installCancelled',
    'modelSlotsTitle', 'modelSlotsHint', 'modelSlotsRequired',
    'slot_opus', 'slot_sonnet', 'slot_haiku', 'slot_fable', 'slot_subagent',
    'contextWindow', 'contextWindowHint', 'contextWindowInvalid',
  ]) {
    assert.ok(dict[language].uiAcpProviders[key], `${language}.uiAcpProviders.${key} must exist`);
  }
  assert.ok(dict[language].uiScheduled.createFromTemplate, `${language}.uiScheduled.createFromTemplate must exist`);
  assert.ok(dict[language].uiScheduled.runHistory, `${language}.uiScheduled.runHistory must exist`);
  assert.ok(dict[language].uiSettingsDetail.restartNow, `${language}.uiSettingsDetail.restartNow must exist`);
  assert.ok(dict[language].uiSettingsDetail.deleteModelTitle, `${language}.uiSettingsDetail.deleteModelTitle must exist`);
  assert.ok(dict[language].uiChat.asrDownloadTitle, `${language}.uiChat.asrDownloadTitle must exist`);
  assert.ok(dict[language].uiChat.memoryMeta.preference, `${language}.uiChat.memoryMeta.preference must exist`);
  assert.ok(dict[language].uiChat.sceneModes.personalWorkbench, `${language}.uiChat.sceneModes.personalWorkbench must exist`);
  assert.ok(dict[language].uiChat.sceneModes.documentWriting, `${language}.uiChat.sceneModes.documentWriting must exist`);
  assert.ok(dict[language].uiChat.sceneModes.poster, `${language}.uiChat.sceneModes.poster must exist`);
  assert.ok(dict[language].uiChat.sceneModes.dataVisualization, `${language}.uiChat.sceneModes.dataVisualization must exist`);
  assert.ok(dict[language].uiChat.sceneModes.pptDesign, `${language}.uiChat.sceneModes.pptDesign must exist`);
  assert.ok(dict[language].uiChat.sceneModes.pptUnavailable, `${language}.uiChat.sceneModes.pptUnavailable must exist`);
  assert.ok(dict[language].uiChat.sceneModes.designGeneralPlaceholder, `${language}.uiChat.sceneModes.designGeneralPlaceholder must exist`);
  assert.ok(dict[language].uiChatView.placeholderPersonalWorkbench, `${language}.uiChatView.placeholderPersonalWorkbench must exist`);
  assert.equal(
    typeof dict[language].uiChat.sceneModes.clear,
    'function',
    `${language}.uiChat.sceneModes.clear must be a function`,
  );
  assert.ok(dict[language].uiChatExtra.draftingScheduled, `${language}.uiChatExtra.draftingScheduled must exist`);
  assert.ok(dict[language].uiSettingsDetail.settingsLoadFailed, `${language}.uiSettingsDetail.settingsLoadFailed must exist`);
  // uiMultiAgent 收缩为活键集（ADR-0006）：开关行 + 行内专家卡 + 只读面板。
  // 确认卡/审批链/台账时代的键已随旧入口退役，不再断言存在。
  const multiAgent = dict[language].uiMultiAgent;
  assert.equal(typeof multiAgent.drawerTitle, 'function', `${language}.uiMultiAgent.drawerTitle must be a function`);
  assert.equal(typeof multiAgent.coordinationRow, 'function', `${language}.uiMultiAgent.coordinationRow must be a function`);
  for (const key of ['agentsListSummary', 'childAgentCount', 'expandChildren', 'collapseChildren']) {
    assert.equal(typeof multiAgent[key], 'function', `${language}.uiMultiAgent.${key} must be a function`);
  }
  for (const role of ['scout', 'manager', 'builder', 'reviewer', 'general']) {
    assert.ok(multiAgent.roleCards[role], `${language}.uiMultiAgent.roleCards.${role} must exist`);
  }
  for (const key of ['toggleLabel', 'toggleHint', 'close', 'loadingTranscript', 'emptyTranscript', 'blockedTag', 'panelResize', 'panelResizeHint', 'agentsListTitle', 'agentsEmpty', 'backToAgents']) {
    assert.ok(multiAgent[key], `${language}.uiMultiAgent.${key} must exist`);
  }
  for (const cardKey of ['spawning', 'working', 'completed', 'failed', 'spawnFailed', 'interrupted']) {
    assert.ok(multiAgent.agentCard[cardKey], `${language}.uiMultiAgent.agentCard.${cardKey} must exist`);
  }
  for (const deadKey of ['confirmTitle', 'impactLabels', 'startDenied', 'stages', 'terminal', 'workerCount', 'advancedEdit', 'planCompileError']) {
    assert.equal(multiAgent[deadKey], undefined, `${language}.uiMultiAgent.${deadKey} is retired and must stay deleted`);
  }
  // The bottom-right code-style toggle was replaced by the All/Code pill in
  // the task list header; its tooltip copy must not come back as dead keys.
  for (const deadKey of ['sidebarCodeStyleOn', 'sidebarCodeStyleOff']) {
    assert.equal(dict[language][deadKey], undefined, `${language}.${deadKey} is retired and must stay deleted`);
  }
}

// 三语 key parity:zh 是全集基准,en 必须覆盖 zh 的每个叶子 key(ja 经 en
// spread 兜底,同样断言)。en/ja 历史上比 zh 多出的 settings 相关键属基线
// 固有(zh 侧由 UI 层默认值兜底),故只断言方向性覆盖而非严格相等——漏 key
// 的 en 用户会渲染 undefined,ja 也随 en 一起漏。
{
  const leafPaths = (obj, prefix = '') => {
    const out = [];
    for (const key of Object.keys(obj)) {
      const value = obj[key];
      const path = prefix ? `${prefix}.${key}` : key;
      if (value && typeof value === 'object' && !Array.isArray(value)) out.push(...leafPaths(value, path));
      else out.push(path);
    }
    return out;
  };
  const zhKeys = leafPaths(dict.zh);
  assert.ok(zhKeys.length > 2000, `zh leaf key count looks wrong: ${zhKeys.length}`);
  for (const language of ['en', 'ja']) {
    const known = new Set(leafPaths(dict[language]));
    const missing = zhKeys.filter((key) => !known.has(key));
    assert.deepEqual(
      missing,
      [],
      `${language} dictionary misses zh keys (renders undefined): ${missing.slice(0, 10).join(', ')}`,
    );
  }
}

const main = source('app/main.jsx');
assert.match(main, /emit\(['"]ui:language_changed['"], \{ language: lang \}\)/);
const viewLoaders = source('app/view-loaders.js');
assert.match(viewLoaders, new RegExp("toolStore: \\(\\) => import\\('\\.\\./features/tools/ToolStoreView\\.jsx'\\)"));
assert.match(main, /<LazyToolStoreView[^>]*t=\{t\}/);
assert.match(main, /<WebConnectionStatus[^>]*t=\{t\}/);
assert.match(main, /<ViewErrorBoundary[^>]*heading=\{t\.uiSettingsDetail\.settingsLoadFailed\}[^>]*t=\{t\}/);
assert.match(viewLoaders, new RegExp("codex: \\(\\) => import\\('\\.\\./features/codex/CodexAcpView\\.jsx'\\)"));
// The main window renders codex through the LazyCodexAcpView wrapper (which
// internally consumes the same view-loaders chunk); its error fallback copy
// still flows through i18n.
assert.match(main, /<CodexAcpView[^>]*t=\{t\}/);
// The settings error boundary has been merged into the shared ViewErrorBoundary; its title flows through i18n via the heading prop.
const viewErrorBoundary = source('shared/ViewErrorBoundary.jsx');
assert.match(viewErrorBoundary, /this\.props\.heading \|\| copy\.viewLoadFailed/);
assert.doesNotMatch(viewErrorBoundary, />设置页加载失败</);

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
assert.match(source('features/tools/ToolStoreView.jsx'), /const storeCopy = t\.uiToolStore/);
assert.match(source('features/tools/ToolStoreView.jsx'), /localizeTool\(baseTool, t\)/);
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
assert.match(chat, /chatCopy\.sceneModes/);
assert.match(chat, /sceneCopy\.designGeneralPlaceholder/);
assert.doesNotMatch(chat, /label:\s*'个人工作台'/);
assert.doesNotMatch(chat, /label:\s*'公文写作'/);
assert.doesNotMatch(chat, /label:\s*'数据可视化'/);
assert.doesNotMatch(chat, /`取消\$\{scene\.label\}`/);
assert.doesNotMatch(chat, /:\s*'描述你想生成或调整的内容'/);
assert.doesNotMatch(chat, />下载语音识别模型</);
assert.match(source('features/pet/PetSettingsSection.jsx'), /t\.uiPetSettings/);
const conversation = source('features/conversation/ConversationTimeline.jsx');
assert.match(conversation, /conversationCopy\(copy\)/);
assert.doesNotMatch(conversation, />等待授权</);
const codex = source('features/codex/CodexAcpView.jsx');
assert.match(codex, /const codexCopy = t\.uiCodex/);
assert.match(codex, /copy=\{t\.uiConversation\}/);
assert.match(codex, /copy=\{t\.uiCodexWorkspace\}/);
const workspace = source('features/codex/CodexWorkspacePanel.jsx');
assert.match(workspace, /\{copy\.title\}/);
assert.doesNotMatch(workspace, />工作区</);
const providersSection = source('features/settings/ProvidersSection.jsx');
assert.match(providersSection, /const copy = t\.uiAcpProviders/);
assert.doesNotMatch(providersSection, />新增 Provider</);
assert.doesNotMatch(providersSection, />切换</);
assert.doesNotMatch(providersSection, />官方登录</);
const settingsViewProviders = source('features/settings/SettingsView.jsx');
assert.match(settingsViewProviders, /t\.uiSettings\.providers/);
assert.match(settingsViewProviders, /<ProvidersSection/);
const providerFormModal = source('features/settings/ProviderFormModal.jsx');
assert.match(providerFormModal, /invokeTauri\(['"]save_acp_provider['"]/);
assert.doesNotMatch(providerFormModal, /placeholder=\{?['"]输入 API Key/);
const codexViewProviders = source('features/codex/CodexAcpView.jsx');
assert.match(codexViewProviders, /set_codex_acp_session_provider/);
assert.match(codexViewProviders, /t\.uiAcpProviders/);
const personas = source('features/personas/Personas.jsx');
assert.match(personas, /\{t\.cpMyCards\}/);
assert.doesNotMatch(personas, /ExpertTeamsPanel|expertPoolTeamTab|expertPoolIndividualTab/);

// 设计检查器字体预设:数据侧 label 保留中文原名(选中态比较键),展示走 labelKey;
// 每个预设都必须声明 labelKey 且三语词典都有该 key,否则 en/ja 用户会看到中文原名。
const fontPresets = source('features/artifacts/DesignInspectorPanel.jsx');
const fontLabelKeys = [...fontPresets.matchAll(/labelKey: '(\w+)'/g)].map(m => m[1]);
assert.ok(fontLabelKeys.length >= 9, `font presets should declare labelKey, found ${fontLabelKeys.length}`);
for (const language of ['zh', 'en', 'ja']) {
  for (const key of fontLabelKeys) {
    assert.ok(dict[language].uiArtifacts[key], `${language}.uiArtifacts.${key} must exist`);
  }
}

// 个人工作台模板 chip:每个模板 id 三语都要有展示名;数据侧 zh title 只是
// 草稿匹配与消息 meta 的正本,不直接展示给 en/ja 用户。
const workbench = source('features/chat/personal-workbench-scene.js');
const workbenchTemplateIds = [...workbench.matchAll(/\n {4}id: '([\w-]+)',/g)].map(m => m[1]);
assert.equal(workbenchTemplateIds.length, 7, `expected 7 workbench templates, found ${workbenchTemplateIds.length}`);
for (const language of ['zh', 'en', 'ja']) {
  for (const id of workbenchTemplateIds) {
    assert.ok(dict[language].uiChatScenes.workbenchTemplates[id], `${language}.uiChatScenes.workbenchTemplates.${id} must exist`);
  }
}

console.log('UI language coverage tests passed');
