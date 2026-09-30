import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js'; // full three-language dict: browser entry lazy-loads via i18n.js, tests use the aggregate shim
import {
  applyConnectorFailure,
  connectorErrorCopy,
  connectorErrorCodeForStep,
  connectorFailure,
  connectorUiStep,
  errorCode,
} from '../src/features/tools/connector-ui-state.js';

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
    'uiChatWorkspace',
    'artifactPreview',
    'uiToolStore',
    'uiPet',
    'uiWebConnection',
    'uiConversation',
    'uiComputerUse',
    'uiHomeMode',
    'uiAttachments',
    'uiCodex',
    'uiCodexWorkspace',
    'uiAcpProviders',
    'uiProjects',
    'uiArtifacts',
    'uiToolDetails',
    'uiBuiltinPlugins',
    // uiBuiltinFeatures lands with #586's switch UI (its only key was an
    // orphan before that).
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
    // Dynamically-built keys invisible to static scans; these assertions are
    // the deletion guard for all three families:
    // - `copy[`slot_${slot}`]` in ProviderFormModal (CLAUDE_MODEL_SLOT_IDS),
    // - `copy[`agent${Cap(key)}`]` in ProvidersSection (agent registry keys),
    'slot_opus', 'slot_sonnet', 'slot_haiku', 'slot_fable', 'slot_subagent',
    'agentClaude', 'agentCodex', 'agentKimi',
    'contextWindow', 'contextWindowHint', 'contextWindowInvalid',
  ]) {
    assert.ok(dict[language].uiAcpProviders[key], `${language}.uiAcpProviders.${key} must exist`);
  }
  // - `t[`dep_${dep.key}`]` / `t[`depHint_${dep.hint}`]` in SettingsView, with
  //   the key values supplied at runtime by the Rust `checkDependencies`
  //   probes (top-level dict entries).
  for (const key of [
    'dep_pdf', 'dep_office_modern', 'dep_office_legacy', 'dep_ocr', 'dep_archive',
    'dep_email', 'dep_voice_asr', 'dep_voice_asr_model', 'dep_knowledge_embedding_model',
    'depHint_email_manual',
  ]) {
    assert.ok(dict[language][key], `${language}.${key} must exist`);
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
  // The PPT scene returns via the #420 integration into the unified scene
  // cards, so the pptDesign label must exist again.
  assert.ok(dict[language].uiChat.sceneModes.pptDesign, `${language}.uiChat.sceneModes.pptDesign must exist`);
  // The sceneModes keys retired by the design-lane merge into work must
  // stay deleted.
  for (const deadKey of ['designGeneralPlaceholder']) {
    assert.equal(dict[language].uiChat.sceneModes[deadKey], undefined, `${language}.uiChat.sceneModes.${deadKey} is retired and must stay deleted`);
  }
  assert.ok(dict[language].uiChatView.placeholderSceneAdjust, `${language}.uiChatView.placeholderSceneAdjust must exist`);
  assert.ok(dict[language].uiChatView.placeholderSceneDataViz, `${language}.uiChatView.placeholderSceneDataViz must exist`);
  assert.ok(dict[language].uiChatView.placeholderScenePoster, `${language}.uiChatView.placeholderScenePoster must exist`);
  assert.ok(dict[language].uiChatView.placeholderScenePpt, `${language}.uiChatView.placeholderScenePpt must exist`);
  assert.ok(dict[language].uiChatView.placeholderPersonalWorkbench, `${language}.uiChatView.placeholderPersonalWorkbench must exist`);
  assert.ok(dict[language].uiChatScenes.pptDesign, `${language}.uiChatScenes.pptDesign must exist`);
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
  for (const key of ['toggleLabel', 'toggleHint', 'close', 'loadingTranscript', 'emptyTranscript', 'blockedTag', 'panelResize', 'panelResizeHint', 'agentsListTitle', 'agentsEmpty', 'backToAgents', 'spawnedAgentsRowHint', 'runningAgentsTitle', 'runningAgentsCollapse', 'runningAgentsExpand']) {
    assert.ok(multiAgent[key], `${language}.uiMultiAgent.${key} must exist`);
  }
  for (const fnKey of ['spawnedAgentsRow', 'runningAgentsCount']) {
    assert.equal(typeof multiAgent[fnKey], 'function', `${language}.uiMultiAgent.${fnKey} must be a function`);
  }
  for (const cardKey of ['working', 'completed', 'failed', 'spawnFailed', 'interrupted', 'cancelled']) {
    assert.ok(multiAgent.agentCard[cardKey], `${language}.uiMultiAgent.agentCard.${cardKey} must exist`);
  }
  // The ConversationTimeline status badge renders uiConversation.cancelled for
  // the ledger's lowercase cancelled token (case-insensitive match); the key
  // must exist in all three locales.
  assert.ok(dict[language].uiConversation.cancelled, `${language}.uiConversation.cancelled must exist`);
  for (const deadKey of ['confirmTitle', 'impactLabels', 'startDenied', 'stages', 'terminal', 'workerCount', 'advancedEdit', 'planCompileError']) {
    assert.equal(multiAgent[deadKey], undefined, `${language}.uiMultiAgent.${deadKey} is retired and must stay deleted`);
  }
  // agentCard spawning-era keys: 'spawning' died with the inline expert card
  // (the aggregate count row only speaks in past-tense totals).
  assert.equal(multiAgent.agentCard.spawning, undefined, `${language}.uiMultiAgent.agentCard.spawning is retired and must stay deleted`);
  // The bottom-right code-style toggle was replaced by the All/Code pill in
  // the task list header; its tooltip copy must not come back as dead keys.
  for (const deadKey of ['sidebarCodeStyleOn', 'sidebarCodeStyleOff']) {
    assert.equal(dict[language][deadKey], undefined, `${language}.${deadKey} is retired and must stay deleted`);
  }
  // The vendor-residue sweep retired the intranet tool-store copy and the
  // MegaCube sidebar entry (no renderer consumes them and no tool data sets
  // tool.internal); the keys must not come back.
  for (const deadKey of ['internal', 'internalTitle', 'internalDesc', 'internalTools', 'internalCount', 'internalDirect']) {
    assert.equal(dict[language].uiToolStore[deadKey], undefined, `${language}.uiToolStore.${deadKey} is retired and must stay deleted`);
  }
  assert.equal(dict[language].megacubeSite, undefined, `${language}.megacubeSite is retired and must stay deleted`);
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

// uiComputerUse (round-17): type parity across languages — a template
// flattened to a string in any language makes describeConfirmAction's
// function guard fall back to the raw English summary while the generic
// leaf walk above stays green. Also pin that the consent surface and the
// settings section actually consume the namespace.
{
  const typedLeafKeys = (obj, prefix = '') => {
    const out = [];
    for (const key of Object.keys(obj)) {
      const value = obj[key];
      const path = prefix ? `${prefix}.${key}` : key;
      if (value && typeof value === 'object' && !Array.isArray(value)) out.push(...typedLeafKeys(value, path));
      else out.push([path, typeof value]);
    }
    return out;
  };
  const zhTypes = new Map(typedLeafKeys(dict.zh.uiComputerUse));
  assert.ok(zhTypes.size >= 37, `uiComputerUse leaf count looks wrong: ${zhTypes.size}`);
  for (const language of ['en', 'ja']) {
    const types = new Map(typedLeafKeys(dict[language].uiComputerUse));
    for (const [key, type] of zhTypes) {
      assert.equal(types.get(key), type, `${language}.uiComputerUse.${key} must be a ${type} like zh`);
    }
  }
  assert.match(source('features/chat/ChatView.jsx'), /t\.uiComputerUse/);
  assert.match(source('features/settings/SettingsView.jsx'), /uiComputerUse/);
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
assert.match(chat, /chatViewCopy\.placeholderSceneAdjust/);
assert.match(chat, /chatViewCopy\.placeholderSceneDataViz/);
assert.match(chat, /chatViewCopy\.placeholderScenePoster/);
assert.doesNotMatch(chat, /designGeneralPlaceholder/);
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

// Connector flow-card failures: stored as error codes and localized at render
// time, so en/ja users never see raw (often Chinese) backend text. The raw
// backend diagnostic is never rendered in the flow card (same border as the
// Official repo's PR #442: the localized category message is the only copy).
// cli_data_access_disabled is dingtalk's org-level CLI block: only an org
// admin can fix it, so the copy carries the remediation instead of the
// generic retry advice.
const connectorErrorCodes = ['runtime_prepare_failed', 'cli_install_failed', 'auth_start_failed', 'registration_failed', 'auth_failed', 'skills_enable_failed', 'cli_data_access_disabled', 'unknown'];
for (const language of ['zh', 'en', 'ja']) {
  for (const code of connectorErrorCodes) {
    const copy = dict[language].uiToolStore.connectorErrors[code];
    assert.equal(typeof copy, 'string', `${language}.uiToolStore.connectorErrors.${code} must exist`);
    assert.ok(copy.length > 0, `${language}.uiToolStore.connectorErrors.${code} must not be empty`);
  }
  assert.ok(typeof dict[language].updateCheckFailed === 'string' && dict[language].updateCheckFailed.length > 0, `${language}.updateCheckFailed must exist and not be empty`);
  assert.ok(typeof dict[language].updateInstallFailed === 'string' && dict[language].updateInstallFailed.length > 0, `${language}.updateInstallFailed must exist and not be empty`);
  for (const retired of ['connFailed', 'dingtalkSkillsFailed', 'tmeetAuthIncomplete', 'installedReady', 'welcomeOptInFailed', 'switchedOffPacks', 'notAppliedPacks', 'consent_persist_failed']) {
    assert.equal(dict[language].uiToolStore[retired], undefined, `${language}.uiToolStore.${retired} must stay retired/reached via its own contract`);
  }
}
// The render lookup must resolve own keys only: `constructor`/`toString` pass the
// snake_case code pattern but would resolve to Object.prototype functions.
const connectorErrorsDict = dict.en.uiToolStore.connectorErrors;
assert.equal(connectorErrorCopy(connectorErrorsDict, 'auth_failed'), connectorErrorsDict.auth_failed);
assert.equal(connectorErrorCopy(connectorErrorsDict, 'constructor'), '');
assert.equal(connectorErrorCopy(connectorErrorsDict, 'toString'), '');
assert.equal(connectorErrorCopy(connectorErrorsDict, 'valueOf'), '');
assert.equal(connectorErrorCopy(connectorErrorsDict, 'hasOwnProperty'), '');
assert.equal(connectorErrorCopy(connectorErrorsDict, 42), '');
assert.equal(connectorErrorCopy(connectorErrorsDict, null), '');
assert.equal(connectorErrorCopy(null, 'auth_failed'), '');
assert.equal(errorCode({ code: 'auth_failed', message: 'raw diagnostic' }), 'auth_failed');
assert.equal(errorCode(new Error('cli_install_failed: raw backend detail')), 'cli_install_failed');
assert.equal(errorCode(new Error('raw backend detail')), '');
assert.equal(errorCode({ code: 'Not A Code' }), '');
assert.equal(errorCode(null), '');
assert.equal(errorCode(42), '');
assert.equal(errorCode(':leading-separator'), '');
assert.equal(connectorErrorCodeForStep('runtime'), 'runtime_prepare_failed');
assert.equal(connectorErrorCodeForStep('cli'), 'cli_install_failed');
assert.equal(connectorErrorCodeForStep('register'), 'registration_failed');
assert.equal(connectorErrorCodeForStep('qr'), 'auth_failed');
assert.equal(connectorErrorCodeForStep('bogus'), 'unknown');
assert.deepEqual(connectorFailure({ code: 'auth_failed', message: 'raw backend diagnostic' }, 'qr'), { errorCode: 'auth_failed' });
assert.deepEqual(connectorFailure(new Error('raw install diagnostic'), 'cli'), { errorCode: 'cli_install_failed' });
assert.equal(connectorUiStep({ active: 'cli' }, 'authorize'), 'qr');
assert.equal(connectorUiStep({ active: 'connect' }, 'register'), 'connect');
assert.equal(connectorUiStep({ active: 'qr' }, 'register'), 'qr');
assert.equal(connectorUiStep(null, 'register'), 'connect');
assert.equal(connectorUiStep({ active: 'cli' }), 'cli');
{
  const failed = applyConnectorFailure(
    { active: 'cli', steps: { runtime: 'done', cli: 'done' } },
    { code: 'auth_failed', message: 'raw backend diagnostic' },
    'authorize',
  );
  assert.equal(failed.phase, 'error');
  assert.equal(failed.errorCode, 'auth_failed');
  assert.equal(failed.steps.cli, 'done');
  assert.equal(failed.steps.qr, 'error');
  assert.equal(Object.hasOwn(failed.steps, 'authorize'), false);
  // The raw diagnostic must not be carried on the flow state at all.
  assert.equal(Object.hasOwn(failed, 'err'), false);
  assert.equal(Object.hasOwn(failed, 'detail'), false);
  assert.equal(Object.hasOwn(failed, 'errStep'), false);
}
{
  const toolStore = source('features/tools/ToolStoreView.jsx');
  // All four flow cards must receive the localized-code dictionary — one card
  // losing the prop would silently degrade that connector's failures to the
  // generic connectionIncomplete fallback.
  assert.ok(
    (toolStore.match(/errors=\{storeCopy\.connectorErrors\}/g) || []).length >= 4,
    'all four flow cards must receive the connectorErrors dictionary',
  );
  assert.match(toolStore, /connectorErrorCopy\(errors, flow\.errorCode\) \|\| errors\.unknown \|\| copy\.connectionIncomplete/);
  // The connected toast must read its copy through the latest ref at event
  // time, so a mid-flow language switch shows the toast in the new language
  // (the ref effect has no dep array and is declared before the listeners).
  assert.match(toolStore, /const toastCopyRef = useRef\(null\);/);
  assert.match(toolStore, /toastCopyRef\.current = \{ doneTitle, enabledSubtitle: detailCopy\.actions\.enabled \};/);
  assert.match(toolStore, /toastCopyRef\.current && toastCopyRef\.current\.doneTitle/);
  // The auto-collapse timer must only close a card that already reached
  // 'done': a stale timer from a previous round would destroy a freshly
  // re-opened card and strand the busy slot with it.
  assert.ok(
    (toolStore.match(/setTimeout\(\(\) => conn\.setFlow\(f => \(f && f\.phase === 'done' \? null : f\)\), 1800\)/g) || []).length >= 2,
    'both connected listeners must guard the auto-collapse against a re-opened card',
  );
  assert.match(toolStore, /applyConnectorFailure\(f, p, p\.phase\)/);
  assert.doesNotMatch(toolStore, /\berr:\s*String\(/);
  assert.doesNotMatch(toolStore, /flow\.err\b/);
  assert.doesNotMatch(toolStore, /flow\.detail\b/);
  assert.doesNotMatch(toolStore, /console\.error\([^\n]*connect failed:[^\n]*,\s*e\)/);
  const tauriSource = relative => readFileSync(new URL(`../src-tauri/src/${relative}`, import.meta.url), 'utf8');
  const feishu = tauriSource('features/connectors/feishu.rs');
  assert.match(feishu, /"phase": "register", "code": "registration_failed"/);
  assert.match(feishu, /"phase": "authorize", "code": "auth_failed"/);
  for (const connector of ['wecom.rs', 'tmeet.rs']) {
    assert.match(tauriSource(`features/connectors/${connector}`), /"phase": "authorize", "code": "auth_failed"/, `${connector} error events must carry an error code`);
  }
  // DingTalk's error code travels on its FlowError (the org-level CLI block
  // gets its own `cli_data_access_disabled` code; everything else defaults to
  // `auth_failed`), so pin the emit's code field plus the default category.
  const dingtalk = tauriSource('features/connectors/dingtalk.rs');
  assert.match(dingtalk, /"phase": "authorize", "code": e\.code/, 'dingtalk error events must carry the FlowError code');
  assert.match(dingtalk, /code: "auth_failed"/, 'dingtalk FlowError must default to the auth_failed category');
  assert.match(dingtalk, /code: "cli_data_access_disabled"/, 'dingtalk org CLI block must carry its own stable code');
  // Source contract: every connector's error arm must consult BOTH the cancel
  // flag and the generation (cancel alone is cleared by the next reset), and
  // each file must gate at least two emit paths on the generation (the error
  // emit plus the QR emit) — deleting either wiring must fail here, not just
  // the mechanism unit test.
  for (const connector of ['feishu.rs', 'wecom.rs', 'dingtalk.rs', 'tmeet.rs']) {
    const src = tauriSource(`features/connectors/${connector}`);
    assert.match(src, /conn\.is_cancelled\(ID\)\s*\|\|\s*conn\.flow_stale\(ID, generation\)/, `${connector} error arm must guard on cancel OR a superseded generation`);
    assert.ok(
      (src.match(/flow_stale\(ID, generation\)/g) || []).length >= 2,
      `${connector} must consult the generation on both the error and the QR emit path`,
    );
  }
  // The wrapped QR emits (feishu register + authorize / dingtalk / tmeet) gate
  // on the same pair. Feishu has two QR phases, so require two wraps there:
  // the authorize emit once shipped with only a pre-parse guard, and a cancel
  // landing inside parse + QR rendering could still paint a dead device code
  // onto a newer round's card.
  assert.match(tauriSource('features/connectors/dingtalk.rs'), /!\(conn\.is_cancelled\(ID\) \|\| conn\.flow_stale\(ID, generation\)\)/);
  assert.match(tauriSource('features/connectors/tmeet.rs'), /!\(conn\.is_cancelled\(ID\) \|\| conn\.flow_stale\(ID, generation\)\)/);
  assert.ok(
    (feishu.match(/!\(conn\.is_cancelled\(ID\) \|\| conn\.flow_stale\(ID, generation\)\)/g) || []).length >= 2,
    'both feishu QR emits (register and authorize) must be wrapped in the cancel-or-stale guard at the emit site',
  );
  // Frontend: a late error event must not fabricate a flow card from null.
  assert.match(toolStore, /f \? applyConnectorFailure\(f, p, p\.phase\) : f\)/);
  // Frontend: the same guard family on the remaining fabrication points — a
  // late QR event, a connect-catch rejection after the card was closed, and
  // the connected listeners' done write (the smoke only exercises the error
  // path end-to-end, so these three are pinned at source level).
  assert.match(toolStore, /if \(!f\) return f;/, 'the qr listener must drop events onto a closed flow instead of fabricating a card');
  assert.match(toolStore, /f \? applyConnectorFailure\(f, e, stage\) : f\)/, 'the connect catch must not fabricate a card from a closed flow');
  assert.ok(
    (toolStore.match(/f \? \{ \.\.\.f, phase: 'done'/g) || []).length >= 2,
    'both connected listeners must guard the done write against a closed flow',
  );
  // Source contract: every pid-slot cleanup must be the compare-and-set. A
  // superseded round's plain set_pid(ID, None) would clear the pid a newer
  // round registered, making its cancel tree-kill a no-op (connector_cli.rs
  // unit-tests the CAS mechanism; this pins that every flow uses it).
  for (const connector of ['feishu.rs', 'wecom.rs', 'dingtalk.rs', 'tmeet.rs']) {
    const src = tauriSource(`features/connectors/${connector}`);
    assert.doesNotMatch(src, /set_pid\(ID, None\)/, `${connector} pid cleanup must use the clear_pid_if compare-and-set`);
    assert.match(src, /let pid = child\.id\(\);/, `${connector} must capture its child pid for the CAS clear`);
    assert.match(src, /clear_pid_if\(ID, pid\)/, `${connector} must clear the pid slot via clear_pid_if`);
  }
  // Source contract: a reconnect without an intervening cancel must not orphan
  // the previous round's registered child — begin's reset overwrites the pid
  // slot, which would make that child invisible to cancel's tree-kill and to
  // kill_all_pids at exit. Every begin therefore tree-kills the registered pid
  // before resetting (the smoke cannot exercise real child processes, so this
  // is pinned at source level like the other begin-time wiring).
  for (const connector of ['feishu.rs', 'wecom.rs', 'dingtalk.rs', 'tmeet.rs']) {
    const src = tauriSource(`features/connectors/${connector}`);
    assert.match(src, /if let Some\(pid\) = conn\.cancel\(ID\)/, `${connector} connect_begin must take the previous round's registered pid before reset`);
    assert.match(src, /kill_pid_tree\(pid\)/, `${connector} connect_begin must tree-kill the stale child`);
  }
  assert.match(source('features/settings/SettingsView.jsx'), /item\.title \|\| presetProviderLabel\(p, t\)/);
}

console.log('UI language coverage tests passed');
