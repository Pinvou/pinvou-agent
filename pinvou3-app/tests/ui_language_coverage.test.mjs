import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js'; // full three-language dict: browser entry lazy-loads via i18n.js, tests use the aggregate shim

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');
// Shape pins must not match text inside comments: a guard commented out in
// the source would otherwise still satisfy every anchor. Round-23 should-fix
// 2 dropped full-line `//` comments (live-verified — commenting out the
// send-registry delete left this suite green); round-24 minor-1 closes the
// two remaining holes at this single strip point, both live-verified against
// that same delete: wrapping it in `/* … */`, and hiding it as a trailing
// `//` comment behind live code. Block-comment contents are blanked with
// newlines preserved (a multi-line comment cannot fuse its neighbours into
// an anchor match) and `//` comment tails are dropped per line. For POSITIVE
// pins over-stripping can only fail loudly, never satisfy one; the same does
// NOT hold for negative pins (round-26 minor M9): stripping inside a future
// string or template literal containing `/*` could silently satisfy a
// doesNotMatch anchor, so every negative pin below runs against the RAW
// text (a pattern absent in the raw text is absent in the strip too).
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');

// The aux-critical pins in these files run against comment-stripped source
// (round-25 should-fix 24-3b): the entry-parity pins on ChatView, the
// native-agent gates on CodexAcpView and the quote pins on
// ConversationTimeline are load-bearing guards, and `//`-commenting any of
// their lines used to pass the suite — the same historical bug class the
// round-23/24 strip fixes closed for AuxChatPanel, one file over.
const auxChatPanel = stripComments(source('features/aux-chat/AuxChatPanel.jsx'));
const chat = stripComments(source('features/chat/ChatView.jsx'));
const conversation = stripComments(source('features/conversation/ConversationTimeline.jsx'));
const codex = stripComments(source('features/codex/CodexAcpView.jsx'));
// Raw twins for the negative pins (round-26 minor M9, see the stripComments
// header): doesNotMatch anchors must see comments and string contents too.
const auxChatPanelRaw = source('features/aux-chat/AuxChatPanel.jsx');
const chatRaw = source('features/chat/ChatView.jsx');
const conversationRaw = source('features/conversation/ConversationTimeline.jsx');

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
    'uiAuxChat',
  ]) {
    assert.ok(dict[language][section], `${language}.${section} must exist`);
  }
  // All 22 uiAuxChat keys are pinned (round-14 minor-3: the list previously
  // covered 13, so sendingHint/bindingHint and the six quote* keys could be
  // deleted from every dictionary with the suite green — and quoteChipCount's
  // absence renders `undefined` at runtime; round-22 Major added discardStuck
  // for the discard settle-watchdog).
  for (const key of [
    'openLabel', 'panelTitle', 'landingHint', 'emptyState', 'inputPlaceholder',
    'send', 'busyHint', 'bindingHint', 'sendingHint', 'newTopic', 'newTopicConfirm',
    'sendFailed', 'ensureFailed', 'discardFailed', 'discardStuck', 'close',
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
assert.match(chat, /const chatCopy = t\.uiChat/);
assert.match(chat, /chatCopy\.asrDownloadTitle/);
assert.match(chat, /chatCopy\.memoryMeta/);
assert.match(chat, /chatCopy\.sceneModes/);
assert.match(chat, /chatViewCopy\.placeholderSceneAdjust/);
assert.match(chat, /chatViewCopy\.placeholderSceneDataViz/);
assert.match(chat, /chatViewCopy\.placeholderScenePoster/);
assert.doesNotMatch(chatRaw, /designGeneralPlaceholder/);
assert.doesNotMatch(chatRaw, /label:\s*'个人工作台'/);
assert.doesNotMatch(chatRaw, /label:\s*'公文写作'/);
assert.doesNotMatch(chatRaw, /label:\s*'数据可视化'/);
assert.doesNotMatch(chatRaw, /`取消\$\{scene\.label\}`/);
assert.doesNotMatch(chatRaw, /:\s*'描述你想生成或调整的内容'/);
assert.doesNotMatch(chatRaw, />下载语音识别模型</);
assert.match(chat, /data-testid="aux-chat-open"/);
// Work-mode entry parity with code mode (round-9 minor-2): the entry pill
// reflects the panel's real dock visibility via onActiveChange — no highlight
// while another dock panel occludes the aux panel.
assert.match(chat, /onActiveChange=\{setAuxChatDockActive\}/);
assert.match(chat, /auxChatPanel && auxChatDockActive/);
// Round-26 minor M4: ChatView's panel mount gate and dock-highlight reset
// must carry the same four conjuncts as the entry button (sched- exclusion,
// active session, bridge.available, bridge.auxChat) — CodexAcpView was
// aligned in round-25, ChatView lagged at two, leaving the gates without a
// test net even though they currently fail closed.
assert.match(
  chat,
  /\{auxChatPanel && activeSessionId && !activeSessionId\.startsWith\('sched-'\)\s*&& bridge\.available && bridge\.auxChat && \(/,
  'the ChatView aux panel mount gate must include the bridge conjuncts (round-26 minor M4)',
);
assert.match(
  chat,
  /if \(auxChatPanel && activeSessionId && !activeSessionId\.startsWith\('sched-'\)\s*&& bridge\.available && bridge\.auxChat\) return;/,
  'the ChatView aux dock-highlight reset must mirror the mount conjuncts (round-26 minor M4)',
);
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
// Raw twin for the negative pins below (round-26 minor M9).
const restartBlockRaw = auxChatPanelRaw.slice(
  auxChatPanelRaw.indexOf('const handleRestart'),
);
assert.match(restartBlock, /try \{\s*try \{[\s\S]*?const discardPromise = auxChat\.discard\(sessionId\);[\s\S]*?await discardPromise;[\s\S]*?\} catch[\s\S]*?setDiscardFailed\(true\);[\s\S]*?generationRef\.current !== generation\) return;\s*try \{\s*const nextAuxId = await withSettleBound\(auxChat\.ensure\(sessionId\)\)/);
assert.match(restartBlock, /setEnsureFailed\(true\)/);
assert.doesNotMatch(restartBlockRaw, /setSendFailed\(true\)/);
assert.match(auxChatPanel, /copy\.discardFailed/);
// Discard-failure restores the binding (round-20 Major-2): the round-18 B-1
// null leaves the panel send-dead while the discardFailed copy says the topic
// is still usable — the old aux session is alive after a failed discard, so
// the catch must re-ensure (idempotent → the same session) before returning,
// with the generation guard on both continuations. The restore is awaited
// (not a fire-and-forget promise chain) so the restarting latch outlives the
// rebind and pullSnapshot stays a direct reactive dependency of handleRestart.
assert.match(
  restartBlock,
  /\} catch \(error\) \{\s*console\.warn\('\[pinvou3\]\[aux-chat\] restart discard failed'[\s\S]{0,900}?setDiscardFailed\(true\);\s*try \{\s*const restoredAuxId = await withSettleBound\(auxChat\.ensure\(sessionId\)\);[\s\S]{0,300}?auxIdRef\.current = restoredAuxId;\s*setAuxId\(restoredAuxId\);\s*pullSnapshot\(restoredAuxId\);/,
  'the discard-failure catch must restore the nulled binding via an idempotent ensure',
);
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
assert.doesNotMatch(auxChatPanelRaw, /discardInFlightRef/);
assert.match(restartBlock, /discardInFlightByTask\.set\(sessionId, discardPromise\)/);
assert.match(restartBlock, /discardInFlightByTask\.delete\(sessionId\)/);
assert.match(auxChatPanel, /discardInFlightByTask\.get\(sessionId\)/);
// Duplicate-discard guard at restart entry (round-12 N1): a task switch resets
// `restarting` while the previous new-topic discard can still be parked in the
// backend turn gate. A second discard would then overwrite the registry entry,
// so nobody awaits the first one anymore — and on the web relay (invoke
// responses are not FIFO) that orphaned discard can land after this restart's
// recreate and delete the aux session the panel just bound to. The guard must
// run before the arm/confirm branch so an in-flight discard also cannot arm;
// the refusal un-arms the confirm so a click in this window is never a
// silent no-op (the rebind's binding hint is the visible in-progress state).
// Round-22 Major carve-out: a *stuck* entry (its settle-watchdog fired) no
// longer refuses — awaiting it forever was the dead end — so the guard reads
// the entry and checks the stuck set.
const duplicateDiscardGuard = restartBlock.indexOf('if (registeredDiscard && !discardStuckByTask.has(sessionId)) {');
assert.ok(duplicateDiscardGuard >= 0, 'handleRestart must refuse a second discard while a healthy one is in flight');
assert.match(restartBlock, /const registeredDiscard = discardInFlightByTask\.get\(sessionId\);\s*if \(registeredDiscard && !discardStuckByTask\.has\(sessionId\)\) \{/);
assert.ok(
  duplicateDiscardGuard < restartBlock.indexOf('if (!restartArmed) {'),
  'the duplicate-discard guard must precede the two-step confirm arming',
);
// Discard settle-watchdog (round-22 Major): the discard registry was the one
// in-flight registry with no never-settling recovery — round-16 B1 cleared the
// send registry at restart entry, but deleting a discard entry would re-open
// the N1 race, so a discard outliving DISCARD_WATCHDOG_MS (mirroring the web
// lane's 180 s invoke timeout in web/bootstrap.js) is marked stuck instead:
// the rebind effect stops awaiting it, the N1 guard above re-arms New Topic,
// and the panel surfaces the stuck state. The entry stays registered so the
// orphan's late settle keeps failing the identity checks.
assert.match(auxChatPanel, /const DISCARD_WATCHDOG_MS = 180_000;/);
assert.match(auxChatPanel, /const discardStuckByTask = new Set\(\);/);
assert.match(
  restartBlock,
  /discardInFlightByTask\.set\(sessionId, discardPromise\);\s*[\s\S]{0,400}?discardStuckByTask\.delete\(sessionId\);\s*[\s\S]{0,1400}?const watchdog = setTimeout\(\(\) => \{\s*[\s\S]{0,400}?if \(discardInFlightByTask\.get\(sessionId\) !== discardPromise\) return;\s*discardStuckByTask\.add\(sessionId\);\s*[\s\S]{0,400}?if \(sessionIdRef\.current !== sessionId\) return;\s*setDiscardStuck\(true\);\s*[\s\S]{0,400}?setRestarting\(false\);\s*setBindingPending\(false\);\s*\}, DISCARD_WATCHDOG_MS\);\s*try \{\s*await discardPromise;/,
  'a settle-watchdog must be armed between the registry set and the discard await, marking stuck and releasing the dead latches',
);
// The watchdog must gate on the live task mirror, not the generation (round-23
// MAJOR-3): an A→B→A round-trip re-awaits the still-pending discard under a
// fresh generation, and a generation gate would suppress the banner and leave
// that rebind's bindingPending uncleared — the eternal "preparing" state the
// watchdog exists to break.
assert.match(
  restartBlock,
  /discardStuckByTask\.add\(sessionId\);[\s\S]{0,400}?if \(sessionIdRef\.current !== sessionId\) return;\s*setDiscardStuck\(true\);/,
  'the watchdog must surface the stuck banner while the panel shows this task, keyed on sessionIdRef',
);
// The settle path must cancel the watchdog and clear entry + marker only by
// promise identity — a re-armed restart's fresh discard owns the slot and any
// marker then, so the orphaned discard's late settle must not touch either.
assert.match(
  restartBlock,
  /\} finally \{\s*clearTimeout\(watchdog\);[\s\S]{0,600}?const ownsEntry = discardInFlightByTask\.get\(sessionId\) === discardPromise;\s*if \(ownsEntry\) discardInFlightByTask\.delete\(sessionId\);\s*if \(ownsEntry && discardStuckByTask\.delete\(sessionId\)\) \{[\s\S]{0,400}?notifyTaskListeners\(discardStuckListenersByTask, sessionId\);/,
  'the discard settle path must clear watchdog, entry and stuck marker by promise identity and notify the removal to whichever instance is mounted',
);
// Stuck-state surfacing and re-arm: the rebind effect must mirror the module
// marker into state (the watchdog may fire while unmounted) and skip awaiting
// a stuck entry; the banner renders from the trilingual copy; the confirmed
// restart clears the banner at entry.
assert.match(auxChatPanel, /const \[discardStuck, setDiscardStuck\] = useState\(false\);/);
assert.match(auxChatPanel, /setDiscardStuck\(!!\(sessionId && discardStuckByTask\.has\(sessionId\)\)\);/);
assert.match(auxChatPanel, /copy\.discardStuck/);
assert.match(restartBlock, /setDiscardFailed\(false\);\s*[\s\S]{0,300}?setDiscardStuck\(false\);/);
// Stuck suppression (round-23 MAJOR-2): while a task's marker stands, the
// orphaned discard command can still execute server-side against the current
// mapping — the rebind effect must neither await nor ENSURE past it (binding
// would put a live transcript in front of the orphan), and handleSend must
// refuse while stuck (a message sent now could be destroyed with the session
// it lands in). The stuck branch drops the "preparing" hint instead of
// keeping it: the banner is the state the panel shows.
assert.match(
  auxChatPanel,
  /if \(discardStuckByTask\.has\(sessionId\)\) \{\s*setBindingPending\(false\);\s*\} else if \(pendingDiscard\) \{\s*pendingDiscard\.then\(ensureAfterDiscard, \(error\) => \{[\s\S]{0,400}?setDiscardFailed\(true\);\s*ensureAfterDiscard\(\);\s*\}\);\s*\} else \{\s*ensureAfterDiscard\(\);\s*\}/,
  'the rebind effect must skip ensure while a discard is stuck, clearing the preparing hint',
);
assert.match(
  auxChatPanel,
  /if \(sendInFlightByTask\.has\(sentTaskId\)\) return;\s*[\s\S]{0,200}?if \(discardStuckByTask\.has\(sentTaskId\)\) return;/,
  'handleSend must refuse while the task\'s discard is stuck',
);
// Send-latch release at restart entry (round-12 N2): handleSend releases the
// latch only when turn_started marks the snapshot busy (round-20 minor-4) or
// on its own failure path, and a same-task restart does not re-run the rebind
// effect that otherwise resets it. A send settling after the restart would
// therefore latch sendingRef true permanently and every later Enter would
// silently no-op behind a visually enabled composer.
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
assert.doesNotMatch(
  restartBlockRaw,
  /sendInFlightByTask\.delete\(sessionId\);/,
  'the send registry entry must be KEPT through the restart (round-25 MAJOR-24-3): the round-23 SEND_WATCHDOG_MS owns the never-settling recovery the round-16 B1 clear served, and a pending ack surviving the restart is what lets the failed-discard restore classify the staged draft',
);
const survivalClear = restartBlock.indexOf('restartDiscardFailedByTask.delete(sessionId);');
assert.ok(
  survivalClear >= 0,
  'restart entry must clear the stale failed-restart survival marker before issuing the fresh discard',
);
// Round-26 MAJOR-2: the kept-ack gate is cleared at restart entry alongside
// the survival marker — a kept-ack classification from an earlier restart
// window must not gate this window's failed-discard restore fixup, or
// recovery material whose delivery an earlier discard destroyed would be
// consumed by a restore that never contained it.
const keptAckClear = restartBlock.indexOf('restartWindowKeptAckByTask.delete(sessionId);');
assert.ok(
  keptAckClear > survivalClear
    && keptAckClear < restartBlock.indexOf('const discardPromise = auxChat.discard(sessionId);'),
  'restart entry must clear the stale kept-ack gate before issuing the fresh discard',
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
  bindingNull > survivalClear
    && bindingNull < restartBlock.indexOf('const discardPromise = auxChat.discard(sessionId);'),
  'the binding must be nulled at restart entry, before the discard await',
);
assert.match(restartBlock, /auxIdRef\.current = null;\s*setAuxId\(null\);/);
// Snapshot clear at restart entry (round-20 minor-5): restart does not re-run
// the rebind effect, so without this reset the old transcript stayed rendered
// through the whole discard window — hasContent stayed true, the
// bindingPending hint (rendered only in the !hasContent branch) was
// unreachable, and a stale busyHint persisted into the window. The clear must
// precede the discard await; the discard-failure restore re-pulls the
// snapshot, so a refused restart gets its transcript back.
const restartSnapshotClear = restartBlock.indexOf('setSnapshot(normalizeAuxSnapshot(null));');
assert.ok(
  restartSnapshotClear > bindingNull
    && restartSnapshotClear < restartBlock.indexOf('const discardPromise = auxChat.discard(sessionId);'),
  'handleRestart must clear the snapshot at entry, before the discard await',
);
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
  6,
  'bindingPending must clear on ensure success, ensure failure, the stuck skip, the restart finally, the round-22 settle-watchdog and the round-25 stuck-notify listener',
);
assert.match(auxChatPanel, /bindingPending \? copy\.bindingHint : copy\.emptyState/);
// In-flight send feedback (round-12 UX): the send window had no visible state
// because snapshot-busy only lands with the backend turn_started, so the
// composer looked idle while the message was already gone. Since round-20
// minor-4 the hint is derived from the task-keyed registry as well, so the
// cross-switch in-flight window (send on A → switch to B → back to A resets
// the component flag while A's send is still registered) does not silently
// no-op Enter behind an idle-looking composer.
assert.match(auxChatPanel, /const \[sending, setSending\] = useState\(false\);/);
assert.match(auxChatPanel, /sendingRef\.current = true;\s*setSending\(true\);/);
assert.match(auxChatPanel, /sendingRef\.current = false;\s*setSending\(false\);/);
assert.match(auxChatPanel, /const sendInFlight = sending \|\| !!\(sessionId && sendInFlightByTask\.has\(sessionId\)\);/);
assert.match(auxChatPanel, /\{sendInFlight && !busy && \(/);
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
  /if \(sentTaskId\) \{\s*consumeSentDraft\(sentTaskId, text\);\s*dropAuxQuotes\(sentTaskId, quotes\);\s*\}/,
  'the ack consumes the delivered draft through the notifying helper (round-25 MAJOR-24-2)',
);
assert.match(
  auxChatPanel,
  /const consumeSentDraft = \(taskId, text\) => \{\s*const storedDraft = draftByTask\.get\(taskId\);\s*if \(storedDraft !== undefined && storedDraft\.trim\(\) !== text\) return;\s*deleteDraftAndNotify\(taskId\);\s*\};/,
  'the consuming helper eats only the stored draft that still equals the sent text and broadcasts the deletion',
);
// Same-binding consumption regardless of generation (round-14 B3), widened
// in round-17 M-A: the success path consumes whenever the send settled into a
// live transcript — same binding (the composer shows this task) OR the panel
// moved to another task (A's aux is alive on its own binding). Only the
// restart case skips, keeping the draft as recovery — keyed since round-23
// MAJOR-4 on the restart epoch captured at dispatch, NOT on binding equality
// alone: a same-task rebind's transient null binding is not a restart, and
// the delivery there reached the still-live transcript the rebind re-ensures.
assert.match(
  auxChatPanel,
  /const sendPromise = auxChat\.send\(sentAuxId, quoteBlock \? text \+ quoteBlock : text\);[\s\S]{0,700}?await sendPromise;\s*\n[\s\S]{0,1600}?const sameBinding = auxIdRef\.current === sentAuxId;\s*const onSameTask = sessionIdRef\.current === sentTaskId;\s*if \(restartKeptDraft\(sentTaskId, sentEpoch\)\) \{\s*restartWindowKeptAckByTask\.add\(sentTaskId\);\s*return;\s*\}\s*if \(sentTaskId\) \{/,
  'consumption must follow the restart-only skip directly, gated on binding, live task identity and the restart epoch',
);
// The epoch must be captured at dispatch and bumped at restart entry before
// the binding null (round-23 MAJOR-4), module-scoped so it survives rebinds
// and remounts.
assert.match(auxChatPanel, /const restartEpochByTask = new Map\(\);/);
assert.match(auxChatPanel, /const sentEpoch = restartEpochByTask\.get\(sentTaskId\) \|\| 0;/);
assert.match(
  restartBlock,
  /generationRef\.current \+= 1;\s*const generation = generationRef\.current;[\s\S]{0,400}?restartEpochByTask\.set\(sessionId, \(restartEpochByTask\.get\(sessionId\) \|\| 0\) \+ 1\);[\s\S]{0,400}?try \{/,
  'the restart epoch must be bumped in the restart-entry block, before the discard is issued',
);
// The UI-touching part (visible draft clear, snapshot pull) stays
// binding-gated after the store-map consumption.
// The visible composer clear is task-gated, not binding-gated (round-24
// Major): a send ack settling inside the same-task rebind's ensure window
// reads a null binding, and the rebind's restore has just re-filled the
// composer from draftByTask — a binding gate there skipped the only composer
// clear and left delivered text staged for a duplicate Enter. Only the
// snapshot pull stays binding-gated after it.
assert.match(
  auxChatPanel,
  /dropAuxQuotes\(sentTaskId, quotes\);\s*\}\s*if \(!onSameTask\) return;\s*setDraft\(\(current\) => clearedIfSent\(current, text\)\);\s*if \(sameBinding\) pullSnapshot\(auxIdRef\.current\);/,
  'the composer clear must be task-gated (round-24 Major), only the snapshot pull binding-gated',
);
assert.match(auxChatPanel, /const sessionIdRef = useRef\(sessionId\);/);
// The aux composer caps input like the main one (round-20 minor-7): drafts
// persist per task, so an unbounded paste would live in memory indefinitely.
assert.match(auxChatPanel, /constrainChatInput\(event\.target\.value\)\.text/);
assert.match(auxChatPanel, /setDraft\(\(current\) => clearedIfSent\(current, text\)\)/);
assert.match(auxChatPanel, /const clearedIfSent = \(current, text\) => \(current\.trim\(\) === text \? '' : current\);/);
assert.doesNotMatch(restartBlockRaw, /setDraft\(''\)/);
// Aux timeline scroll (round-20 minor-6): mirror the main conversation's
// autoScrollRef pattern — a scroll listener derives the follow flag through
// the shared transition helper (scrolling up parks it, returning near the
// bottom resumes), a rebind re-arms it at the tail, and content growth snaps
// only while following. The old unconditional snap on item-count change
// yanked a scrolled-up reader, and streaming deltas (same count) never stuck
// to the bottom; depending on the snapshot covers both turns and deltas.
assert.match(auxChatPanel, /const autoScrollRef = useRef\(true\);/);
assert.match(auxChatPanel, /transitionConversationScrollState\(\{/);
assert.match(auxChatPanel, /autoScrollRef\.current = transition\.following;/);
assert.match(auxChatPanel, /autoScrollRef\.current = true;\s*\}, \[auxId\]\);/);
assert.match(auxChatPanel, /if \(el && autoScrollRef\.current\) el\.scrollTop = el\.scrollHeight;\s*\}, \[auxId, snapshot\]\);/);
// Conversation quotes ("划词引用"): staged per task through the aux-quote store
// (module scope, same ownership as the draft) and appended to the outgoing
// message as an inline userselect block; a quote-only send is allowed.
assert.match(auxChatPanel, /const quoteBlock = buildAuxQuoteBlock\(quotes\);/);
assert.match(auxChatPanel, /const sendPromise = auxChat\.send\(sentAuxId, quoteBlock \? text \+ quoteBlock : text\);/);
assert.match(auxChatPanel, /!hasSendContent\(text, quoteBlock\)/);
assert.match(auxChatPanel, /subscribeAuxQuotes\(sessionId/);
assert.match(auxChatPanel, /data-testid="aux-quote-chips"/);
assert.match(auxChatPanel, /data-testid="aux-quote-remove"/);
// Quote-only send affordance: with staged quotes the composer stays usable
// even while the draft is empty (E2E scenario 7 covers it, but that does not
// run in CI).
assert.match(auxChatPanel, /disabled=\{composerDisabled \|\| \(!draft\.trim\(\) && quotes\.length === 0\)\}/);
// Stale send outcomes (round-12 UX): a restart on the same task re-binds to a
// new aux, so a send issued before it must neither re-latch sendFailed next to
// ensureFailed ("double banner") nor clear text typed since. Since round-24
// (minors 8-9) the failure banner is gated on registry identity, the displayed
// task and the restart epoch — NOT on the generation: the generation gate also
// silenced a genuine failure surfacing on the A→B→A round trip, and the
// un-guarded path let a late rejection after the watchdog fired release a
// newer send's latch.
assert.match(auxChatPanel, /const sentTaskId = sessionId;/);
assert.match(
  auxChatPanel,
  /if \(sendInFlightByTask\.get\(sentTaskId\) !== sendPromise\s*\|\| \(restartEpochByTask\.get\(sentTaskId\) \|\| 0\) !== sentEpoch\s*\|\| sessionIdRef\.current !== sentTaskId\) return;\s*setSendFailed\(true\);/,
  'the failure banner must be gated on registry identity (round-24 minor-9), the displayed task and the restart epoch (round-24 minor-8)',
);
// The latch release on the failure path stays scoped to the binding that
// still owns it (round-14 B2): inside the rebind window the rebind effect
// already reset the latch, so the release is skipped there.
assert.match(auxChatPanel, /setSendFailed\(true\);[\s\S]{0,600}?if \(auxIdRef\.current === sentAuxId\) \{\s*sendingRef\.current = false;\s*setSending\(false\);\s*\}\s*\} finally \{/);
// restarting leak guard: the normal flow has exactly one setRestarting(false),
// located in the outer finally (whose try opens before the discard await and
// whose finally closes after the ensure await) — every early-return path
// resets through it. The reset must be generation-gated (round-8 m2): a stale
// continuation must not clear a newer restart's latch. The binding-pending
// hint rides the same gate (round-12 UX): a stale continuation must not clear
// a newer restart's pending state either. The second occurrence is the
// round-22 settle-watchdog: it releases the dead latch (identity- and
// generation-gated inside the timer callback) so the re-armed New Topic it
// grants is not stuck behind a disabled button.
const restartingClears = restartBlock.match(/setRestarting\(false\)/g) || [];
assert.equal(restartingClears.length, 2, 'restarting must be cleared only in the outer finally and the settle-watchdog');
const outerTry = restartBlock.indexOf('try {');
const discardAwait = restartBlock.indexOf('await discardPromise;');
const ensureAwait = restartBlock.indexOf('await withSettleBound(auxChat.ensure');
const restartingClearIdx = restartBlock.lastIndexOf('setRestarting(false)');
const finallyClause = restartBlock.lastIndexOf('} finally {', restartingClearIdx);
assert.ok(
  outerTry >= 0 && outerTry < discardAwait && discardAwait < ensureAwait && ensureAwait < finallyClause,
  'a single outer try must span discard+ensure so its finally resets restarting on every early return',
);
assert.match(restartBlock.slice(finallyClause), /} finally \{[\s\S]*?if \(generationRef\.current === generation\) \{\s*setRestarting\(false\);\s*setBindingPending\(false\);\s*\}/);
// In-flight send latch (round-7 M-A): snapshot-busy lags the dispatch by one
// event round trip, so without a synchronous latch a double Enter fires a
// duplicate turn whose rejection surfaces as a bogus "send failed" banner;
// key-repeat Enter must be ignored outright. Since round-20 minor-4 the latch
// is NOT released in the send's finally: that resolve is only the dispatch
// ack and turn_started still lags it, so releasing there re-opened the
// duplicate-send window exactly where the latch claims coverage — fresh
// input died at the backend turn gate as a misleading "send failed, retry"
// banner. The latch releases when turn_started marks the snapshot busy; the
// failure path releases it directly (a failed dispatch never reaches
// turn_started, and a retry must stay possible).
assert.match(auxChatPanel, /if \(!auxChat \|\| !sentAuxId \|\| !hasSendContent\(text, quoteBlock\) \|\| busy \|\| restarting \|\| sendingRef\.current\) return;/);
assert.match(auxChatPanel, /sendingRef\.current = true;\s*setSending\(true\);[\s\S]*?const sendPromise = auxChat\.send\(sentAuxId, quoteBlock \? text \+ quoteBlock : text\);[\s\S]*?await sendPromise;/);
assert.match(auxChatPanel, /if \(!sending \|\| !busy\) return;\s*sendingRef\.current = false;/);
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
// Send-finally scope guard (round-9 minor-3, rescoped in round-20 minor-4):
// the send's finally now only removes the registry entry — it must NOT touch
// the latch, because the invoke's resolve is merely the dispatch ack while
// turn_started still lags one event round trip. The latch release lives in
// the busy-gated effect (success), the failure path of the same send
// (round-14 B2: only the exact send on the binding that still owns it), and
// the rebind/restart resets.
const sendFinallyStart = auxChatPanel.indexOf('} finally {', auxChatPanel.indexOf('const handleSend'));
const sendFinallyEnd = auxChatPanel.indexOf('}, [auxChat, draft, quotes, busy, restarting, pullSnapshot, sessionId]);');
assert.ok(
  sendFinallyStart >= 0 && sendFinallyEnd > sendFinallyStart,
  'handleSend finally anchors must resolve (a vacuous slice would pass trivially)',
);
const sendFinallyBlock = auxChatPanel.slice(sendFinallyStart, sendFinallyEnd);
// Raw twin for the negative pins below (round-26 minor M9).
const sendFinallyBlockRaw = auxChatPanelRaw.slice(
  auxChatPanelRaw.indexOf('} finally {', auxChatPanelRaw.indexOf('const handleSend')),
  auxChatPanelRaw.indexOf('}, [auxChat, draft, quotes, busy, restarting, pullSnapshot, sessionId]);'),
);
assert.match(sendFinallyBlock, /removeSendIfOwner\(sentTaskId, sendPromise\);/);
assert.doesNotMatch(sendFinallyBlockRaw, /sendingRef\.current = false;/);
assert.doesNotMatch(sendFinallyBlockRaw, /setSending\(false\);/);
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
assert.match(auxChatPanel, /finally \{[\s\S]{0,700}?removeSendIfOwner\(sentTaskId, sendPromise\);/);
assert.match(auxChatPanel, /const removeSendIfOwner = \(taskId, sendPromise\) => \{\s*if \(sendInFlightByTask\.get\(taskId\) === sendPromise\) sendInFlightByTask\.delete\(taskId\);\s*\};/);
// Send-latch failsafe (round-23 should-fix 1): the busy-gated release only
// fires if a render observes busy=true — turn_started and the turn-terminal
// events coalescing into one render batch (fast-failing turns, relay bursts)
// never do, and the latch and the registry entry would stick with no
// recovery. A dispatch outliving SEND_WATCHDOG_MS (the web lane's invoke
// timeout, same bound as the discard watchdog) releases both, gated on
// registry-entry identity so a settled or replaced send is never touched.
assert.match(auxChatPanel, /const SEND_WATCHDOG_MS = 180_000;/);
assert.match(
  auxChatPanel,
  /const armSendWatchdog = \(taskId, sendPromise, onFailsafe\) => setTimeout\(\(\) => \{\s*if \(sendInFlightByTask\.get\(taskId\) !== sendPromise\) return;\s*sendInFlightByTask\.delete\(taskId\);\s*onFailsafe\(\);\s*\}, SEND_WATCHDOG_MS\);/,
  'the send watchdog must release the registry entry by promise identity and hand the latch release to the owner',
);
assert.match(
  auxChatPanel,
  /const sendWatchdog = armSendWatchdog\(sentTaskId, sendPromise, \(\) => \{\s*sendingRef\.current = false;\s*setSending\(false\);\s*\}\);/,
);
assert.match(
  auxChatPanel,
  /\} finally \{\s*clearTimeout\(sendWatchdog\);/,
  'the send watchdog must be cancelled when the send settles',
);
// Double-banner guard (round-9 minor-1): entering the restart flow must
// clear a stale sendFailed too, or a failed send's "retry" banner renders
// next to the ensure-failure banner after the binding was cleared. The
// round-22 discardStuck banner clears on the same entry (the confirmed
// restart is the recovery it asks for).
assert.match(restartBlock, /setRestarting\(true\);\s*setDiscardFailed\(false\);[\s\S]{0,300}setDiscardStuck\(false\);[\s\S]{0,500}setSendFailed\(false\);/);
// Round-25 (fresh re-review): the ack-consumption boundary must consult only
// module state, and transitions that run on a dead instance must reach the
// mounted one.
// MAJOR-24-1: the epoch is module-scoped and consulted unconditionally — the
// per-instance sameBinding/onSameTask refs freeze on close/reopen and a
// frozen auxIdRef used to short-circuit the skip, deleting a restart's
// preserved recovery draft.
assert.match(
  auxChatPanel,
  /const restartKeptDraft = \(taskId, sentEpoch\) => \{\s*if \(\(restartEpochByTask\.get\(taskId\) \|\| 0\) === sentEpoch\) return false;\s*return !restartDiscardFailedByTask\.delete\(taskId\);\s*\};/,
  'the keep-draft decision must consult module state only (epoch first, single-shot failed-restart survival marker falling through)',
);
assert.match(
  auxChatPanel,
  /const onSameTask = sessionIdRef\.current === sentTaskId;\s*if \(restartKeptDraft\(sentTaskId, sentEpoch\)\) \{\s*restartWindowKeptAckByTask\.add\(sentTaskId\);\s*return;\s*\}/,
  'the ack must call the module-state keep-draft decision unconditionally (round-25 MAJOR-24-1) and mark the window that kept a delivered ack (round-26 MAJOR-2)',
);
// MAJOR-24-3: the survival marker is set by the failed-discard restore
// (generation-gated), and the restore consumes the delivered draft itself
// when no ack is pending — precisely, only while the stored draft still
// equals the recorded sent text.
assert.match(
  restartBlock,
  /pullSnapshot\(restoredAuxId\);[\s\S]{0,300}?restartDiscardFailedByTask\.add\(sessionId\);\s*consumeDeliveredDraftAfterFailedRestart\(sessionId\);/,
  'the failed-discard restore must mark survival and run the delivered-draft fixup',
);
assert.match(
  auxChatPanel,
  /const consumeDeliveredDraftAfterFailedRestart = \(sessionId\) => \{\s*if \(sendInFlightByTask\.has\(sessionId\)\) return;\s*if \(!restartWindowKeptAckByTask\.delete\(sessionId\)\) return;\s*const sentQuotes = sentQuotesByTask\.get\(sessionId\);\s*if \(sentQuotes\) dropAuxQuotes\(sessionId, sentQuotes\);\s*const sentText = sentTextByTask\.get\(sessionId\);\s*const storedDraft = draftByTask\.get\(sessionId\);\s*if \(sentText !== undefined && storedDraft !== undefined\s*&& storedDraft\.trim\(\) === sentText\.trim\(\)\) \{\s*deleteDraftAndNotify\(sessionId\);\s*\}/,
  'the fixup consumes the delivered draft precisely (stored still equals the recorded sent text) only when no ack is pending AND this restart window kept a delivered ack (round-26 MAJOR-2), and drops exactly the delivered send\'s captured quotes (round-26 minor M2)',
);
// The sent text is recorded at dispatch (for the restore fixup) and cleared
// on failure (a failed dispatch delivered nothing).
assert.match(auxChatPanel, /sendInFlightByTask\.set\(sentTaskId, sendPromise\);\s*[\s\S]{0,900}?sentTextByTask\.set\(sentTaskId, text\);\s*sentQuotesByTask\.set\(sentTaskId, quotes\);/);
assert.match(auxChatPanel, /console\.warn\('\[pinvou3\]\[aux-chat\] send failed', error\);\s*[\s\S]{0,300}?sentTextByTask\.delete\(sentTaskId\);/);
// Round-26 minor M1: the failure-path delete runs before the registry-
// identity gate, so a stale rejection (watchdog released the entry, a newer
// send already recorded its own text) must not erase the newer send's
// record — only delete while the entry still records THIS send's text.
assert.match(
  auxChatPanel,
  /if \(sentTextByTask\.get\(sentTaskId\) === text\) sentTextByTask\.delete\(sentTaskId\);/,
  'a stale send rejection must not delete a newer send\'s recorded text (round-26 minor M1)',
);
// Round-26 minor M2 twin guard: the quote capture clears by reference
// identity — a newer send captured a fresh array.
assert.match(
  auxChatPanel,
  /if \(sentQuotesByTask\.get\(sentTaskId\) === quotes\) sentQuotesByTask\.delete\(sentTaskId\);/,
  'a stale send rejection must not delete a newer send\'s quote capture (round-26 minor M2)',
);
// Round-26 MAJOR-2: the kept-ack gate is module-scoped state alongside the
// other registries.
assert.match(
  auxChatPanel,
  /const restartWindowKeptAckByTask = new Set\(\);/,
  'the kept-ack gate must be module-scoped so it survives rebinds and remounts',
);
// MAJOR-24-2: the ack's store-map consumption notifies the mounted panel, and
// the panel follows draft-store deletions (the composer was restored from the
// consumed entry).
assert.match(
  auxChatPanel,
  /const deleteDraftAndNotify = \(taskId\) => \{\s*draftByTask\.delete\(taskId\);\s*notifyTaskListeners\(draftDeleteListenersByTask, taskId\);\s*\};/,
  'a store-map deletion must notify the mounted panel (the ack may settle on a dead instance)',
);
assert.match(
  auxChatPanel,
  /subscribeTaskListeners\(draftDeleteListenersByTask, sessionId, \(\) => \{\s*if \(sessionIdRef\.current !== sessionId\) return;\s*[\s\S]{0,200}?setDraft\(''\);/,
  'the panel must follow draft-store deletions with a live-task gate',
);
// Should-fix-24-2: stuck-marker transitions notify the mounted panel — the
// watchdog can fire, or a pending discard settle, behind a close/reopen.
assert.match(auxChatPanel, /discardStuckByTask\.add\(sessionId\);\s*[\s\S]{0,400}?notifyTaskListeners\(discardStuckListenersByTask, sessionId\);/);
assert.match(
  auxChatPanel,
  /discardStuckByTask\.delete\(sessionId\)\) \{\s*[\s\S]{0,400}?notifyTaskListeners\(discardStuckListenersByTask, sessionId\);/,
  'the settle path must notify the marker removal so whichever instance is mounted re-mirrors it with the live-task gate',
);
assert.match(
  auxChatPanel,
  /subscribeTaskListeners\(discardStuckListenersByTask, sessionId, \(\) => \{\s*if \(sessionIdRef\.current !== sessionId\) return;\s*const stuck = discardStuckByTask\.has\(sessionId\);\s*[\s\S]{0,300}?setDiscardStuck\(stuck\);\s*if \(stuck\) \{\s*setRestarting\(false\);\s*setBindingPending\(false\);/,
  'the stuck-notify listener must re-mirror the marker and apply the watchdog latch releases',
);
// Round-25 minor: both restart-stage ensures carry the same settle bound as
// the registries; a hung ensure must not latch restarting forever.
assert.match(auxChatPanel, /const ENSURE_WATCHDOG_MS = 180_000;/);
assert.equal(
  (auxChatPanel.match(/await withSettleBound\(auxChat\.ensure\(sessionId\)\)/g) || []).length,
  2,
  'both restart-stage ensures (failed-discard restore and recreate) must be settle-bound',
);
// Round-25 minor: a discard rejection awaited across a task round trip must
// surface discardFailed on the rebind instead of silently re-binding the old
// transcript (the restart catch is generation-gated and can no longer see it).
assert.match(
  auxChatPanel,
  /pendingDiscard\.then\(ensureAfterDiscard, \(error\) => \{[\s\S]{0,400}?setDiscardFailed\(true\);\s*ensureAfterDiscard\(\);\s*\}\);/,
  'the rebind must surface an awaited-discard rejection instead of silently keeping the old transcript',
);
assert.match(source('features/pet/PetSettingsSection.jsx'), /t\.uiPetSettings/);
assert.match(conversation, /conversationCopy\(copy\)/);
assert.doesNotMatch(conversationRaw, />等待授权</);
// Aux quote chips ("划词引用") in the user bubble: the aux projection strips
// the inline userselect block from userText and hands the excerpts over as
// userQuotes, so this render branch is the only place the quoted content is
// still visible. If it regresses, quotes disappear from the transcript
// silently — the raw block is already stripped and no raw text remains.
assert.match(conversation, /const userQuotes = Array\.isArray\(turn\.userQuotes\) \? turn\.userQuotes : \[\];/);
assert.match(conversation, /turn\.userText \|\| userAttachments\.length \|\| userQuotes\.length/);
assert.match(conversation, /userQuotes\.map\(\(quote, index\)/);
assert.match(conversation, /data-testid="conversation-user-quote"/);
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
// The same gate must hold on the two paths that were reachable without it
// (fixed in the round-19 hardening): the quote-selection feed (a null
// sessionId suppresses the popover entirely) and the panel mount (switching
// to an external-ACP agent must unmount the open panel, not rebind it).
assert.match(codex, /sessionId=\{activeSession && isNativeAgent && bridge\.available && bridge\.auxChat \? activeSession\.id : null\}/);
assert.match(codex, /\{auxChatPanel && activeSession && isNativeAgent && bridge\.available && bridge\.auxChat && \(/);
// The dock-highlight reset effect must gate on the SAME condition (round-25
// minor consistency note): a gate drift would leave the highlight latched
// when the panel unmounts through the bridge conjuncts.
assert.match(codex, /if \(auxChatPanel && activeSession && isNativeAgent && bridge\.available && bridge\.auxChat\) return;/);
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
