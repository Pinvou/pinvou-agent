/**
 * 普通聊天「绑定工作目录会话」逻辑测试（安全姿态对齐 code 模式）：
 *   - 纯逻辑：chatYoloGateApplies（确认门适用对象）/
 *     shouldShowWorkspaceBindingChip（绑定指示显示条件）；
 *   - sessions bridge：getSessionWorkspaceBinding 的归一化与失败兜底；
 *     ensureSession 物化时绑定草稿不再套用 work lane 默认、按暂存 mode 应用；
 *     setDraftWorkspace 绑定/解绑刷新草稿 mode 显示并作废暂存；
 *   - interaction bridge：绑定草稿显式切换写 code lane 全局默认并暂存选择，
 *     未绑定草稿维持 work/design lane 语义。
 * harness 复刻 chat_draft_workspace_logic.test.mjs 的 vm 注入面。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

const {
  chatYoloGateApplies,
  shouldShowWorkspaceBindingChip,
} = await import('../src/features/chat/chat-workspace-binding.js');
const { needsYoloConfirmation } = await import('../src/features/codex/code-permission-state.js');

// ── 纯逻辑：确认门适用对象 ─────────────────────────────────────
test('chatYoloGateApplies：已生成会话看目录绑定，草稿看 draftWorkspacePath', () => {
  assert.equal(chatYoloGateApplies({ activeSessionId: 's1', sessionBinding: '/work/p', draftWorkspacePath: null }), true);
  assert.equal(chatYoloGateApplies({ activeSessionId: 's1', sessionBinding: null, draftWorkspacePath: '/work/p' }), false,
    '活动会话未绑定时不因残留草稿选择误触发');
  assert.equal(chatYoloGateApplies({ activeSessionId: null, sessionBinding: null, draftWorkspacePath: '/work/p' }), true);
  assert.equal(chatYoloGateApplies({ activeSessionId: null, sessionBinding: null, draftWorkspacePath: null }), false);
});

test('确认门判定复用 needsYoloConfirmation：未确认弹卡、已确认直切、读取失败按未确认', () => {
  assert.equal(needsYoloConfirmation(null), true, 'prefs 读取失败按未确认（安全方向）');
  assert.equal(needsYoloConfirmation({ yolo_confirmed: false }), true);
  assert.equal(needsYoloConfirmation({ yolo_confirmed: true }), false);
});

// ── 纯逻辑：绑定指示显示条件 ──────────────────────────────────
test('shouldShowWorkspaceBindingChip：仅活动会话且有绑定路径时显示', () => {
  assert.equal(shouldShowWorkspaceBindingChip({ activeSessionId: 's1', sessionBinding: '/work/p' }), true);
  assert.equal(shouldShowWorkspaceBindingChip({ activeSessionId: null, sessionBinding: '/work/p' }), false,
    '草稿态由 ComposerWorkspaceSelector 呈现，不显示只读 chip');
  assert.equal(shouldShowWorkspaceBindingChip({ activeSessionId: 's1', sessionBinding: null }), false,
    '查询失败/无绑定/Web 桩 null 不显示');
});

// ── vm harness ────────────────────────────────────────────────
function loadFeature(name, contextOverrides, stateOverrides) {
  const root = { __PINVOU_SHARED_I18N__: {} };
  const storage = new Map();
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  };
  const src = fs.readFileSync(path.join(bridgeDir, name + '.js'), 'utf8');
  vm.runInNewContext(src, { window: root, globalThis: root, localStorage, setTimeout, clearTimeout });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__[name];
  const state = Object.assign({
    activeSessionId: null,
    messages: [], chatItems: [], artifacts: [], queued: [],
    sessions: [], archivedSessions: [],
    scheduledTaskRecentRuns: [], scheduledTaskRuns: [],
    modeState: { mode: 'yolo', multiAgent: false },
    modeDefaults: { work: 'yolo', design: 'yolo', code: null },
    modeLane: 'work',
    draftEpoch: 0,
    composerDraft: '',
    pendingDraftMultiAgent: false,
    pendingDraftMode: null,
    draftWorkspacePath: null,
    scheduledRunContext: null,
    scheduledTaskPendingGuide: null,
    mountedCollections: [],
    mountedCollectionsRevision: 0,
    busy: false,
    thinking: false,
    tokens: { input: 0, max: 0 },
    turnTimeline: [],
    activeTurnTimelineId: null,
    personaEvents: [],
    pinvouReviews: [],
    pinvouSceneEvents: [],
    scheduledTaskDraft: null,
  }, stateOverrides || {});
  const calls = { invoke: [], notify: 0 };
  // 自定义 invoke 应答器从覆盖面中取出单独接线：默认 invoke 负责记录调用，
  // 覆盖的应答器只做返回编排，否则调用证据会被整体替换掉。
  const overrides = Object.assign({}, contextOverrides || {});
  const invokeResponder = overrides.invoke || null;
  delete overrides.invoke;
  const api = factory(Object.assign({
    state,
    sessionStates: {},
    notify() { calls.notify += 1; },
    listen: null,
    bt(key) { return key; },
    addSystemItem(text) { state.chatItems.push({ type: 'system', text, id: 'sys-' + (state.chatItems.length + 1) }); },
    addChatItem(item) { item.id = item.id || ('item-' + (state.chatItems.length + 1)); state.chatItems.push(item); },
    addAuthoritySyncNotice() {},
    timeStr() { return ''; },
    invoke(name, args) {
      calls.invoke.push([name, args]);
      if (invokeResponder) return invokeResponder(name, args);
      if (name === 'create_session') return Promise.resolve({ id: 'chat-new' });
      if (name === 'list_sessions' || name === 'list_archived_sessions') return Promise.resolve([]);
      if (name === 'set_mode_default') return Promise.resolve(state.modeDefaults);
      return Promise.resolve({ mode: 'yolo', multi_agent: false });
    },
    runSyncOnSession(sid, fn) { fn(); },
    persistMessagesFor() {},
    resetPendingAssistant() {},
    stopThinking() {},
    rerenderFromMessages() {},
    syncModeState() { return Promise.resolve(); },
    // 与 bridge.js 同构的草稿态显示解析：绑定草稿 → code lane（缺省 plan），
    // 否则当前 lane 全局默认（缺省 yolo）。
    currentDraftModeState() {
      const boundDraft = !!state.draftWorkspacePath;
      const lane = boundDraft ? 'code' : (state.modeLane === 'design' ? 'design' : 'work');
      const d = state.modeDefaults && state.modeDefaults[lane];
      return { mode: d || (boundDraft ? 'plan' : 'yolo'), multiAgent: false };
    },
    applyAuthoritativeModeState(sid, st) {
      state.modeState = { mode: st.mode || 'yolo', multiAgent: !!st.multi_agent };
    },
    modeStateEpochs: {},
    bumpModeStateEpoch() {},
    syncActivePersona() { return Promise.resolve(); },
    syncMountedCollection() { return Promise.resolve(); },
    reconcileArtifacts() {},
    loadSessionModel() { return Promise.resolve(); },
    clearScheduledTaskSelection() {},
    invalidateScheduledRecentRunsForSession() {},
    turnUsageDirty: {},
    basename(p) { return String(p || '').split('/').pop(); },
    isAbsPath() { return false; },
    filterSessionArtifacts(list) { return list; },
    scheduleShellPoll() {},
    setScheduledTaskError() {},
    userMessageDisplayText(t) { return t; },
    loadMemoryOverview() { return Promise.resolve(); },
    isScheduledRunSession(id) { return String(id || '').indexOf('sched-') === 0; },
    invalidateScheduledTaskReads() {},
    applyScheduledRunViewed() {},
    loadScheduledTaskRecentRuns() { return Promise.resolve(); },
    scheduledRunSessionOwners: {},
    personaPlaceholderTitles: {},
    getBuffer() { return null; },
    flushAssistantMessageToHistory() {},
    ensureSession: async () => (state.activeSessionId || 'chat-a'),
    sendMessage: async () => {},
    reconcileRemoteTurn: async () => true,
    isBusyFor() { return false; },
    markRemoteTurn() {},
  }, overrides));
  return {
    api, state, calls,
    invokeNames() { return calls.invoke.map(call => call[0]); },
    invokeArgs(name) {
      // invoke 入参对象产自 vm realm，JSON 归一化后再比较（跨域原型不等）。
      return calls.invoke.filter(call => call[0] === name)
        .map(call => JSON.parse(JSON.stringify(call[1])));
    },
  };
}

// ── getSessionWorkspaceBinding：归一化与失败兜底 ──────────────────
test('getSessionWorkspaceBinding：绑定路径透传，空串/非字符串按 null', async () => {
  const rt = loadFeature('sessions', {
    invoke(name) {
      if (name === 'get_session_workspace_binding') return Promise.resolve('D:\\work\\proj');
      return Promise.resolve(null);
    },
  });
  assert.equal(await rt.api.getSessionWorkspaceBinding('chat-a'), 'D:\\work\\proj');
  assert.deepEqual(rt.invokeArgs('get_session_workspace_binding'), [{ sessionId: 'chat-a' }]);

  const rtEmpty = loadFeature('sessions', {
    invoke() { return Promise.resolve(''); },
  });
  assert.equal(await rtEmpty.api.getSessionWorkspaceBinding('chat-a'), null);
});

test('getSessionWorkspaceBinding：无 sessionId / 查询失败（Web、旧后端无此命令）按 null', async () => {
  const rt = loadFeature('sessions', {
    invoke(name) {
      if (name === 'get_session_workspace_binding') return Promise.reject(new Error('unknown command'));
      return Promise.resolve(null);
    },
  });
  assert.equal(await rt.api.getSessionWorkspaceBinding(null), null);
  assert.equal(await rt.api.getSessionWorkspaceBinding('chat-a'), null, '查询失败不得上抛，UI 按无绑定处理');
});

// ── setDraftWorkspace：绑定/解绑即刷新草稿 mode 显示 ──────────────────
test('setDraftWorkspace：绑定后草稿 mode 显示切 code lane（无记录缺省 plan）', () => {
  const rt = loadFeature('sessions');
  rt.api.setDraftWorkspace('/work/project');
  assert.equal(rt.state.modeState.mode, 'plan', '绑定草稿显示 code lane 默认，无记录 → plan');
  rt.state.modeDefaults = { work: 'yolo', design: 'yolo', code: 'yolo' };
  rt.api.setDraftWorkspace('/work/other');
  assert.equal(rt.state.modeState.mode, 'yolo', 'code lane 有记录时跟随记录');
});

test('setDraftWorkspace：解绑回本 lane 默认并作废显式 mode 暂存', () => {
  const rt = loadFeature('sessions');
  rt.api.setDraftWorkspace('/work/project');
  rt.state.pendingDraftMode = 'yolo'; // 模拟绑定草稿上显式暂存过
  rt.api.setDraftWorkspace(null);
  assert.equal(rt.state.pendingDraftMode, null, '解绑不得把暂存带入未绑定草稿');
  assert.equal(rt.state.modeState.mode, 'yolo', '未绑定草稿回 work lane 默认（缺省 yolo）');
});

test('enterDraft：作废绑定草稿的显式 mode 暂存', () => {
  const rt = loadFeature('sessions');
  rt.state.pendingDraftMode = 'plan';
  rt.api.enterDraft();
  assert.equal(rt.state.pendingDraftMode, null);
});

// ── ensureSession：绑定草稿的 lane 默认应用 ──────────────────
test('ensureSession：绑定草稿不套用 work lane 默认（后端按 code lane 解析）', async () => {
  const rt = loadFeature('sessions');
  rt.state.modeDefaults = { work: 'plan', design: null, code: null }; // work lane 默认 plan
  rt.api.setDraftWorkspace('/work/project');
  const id = await rt.api.ensureSession();
  assert.equal(id, 'chat-new');
  assert.deepEqual(rt.invokeArgs('create_session'), [{ workspacePath: '/work/project' }]);
  assert.ok(!rt.invokeNames().includes('set_plan_mode_next'),
    '绑定会话不得把 work lane 默认经 set_plan_mode_next 套用');
});

test('ensureSession：未绑定草稿维持 work lane 默认应用（回归保护）', async () => {
  const rt = loadFeature('sessions');
  rt.state.modeDefaults = { work: 'plan', design: null, code: null };
  const id = await rt.api.ensureSession();
  assert.equal(id, 'chat-new');
  assert.deepEqual(rt.invokeArgs('set_plan_mode_next'), [{ sessionId: 'chat-new' }]);
});

test('ensureSession：绑定草稿暂存 plan → 物化时 set_plan_mode_next；暂存 yolo → exit_plan_to_yolo', async () => {
  const rtPlan = loadFeature('sessions');
  rtPlan.api.setDraftWorkspace('/work/project');
  rtPlan.state.pendingDraftMode = 'plan';
  await rtPlan.api.ensureSession();
  assert.deepEqual(rtPlan.invokeArgs('set_plan_mode_next'), [{ sessionId: 'chat-new' }]);
  assert.ok(!rtPlan.invokeNames().includes('exit_plan_to_yolo'));
  assert.equal(rtPlan.state.pendingDraftMode, null, '物化后暂存必须清空');

  const rtYolo = loadFeature('sessions');
  rtYolo.api.setDraftWorkspace('/work/project');
  rtYolo.state.pendingDraftMode = 'yolo';
  await rtYolo.api.ensureSession();
  assert.deepEqual(rtYolo.invokeArgs('exit_plan_to_yolo'), [{ sessionId: 'chat-new' }]);
  assert.ok(!rtYolo.invokeNames().includes('set_plan_mode_next'));
});

// ── setDraftMode：绑定草稿写 code lane 并暂存 ──────────────────
test('setDraftMode：绑定草稿显式切换写 code lane 全局默认并暂存选择', async () => {
  const rt = loadFeature('interaction');
  rt.state.draftWorkspacePath = '/work/project';
  await rt.api.setDraftMode('plan');
  assert.deepEqual(rt.invokeArgs('set_mode_default'), [{ lane: 'code', mode: 'plan' }],
    '绑定草稿不写 work lane');
  assert.equal(rt.state.pendingDraftMode, 'plan', '显式选择必须暂存，物化时按暂存值应用');
  assert.equal(rt.state.modeState.mode, 'plan', '草稿显示跟随切换');
});

test('setDraftMode：未绑定草稿维持本 lane 语义且不暂存（回归保护）', async () => {
  const rt = loadFeature('interaction');
  await rt.api.setDraftMode('plan');
  assert.deepEqual(rt.invokeArgs('set_mode_default'), [{ lane: 'work', mode: 'plan' }]);
  assert.equal(rt.state.pendingDraftMode, null, '未绑定草稿不引入暂存语义');

  const rtDesign = loadFeature('interaction', null, { modeLane: 'design' });
  await rtDesign.api.setDraftMode('plan');
  assert.deepEqual(rtDesign.invokeArgs('set_mode_default'), [{ lane: 'design', mode: 'plan' }]);
});

// ── code 权限偏好包装（YOLO 确认门事实源）──────────────────────
test('getCodePermissionPrefs：读取失败按 null（确认门按未确认处理，安全方向）', async () => {
  const rt = loadFeature('interaction', {
    invoke(name) {
      if (name === 'get_code_permission_prefs') return Promise.resolve({ last_mode: 'plan', yolo_confirmed: false });
      return Promise.resolve({});
    },
  });
  const prefs = await rt.api.getCodePermissionPrefs();
  assert.equal(prefs.yolo_confirmed, false);

  const rtFail = loadFeature('interaction', {
    invoke(name) {
      if (name === 'get_code_permission_prefs') return Promise.reject(new Error('backend down'));
      return Promise.resolve({});
    },
  });
  assert.equal(await rtFail.api.getCodePermissionPrefs(), null, '读取失败不得上抛打断切换流程');
});

test('confirmCodeYolo：调用 confirm_code_yolo 并透传返回；失败上抛给 UI', async () => {
  const rt = loadFeature('interaction', {
    invoke(name) {
      if (name === 'confirm_code_yolo') return Promise.resolve({ last_mode: 'plan', yolo_confirmed: true });
      return Promise.resolve({});
    },
  });
  const prefs = await rt.api.confirmCodeYolo();
  assert.equal(prefs.yolo_confirmed, true);
  assert.ok(rt.invokeNames().includes('confirm_code_yolo'));

  const rtFail = loadFeature('interaction', {
    invoke(name) {
      if (name === 'confirm_code_yolo') return Promise.reject(new Error('write failed'));
      return Promise.resolve({});
    },
  });
  await assert.rejects(rtFail.api.confirmCodeYolo(), /write failed/, '确认写盘失败必须上抛，不得静默放行');
});
