import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js'; // full three-language dict: browser entry lazy-loads via i18n.js, tests use the aggregate shim

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
    'uiHomeMode',
    'uiAttachments',
    'uiCodex',
    'uiCodexWorkspace',
    'uiAcpProviders',
    'uiProjects',
    'uiArtifacts',
    'uiToolDetails',
    'uiAuxChat',
  ]) {
    assert.ok(dict[language][section], `${language}.${section} must exist`);
  }
  // All 21 uiAuxChat keys are pinned (round-14 minor-3: the list previously
  // covered 13, so sendingHint/bindingHint and the six quote* keys could be
  // deleted from every dictionary with the suite green — and quoteChipCount's
  // absence renders `undefined` at runtime).
  for (const key of [
    'openLabel', 'panelTitle', 'landingHint', 'emptyState', 'inputPlaceholder',
    'send', 'busyHint', 'bindingHint', 'sendingHint', 'newTopic', 'newTopicConfirm',
    'sendFailed', 'ensureFailed', 'discardFailed', 'close',
    'quoteAction', 'quoteChipCount', 'quoteRemove',
    'quoteLimitSingle', 'quoteLimitCount', 'quoteLimitTotal',
  ]) {
    assert.ok(dict[language].uiAuxChat[key], `${language}.uiAuxChat.${key} must exist`);
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
assert.match(chat, /data-testid="aux-chat-open"/);
// Work-mode entry parity with code mode (round-9 minor-2): the entry pill
// reflects the panel's real dock visibility via onActiveChange — no highlight
// while another dock panel occludes the aux panel.
assert.match(chat, /onActiveChange=\{setAuxChatDockActive\}/);
assert.match(chat, /auxChatPanel && auxChatDockActive/);
const auxChatPanel = source('features/aux-chat/AuxChatPanel.jsx');
assert.match(auxChatPanel, /const copy = t\.uiAuxChat/);
assert.match(auxChatPanel, /copy=\{conversationCopy\}/);
// Restart-topic staged guards: discard and ensure are wrapped in separate
// try/catch blocks — a discard failure keeps the binding and snapshot as-is
// (the old session is still usable) and shows discardFailed; after the
// discard round trip the generation must be re-checked before ensure, or a
// rebind would idempotently recreate the just-discarded aux session on the
// backend; an ensure failure must clear the binding and show ensureFailed
// (the composer is already disabled and sendFailed's "retry send" copy would
// mislead). The two stages must share **one** outer try/finally that resets
// restarting: no early return (discard failure / generation mismatch) may
// latch the panel in the restarting state.
const restartBlock = auxChatPanel.slice(
  auxChatPanel.indexOf('const handleRestart'),
);
assert.match(restartBlock, /try \{\s*try \{[\s\S]*?const discardPromise = auxChat\.discard\(sessionId\);[\s\S]*?await discardPromise;[\s\S]*?\} catch[\s\S]*?setDiscardFailed\(true\);[\s\S]*?generationRef\.current !== generation\) return;\s*try \{\s*const nextAuxId = await auxChat\.ensure\(sessionId\)/);
assert.match(restartBlock, /setEnsureFailed\(true\)/);
assert.doesNotMatch(restartBlock, /setSendFailed\(true\)/);
assert.match(auxChatPanel, /copy\.discardFailed/);
// Generation bump at restart entry (round-11 B3): only the rebind effect
// increments the generation otherwise, so an ensure still in flight from the
// current rebind (including its ensureSessionBufferLoaded chain) would resolve
// after the restart's discard+ensure with a matching generation and rebind the
// panel to the just-discarded aux session. The bump must precede the capture.
const generationBump = restartBlock.indexOf('generationRef.current += 1;');
const generationCapture = restartBlock.indexOf('const generation = generationRef.current;');
assert.ok(
  generationBump >= 0 && generationBump < generationCapture,
  'handleRestart must bump generationRef at entry so in-flight rebind ensures go stale',
);
// In-flight discard registry (round-7 M-B, rescoped module-level in round-8
// M-2): while the backend turn gate waits out a running turn, the old mapping
// is still live — the rebind effect must await the registered discard promise
// before re-ensuring the same task, or it would bind the doomed aux session
// that the discard then deletes. The registry must be module-scoped, not a
// component useRef: the discard is backend-scoped while the panel unmounts on
// close / sched- switches, and an instance-level registry would die with the
// unmount and re-open the exact hole through remount.
assert.match(auxChatPanel, /const discardInFlightByTask = new Map\(\);/);
assert.doesNotMatch(auxChatPanel, /discardInFlightRef/);
assert.match(restartBlock, /discardInFlightByTask\.set\(sessionId, discardPromise\)/);
assert.match(restartBlock, /discardInFlightByTask\.delete\(sessionId\)/);
assert.match(auxChatPanel, /discardInFlightByTask\.get\(sessionId\)/);
// Duplicate-discard guard at restart entry (round-12 N1): a task switch resets
// `restarting` while the previous new-topic discard can still be parked in the
// backend turn gate. A second discard would then overwrite the registry entry,
// so nobody awaits the first one anymore — and on the web relay (invoke
// responses are not FIFO) that orphaned discard can land after this restart's
// recreate and delete the aux session the panel just bound to. The guard must
// run before the arm/confirm branch so an in-flight discard also cannot arm.
const duplicateDiscardGuard = restartBlock.indexOf('if (discardInFlightByTask.has(sessionId)) return;');
assert.ok(duplicateDiscardGuard >= 0, 'handleRestart must refuse a second discard while one is in flight');
assert.ok(
  duplicateDiscardGuard < restartBlock.indexOf('if (!restartArmed) {'),
  'the duplicate-discard guard must precede the two-step confirm arming',
);
// Send-latch release at restart entry (round-12 N2): handleSend's finally only
// clears sendingRef while the binding is unchanged, and a same-task restart
// does not re-run the rebind effect that otherwise resets it. A send settling
// after the restart would therefore latch sendingRef true permanently and every
// later Enter would silently no-op behind a visually enabled composer.
const sendLatchReset = restartBlock.indexOf('sendingRef.current = false;');
assert.ok(sendLatchReset >= 0, 'handleRestart must release the in-flight send latch');
assert.ok(
  sendLatchReset < restartBlock.indexOf('const discardPromise = auxChat.discard(sessionId);'),

  'the send latch must be released at restart entry, before the discard await',
);
// Registry recovery at restart entry (round-16 B1): a never-settling invoke
// leaves its sendInFlightByTask entry forever, and the registry outlives
// rebinds by design — without this clear, every later Enter on the task
// silently no-ops at the guard, and New Topic itself could not recover the
// panel because the fresh aux binds under the same task key. Deleting the
// entry at restart entry is safe because the stale send's finally only
// removes the entry it registered (promise identity).
const sendRegistryReset = restartBlock.indexOf('sendInFlightByTask.delete(sessionId);');
assert.ok(sendRegistryReset >= 0, 'handleRestart must clear the task\'s send registry entry');
assert.ok(
  sendRegistryReset > sendLatchReset
    && sendRegistryReset < restartBlock.indexOf('const discardPromise = auxChat.discard(sessionId);'),
  'the registry entry must be cleared at restart entry, before the discard await',
);
// Binding null at restart entry (round-18 B-1): a send settling inside the
// discard window must read as the restart case (keep-draft skip). With the
// binding left set, its success continuation would consume the draft and
// staged quotes as "delivered" and the discard would then destroy both the
// transcript and the recovery material. The null must precede the discard
// await and mirror the rebind effect's reset (ref + state).
const bindingNull = restartBlock.indexOf('auxIdRef.current = null;');
assert.ok(bindingNull >= 0, 'handleRestart must null the binding at entry');
assert.ok(
  bindingNull > sendRegistryReset
    && bindingNull < restartBlock.indexOf('const discardPromise = auxChat.discard(sessionId);'),
  'the binding must be nulled at restart entry, before the discard await',
);
assert.match(restartBlock, /auxIdRef\.current = null;\s*setAuxId\(null\);/);
// Binding-pending hint (round-12 UX): "first open / rebind shows a false
// 'nothing here yet' while ensure is in flight". The pending flag must be
// raised wherever a binding is being acquired (rebind effect and restart
// entry), cleared on both ensure outcomes, and it must drive the timeline copy
// instead of the empty state.
assert.match(auxChatPanel, /const \[bindingPending, setBindingPending\] = useState\(false\);/);
assert.match(auxChatPanel, /setBindingPending\(!!\(auxChat && sessionId\)\);/);
assert.match(auxChatPanel, /setBindingPending\(true\);[\s\S]*?const discardPromise = auxChat\.discard\(sessionId\);/);
assert.equal(
  (auxChatPanel.match(/setBindingPending\(false\);/g) || []).length,
  3,
  'bindingPending must clear on ensure success, ensure failure and the restart finally',
);
assert.match(auxChatPanel, /bindingPending \? copy\.bindingHint : copy\.emptyState/);
// In-flight send feedback (round-12 UX): the send window had no visible state
// because snapshot-busy only lands with the backend turn_started, so the
// composer looked idle while the message was already gone.
assert.match(auxChatPanel, /const \[sending, setSending\] = useState\(false\);/);
assert.match(auxChatPanel, /sendingRef\.current = true;\s*setSending\(true\);/);
assert.match(auxChatPanel, /sendingRef\.current = false;\s*setSending\(false\);/);
assert.match(auxChatPanel, /\{sending && !busy && \(/);
// Draft preservation (round-12 UX): text the user typed but never sent must not
// be wiped by a task switch, a close/reopen or a new-topic confirm — only a
// successful send clears it.
assert.match(auxChatPanel, /const draftByTask = new Map\(\);/);
assert.match(auxChatPanel, /setDraft\(sessionId \? \(draftByTask\.get\(sessionId\) \|\| ''\) : ''\);/);
assert.match(auxChatPanel, /if \(sessionId\) draftByTask\.set\(sessionId, next\);/);
// A successful send consumes only what it actually sent: the task draft is
// cleared only when it still equals the sent text (text typed during the
// in-flight window belongs to the next message), and only the quotes captured
// when the send started are dropped — quotes staged from the main view while
// the send was in flight survive for the next message.
assert.match(
  auxChatPanel,
  /if \(sentTaskId\) \{[\s\S]{0,600}?draftByTask\.delete\(sentTaskId\);[\s\S]{0,300}?dropAuxQuotes\(sentTaskId, quotes\);\s*\}/,
);
// Same-binding consumption regardless of generation (round-14 B3), widened
// in round-17 M-A: the success path consumes whenever the send settled into a
// live transcript — same binding (the composer shows this task) OR the panel
// moved to another task (A's aux is alive on its own binding). Only the
// same-task fresh-aux restart case skips, keeping the draft as recovery. The
// skip must be the restart-only conjunction, and consumption must follow it
// with no generation guard.
assert.match(
  auxChatPanel,
  /const sendPromise = auxChat\.send\(sentAuxId, quoteBlock \? text \+ quoteBlock : text\);[\s\S]{0,200}?await sendPromise;\s*\n[\s\S]{0,1200}?const sameBinding = auxIdRef\.current === sentAuxId;\s*const onSameTask = sessionIdRef\.current === sentTaskId;\s*if \(!sameBinding && onSameTask\) return;\s*if \(sentTaskId\) \{/,
  'consumption must follow the restart-only skip directly, gated on binding and live task identity',
);
// The UI-touching part (visible draft clear, snapshot pull) stays
// binding-gated after the store-map consumption.
assert.match(
  auxChatPanel,
  /dropAuxQuotes\(sentTaskId, quotes\);\s*\}\s*if \(!sameBinding\) return;\s*setDraft\(\(current\) =>/,
  'only the store-map consumption runs for a send settling into another task',
);
assert.match(auxChatPanel, /const sessionIdRef = useRef\(sessionId\);/);
assert.match(auxChatPanel, /setDraft\(\(current\) => \(current\.trim\(\) === text \? '' : current\)\)/);
assert.doesNotMatch(restartBlock, /setDraft\(''\)/);
// Conversation quotes ("划词引用"): staged per task through the aux-quote store
// (module scope, same ownership as the draft) and appended to the outgoing
// message as an inline userselect block; a quote-only send is allowed.
assert.match(auxChatPanel, /const quoteBlock = buildAuxQuoteBlock\(quotes\);/);
assert.match(auxChatPanel, /const sendPromise = auxChat\.send\(sentAuxId, quoteBlock \? text \+ quoteBlock : text\);/);
assert.match(auxChatPanel, /\(!text && !quoteBlock\)/);
assert.match(auxChatPanel, /subscribeAuxQuotes\(sessionId/);
assert.match(auxChatPanel, /data-testid="aux-quote-chips"/);
assert.match(auxChatPanel, /data-testid="aux-quote-remove"/);
// Quote-only send affordance: with staged quotes the composer stays usable
// even while the draft is empty (E2E scenario 7 covers it, but that does not
// run in CI).
assert.match(auxChatPanel, /disabled=\{composerDisabled \|\| \(!draft\.trim\(\) && quotes\.length === 0\)\}/);
// Stale send outcomes (round-12 UX): a restart on the same task re-binds to a
// new aux, so a send issued before it must neither re-latch sendFailed next to
// ensureFailed ("double banner") nor clear text typed since.
assert.match(auxChatPanel, /const sentGeneration = generationRef\.current;/);
assert.match(auxChatPanel, /if \(generationRef\.current !== sentGeneration\) return;\s*if \(auxIdRef\.current !== sentAuxId\) return;\s*setSendFailed\(true\);/);
// restarting leak guard: the whole function body has exactly one
// setRestarting(false), located in the outer finally (whose try opens before
// the discard await and whose finally closes after the ensure await) — every
// early-return path resets through it. The reset must be generation-gated
// (round-8 m2): a stale continuation must not clear a newer restart's latch.
// The binding-pending hint rides the same gate (round-12 UX): a stale
// continuation must not clear a newer restart's pending state either.
const restartingClears = restartBlock.match(/setRestarting\(false\)/g) || [];
assert.equal(restartingClears.length, 1, 'restarting must be cleared at exactly one place in handleRestart');
const outerTry = restartBlock.indexOf('try {');
const discardAwait = restartBlock.indexOf('await discardPromise;');
const ensureAwait = restartBlock.indexOf('await auxChat.ensure');
const restartingClearIdx = restartBlock.indexOf('setRestarting(false)');
const finallyClause = restartBlock.lastIndexOf('} finally {', restartingClearIdx);
assert.ok(
  outerTry >= 0 && outerTry < discardAwait && discardAwait < ensureAwait && ensureAwait < finallyClause,
  'a single outer try must span discard+ensure so its finally resets restarting on every early return',
);
assert.match(restartBlock.slice(finallyClause), /} finally \{[\s\S]*?if \(generationRef\.current === generation\) \{\s*setRestarting\(false\);\s*setBindingPending\(false\);\s*\}/);
// In-flight send latch (round-7 M-A): snapshot-busy lags the dispatch by one
// event round trip, so without a synchronous latch a double Enter fires a
// duplicate turn whose rejection surfaces as a bogus "send failed" banner;
// key-repeat Enter must be ignored outright. The latch must be released on
// every outcome via finally, or the composer would lock after one failure.
assert.match(auxChatPanel, /if \(!auxChat \|\| !sentAuxId \|\| \(!text && !quoteBlock\) \|\| busy \|\| restarting \|\| sendingRef\.current\) return;/);
assert.match(auxChatPanel, /sendingRef\.current = true;[\s\S]*?const sendPromise = auxChat\.send\(sentAuxId, quoteBlock \? text \+ quoteBlock : text\);[\s\S]*?await sendPromise;[\s\S]*?\} finally \{[\s\S]*?if \(auxIdRef\.current === sentAuxId && generationRef\.current === sentGeneration\) \{\s*sendingRef\.current = false;/);
assert.match(auxChatPanel, /if \(event\.repeat\) return;/);
// Rebind resets restarting (round-7 m11): the restart invokes have no
// transport timeout, so a promise that never settles must not latch the next
// task's panel disabled — the rebind effect resets the flag itself, between
// the restartArmed reset and the draft reset in the binding-state reset block.
// The slice anchors must resolve (round-17 B-B): the previous end anchor was
// deleted by the round-14 draft-preservation change, so indexOf returned -1
// and slice(start, -1) ran to EOF — both assertions were vacuously satisfied
// by handleRestart's own copies. Anchor to the effect's closing deps line
// and assert the anchors resolve, so a future rename fails loudly.
const rebindStart = auxChatPanel.indexOf('const generation = generationRef.current + 1;');
const rebindEnd = auxChatPanel.indexOf('}, [auxChat, sessionId, pullSnapshot]);');
assert.ok(
  rebindStart >= 0 && rebindEnd > rebindStart,
  'rebind effect anchors must resolve (a vacuous slice would pass on handleRestart copies)',
);
const rebindBlock = auxChatPanel.slice(rebindStart, rebindEnd);
assert.match(rebindBlock, /setRestarting\(false\);/, 'the rebind effect must reset restarting itself');
// Rebind also resets the send latch (round-8 m3): a never-settling
// auxChat.send invoke (same no-transport-timeout class) must not latch sends
// across later task rebinds either.
assert.match(rebindBlock, /sendingRef\.current = false;/, 'the rebind effect must reset the send latch itself');
// Stale-finally latch guard (round-9 minor-3, tightened in round-14 B2): an
// old send's late finally must not clear the latch a newer send relies on —
// a same-id rebind (switch A→B→A; ensure is idempotent) keeps auxIdRef equal
// to sentAuxId, so the release must be gated on the send's generation as well
// as the binding.
assert.match(auxChatPanel, /finally \{[\s\S]{0,1300}if \(auxIdRef\.current === sentAuxId && generationRef\.current === sentGeneration\) \{\s*sendingRef\.current = false;\s*setSending\(false\);\s*\}/);
// Task-keyed in-flight send registry (round-15 MAJOR-2): the rebind effect
// resets sendingRef on every task switch, so without a module-scoped registry
// a send on task A → switch to B → back to A would pass every guard before
// turn_started lands, firing a duplicate turn on the same aux session. The
// registry must be checked in the guard, registered synchronously with the
// dispatch, and removed by the exact send that registered it (identity check,
// independent of the binding state, or a post-rebind settle would leak the
// entry and block the task's sends forever).
assert.match(auxChatPanel, /const sendInFlightByTask = new Map\(\);/);
assert.match(auxChatPanel, /if \(sendInFlightByTask\.has\(sentTaskId\)\) return;/);
assert.match(auxChatPanel, /sendInFlightByTask\.set\(sentTaskId, sendPromise\);/);
assert.match(auxChatPanel, /finally \{[\s\S]{0,400}?if \(sendInFlightByTask\.get\(sentTaskId\) === sendPromise\) \{\s*sendInFlightByTask\.delete\(sentTaskId\);\s*\}/);
// Double-banner guard (round-9 minor-1): entering the restart flow must
// clear a stale sendFailed too, or a failed send's "retry" banner renders
// next to the ensure-failure banner after the binding was cleared.
assert.match(restartBlock, /setRestarting\(true\);\s*setDiscardFailed\(false\);[\s\S]{0,400}setSendFailed\(false\);/);
assert.match(source('features/pet/PetSettingsSection.jsx'), /t\.uiPetSettings/);
const conversation = source('features/conversation/ConversationTimeline.jsx');
assert.match(conversation, /conversationCopy\(copy\)/);
assert.doesNotMatch(conversation, />等待授权</);
// Aux quote chips ("划词引用") in the user bubble: the aux projection strips
// the inline userselect block from userText and hands the excerpts over as
// userQuotes, so this render branch is the only place the quoted content is
// still visible. If it regresses, quotes disappear from the transcript
// silently — the raw block is already stripped and no raw text remains.
assert.match(conversation, /const userQuotes = Array\.isArray\(turn\.userQuotes\) \? turn\.userQuotes : \[\];/);
assert.match(conversation, /turn\.userText \|\| userAttachments\.length \|\| userQuotes\.length/);
assert.match(conversation, /userQuotes\.map\(\(quote, index\)/);
assert.match(conversation, /data-testid="conversation-user-quote"/);
const codex = source('features/codex/CodexAcpView.jsx');
assert.match(codex, /const codexCopy = t\.uiCodex/);
assert.match(codex, /copy=\{t\.uiConversation\}/);
assert.match(codex, /copy=\{t\.uiCodexWorkspace\}/);
assert.match(codex, /data-testid="aux-chat-open"/);
assert.match(codex, /t\.uiAuxChat\.openLabel/);
// The code-mode aux entry must be native-agent-only (round-18 must-land): an
// external-ACP task's side chat would silently answer on Pinvou's internal
// default model while the panel copy implies the task's own assistant — the
// same reason the quote popover suppresses on external ACP.
assert.match(codex, /\{activeSession && isNativeAgent && bridge\.available && bridge\.auxChat && \(/);
assert.match(codex, /<AuxChatPanel/);
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
