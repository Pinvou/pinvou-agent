#!/usr/bin/env node
// code-permission-state.js 的纯逻辑回归：code 会话 mode 默认值解析（首次 Plan /
// 跟随全局 last_mode）、yolo 一次性确认门、chip 展示值归属保护。
// 附带 CodexAcpView.jsx 的轻量源码契约：mode 由后端驱动 + 确认卡接线存在。
// 风格对齐 code_native_lane.test.mjs：把模块复制到临时 type:module 目录再导入。
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-code-permission-'));
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
mkdirSync(path.join(temp, 'codex'), { recursive: true });
copyFileSync(
  path.join(root, 'src', 'features', 'codex', 'code-permission-state.js'),
  path.join(temp, 'codex', 'code-permission-state.js'),
);
copyFileSync(
  path.join(root, 'src', 'features', 'codex', 'native-session-handoff.js'),
  path.join(temp, 'codex', 'native-session-handoff.js'),
);
copyFileSync(
  path.join(root, 'src', 'features', 'codex', 'acp-session-operation.js'),
  path.join(temp, 'codex', 'acp-session-operation.js'),
);

try {
  const {
    CODE_MODE_FALLBACK,
    nativeModeFallback,
    needsYoloConfirmation,
    resolveNativeModeValue,
  } = await import(`${pathToFileURL(path.join(temp, 'codex', 'code-permission-state.js')).href}?t=${Date.now()}`);
  const {
    canApplyNativeControlsRefresh,
    claimNativeControlsRefreshId,
    finalizePreparedSessionCreation,
    resolveNativeModelId,
  } = await import(`${pathToFileURL(path.join(temp, 'codex', 'native-session-handoff.js')).href}?t=${Date.now()}`);
  const { createAcpSessionOperationTracker } = await import(
    `${pathToFileURL(path.join(temp, 'codex', 'acp-session-operation.js')).href}?t=${Date.now()}`
  );

  // ── 全局默认 mode 解析 ─────────────────────────────────────────
  assert.equal(CODE_MODE_FALLBACK, 'plan', '兜底必须是只读方向 Plan');
  assert.equal(nativeModeFallback(null), 'plan', 'prefs 未拉到（首次）→ Plan');
  assert.equal(nativeModeFallback({ last_mode: null, yolo_confirmed: false }), 'plan', '无记录 → Plan');
  assert.equal(nativeModeFallback({ last_mode: 'yolo', yolo_confirmed: true }), 'yolo', '跟随上次显式 mode');
  assert.equal(nativeModeFallback({ last_mode: 'plan', yolo_confirmed: true }), 'plan');
  assert.equal(nativeModeFallback({ last_mode: 'bogus', yolo_confirmed: false }), 'plan', '非法值兜底 Plan');

  // ── yolo 一次性确认门 ──────────────────────────────────────────
  assert.equal(needsYoloConfirmation(null), true, 'prefs 读取失败按未确认（安全方向）');
  assert.equal(needsYoloConfirmation({ yolo_confirmed: false }), true, '未确认 → 弹卡');
  assert.equal(needsYoloConfirmation({ yolo_confirmed: true }), false, '已确认 → 直切');

  // ── chip 展示值归属保护 ────────────────────────────────────────
  // 会话控件已归属刷新 → 用会话实测值（get_mode_state 驱动，无视全局默认）。
  assert.equal(resolveNativeModeValue({
    activeId: 's1', controlsSessionId: 's1', controlsMode: 'yolo', draftMode: null, prefs: null,
  }), 'yolo');
  // 切会话途中（控件还是上一会话的）→ 全局默认，不闪上一会话的值。
  assert.equal(resolveNativeModeValue({
    activeId: 's2', controlsSessionId: 's1', controlsMode: 'yolo', draftMode: null, prefs: null,
  }), 'plan');
  assert.equal(resolveNativeModeValue({
    activeId: 's2', controlsSessionId: 's1', controlsMode: 'plan', draftMode: null,
    prefs: { last_mode: 'yolo', yolo_confirmed: true },
  }), 'yolo');
  // 首发物化中的新会话使用与该会话绑定的 handoff，不能闪成全局默认。
  assert.equal(resolveNativeModeValue({
    activeId: 's2', controlsSessionId: null, controlsMode: 'plan', draftMode: null,
    handoffMode: 'yolo', prefs: null,
  }), 'yolo');
  // 草稿态：无暂存 → 全局默认（首次 Plan / 跟随 last_mode）；有暂存 → 暂存优先。
  assert.equal(resolveNativeModeValue({
    activeId: null, controlsSessionId: null, controlsMode: 'plan', draftMode: null, prefs: null,
  }), 'plan');
  assert.equal(resolveNativeModeValue({
    activeId: null, controlsSessionId: null, controlsMode: 'plan', draftMode: null,
    prefs: { last_mode: 'yolo', yolo_confirmed: true },
  }), 'yolo');
  assert.equal(resolveNativeModeValue({
    activeId: null, controlsSessionId: null, controlsMode: 'plan', draftMode: 'plan',
    prefs: { last_mode: 'yolo', yolo_confirmed: true },
  }), 'plan');

  // ── CodexAcpView.jsx 接线契约 ──────────────────────────────────
  // A staged model must remain visible throughout delayed activation. A late control
  // response from the previous request cannot replace the authoritative new-session value.
  let controls = { sessionId: 'old-session', modelId: 'model-b' };
  const handoff = { sessionId: 'new-session', modelId: 'model-a' };
  assert.equal(resolveNativeModelId({
    activeId: null,
    controlsSessionId: controls.sessionId,
    controlsModelId: controls.modelId,
    draftModelId: 'model-a',
    handoffModelId: null,
  }), 'model-a');
  assert.equal(resolveNativeModelId({
    activeId: handoff.sessionId,
    controlsSessionId: controls.sessionId,
    controlsModelId: controls.modelId,
    draftModelId: null,
    handoffModelId: handoff.modelId,
  }), 'model-a');

  const applyRefresh = ({ requestId, latestRequestId, sessionId, modelId }) => {
    if (!canApplyNativeControlsRefresh({
      requestId,
      latestRequestId,
      sessionId,
      activeId: handoff.sessionId,
    })) return false;
    controls = { sessionId, modelId };
    return true;
  };
  assert.equal(applyRefresh({
    requestId: 2,
    latestRequestId: 2,
    sessionId: handoff.sessionId,
    modelId: 'model-a',
  }), true);
  assert.equal(applyRefresh({
    requestId: 1,
    latestRequestId: 2,
    sessionId: handoff.sessionId,
    modelId: null,
  }), false);
  assert.equal(resolveNativeModelId({
    activeId: handoff.sessionId,
    controlsSessionId: controls.sessionId,
    controlsModelId: controls.modelId,
    draftModelId: null,
    handoffModelId: handoff.modelId,
  }), 'model-a');

  // ── 请求序号发放：陈旧会话的刷新不得占用序号 ─────────────────────
  // 发起时已不归属当前会话的刷新注定被归属检查丢弃；若它占用序号，会把当前会话
  // 在途的权威刷新顶成过期且无人补发，控件卡死在全局兜底（跨会话抢占回归）。
  {
    let latest = 0;
    // 当前会话 B 的权威刷新正常领取序号。
    let claim = claimNativeControlsRefreshId({ sessionId: 'B', activeId: 'B', latestRequestId: latest });
    latest = claim.latestRequestId;
    assert.equal(claim.requestId, 1);
    // 用户在 accept_plan 在途期间从 A 切到 B；A 的陈旧刷新晚发起，不得顶掉序号。
    const stale = claimNativeControlsRefreshId({ sessionId: 'A', activeId: 'B', latestRequestId: latest });
    assert.equal(stale.requestId, 0, 'stale-session refresh must not claim a sequence number');
    assert.equal(stale.latestRequestId, 1, 'stale-session refresh must not supersede the active request');
    // B 的在途刷新返回后仍可提交；陈旧请求提交时被拒绝。
    assert.equal(canApplyNativeControlsRefresh({
      requestId: 1, latestRequestId: stale.latestRequestId, sessionId: 'B', activeId: 'B',
    }), true);
    assert.equal(canApplyNativeControlsRefresh({
      requestId: stale.requestId, latestRequestId: stale.latestRequestId, sessionId: 'A', activeId: 'B',
    }), false);
    // 同会话内仍保持后发胜出：B 再次刷新领取新序号，旧响应被丢弃。
    claim = claimNativeControlsRefreshId({ sessionId: 'B', activeId: 'B', latestRequestId: stale.latestRequestId });
    assert.equal(claim.requestId, 2);
    assert.equal(canApplyNativeControlsRefresh({
      requestId: 1, latestRequestId: claim.latestRequestId, sessionId: 'B', activeId: 'B',
    }), false);
    // 序号虽是最新，但会话不归属当前：归属检查必须独立承重，不能只靠序号差异。
    assert.equal(canApplyNativeControlsRefresh({
      requestId: 3, latestRequestId: 3, sessionId: 'A', activeId: 'B',
    }), false, 'a current sequence number must not let a stale session commit');
  }

  // Preparation can fail after the backend session exists. This exercises the
  // integration between creation and the real operation tracker: activation
  // invalidates the draft token, then sendNative-style rebinding makes the
  // visible error path current again.
  {
    const preparationError = new Error('model persistence failed');
    const calls = [];
    const visibleErrors = [];
    const tracker = createAcpSessionOperationTracker('draft');
    let operation = tracker.begin('draft', 'send');
    let activeSessionId = null;
    const created = await finalizePreparedSessionCreation({
      sessionId: 'new-session',
      prepareSession: async sessionId => {
        calls.push(`prepare:${sessionId}`);
        throw preparationError;
      },
      shouldActivate: () => {
        calls.push('should-activate');
        return tracker.isCurrent(operation);
      },
      activateSession: sessionId => {
        calls.push(`activate:${sessionId}`);
        activeSessionId = sessionId;
        tracker.switchSession(sessionId);
      },
      loadSession: async sessionId => {
        calls.push(`load:${sessionId}`);
        return null;
      },
      loadInactiveSessionInfo: null,
    });
    assert.equal(tracker.isCurrent(operation), false, 'activation invalidates the draft operation');
    if (created.activated && activeSessionId === created.id) {
      tracker.switchSession(created.id);
      operation = tracker.begin(created.id, 'send');
    }
    try {
      if (created.preparationError) throw created.preparationError;
    } catch (err) {
      if (tracker.isCurrent(operation)) visibleErrors.push(err);
    }
    assert.equal(activeSessionId, 'new-session');
    assert.equal(operation.sessionId, 'new-session');
    assert.deepEqual(visibleErrors, [preparationError]);
    assert.deepEqual(calls, [
      'prepare:new-session',
      'should-activate',
      'activate:new-session',
      'load:new-session',
    ]);
  }

  // ── finalize 非激活分支与异常传播 ─────────────────────────────────
  {
    const calls = [];
    const created = await finalizePreparedSessionCreation({
      sessionId: 's9',
      prepareSession: async () => { calls.push('prepare'); },
      shouldActivate: () => { calls.push('should-activate'); return false; },
      activateSession: () => { throw new Error('must not activate'); },
      loadSession: async () => { throw new Error('must not load'); },
      loadInactiveSessionInfo: async id => { calls.push(`info:${id}`); return { id }; },
    });
    assert.deepEqual(created, { id: 's9', info: { id: 's9' }, activated: false });
    assert.deepEqual(calls, ['prepare', 'should-activate', 'info:s9'],
      'non-activation must not activate or load, only fetch inactive info');
    const prepErr = new Error('prepare failed before activation check');
    await assert.rejects(finalizePreparedSessionCreation({
      sessionId: 's9',
      prepareSession: async () => { throw prepErr; },
      shouldActivate: () => false,
      activateSession: () => {},
      loadSession: async () => null,
      loadInactiveSessionInfo: null,
    }), prepErr, 'prepare failure with no activation must throw directly');
    const loadErr = new Error('load failed after activation');
    await assert.rejects(finalizePreparedSessionCreation({
      sessionId: 's9',
      prepareSession: null,
      shouldActivate: () => true,
      activateSession: () => {},
      loadSession: async () => { throw loadErr; },
      loadInactiveSessionInfo: null,
    }), loadErr, 'load failures must propagate to the caller');
  }

  // ── 权威控件 vs handoff 优先级（用不同值区分分支） ────────────────
  // 权威刷新按归属提交后，即使残留 handoff 也必须用实测值；无 handoff 且
  // 控件归属过期时显示空，绝不显示上一会话的值。
  assert.equal(resolveNativeModelId({
    activeId: 's1', controlsSessionId: 's1', controlsModelId: 'model-b',
    draftModelId: null, handoffModelId: 'model-a',
  }), 'model-b', 'session-owned authoritative model must beat the stale handoff');
  assert.equal(resolveNativeModelId({
    activeId: 's1', controlsSessionId: 'old', controlsModelId: 'model-b',
    draftModelId: null, handoffModelId: null,
  }), null, 'no handoff + stale controls must show null, never the previous session model');
  assert.equal(resolveNativeModeValue({
    activeId: 's1', controlsSessionId: 's1', controlsMode: 'plan',
    draftMode: null, handoffMode: 'yolo', prefs: null,
  }), 'plan', 'session-owned authoritative mode must beat the stale handoff');

  const view = readFileSync(path.join(root, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
  assert.match(view, /invoke\('get_code_permission_prefs'\)/, '启动/切换拉取全局 code 权限偏好');
  assert.match(view, /invoke\('confirm_code_yolo'\)/, '确认卡【确认】写全局标志');
  assert.match(view, /resolveNativeModeValue\(/, 'chip 展示值经纯逻辑解析');
  assert.match(view, /resolveNativeModelId\(/, 'native model display must use the tested handoff resolver');
  assert.match(view, /finalizePreparedSessionCreation\(/, 'native session creation must use the tested preparation lifecycle');
  assert.match(
    view,
    /operation = beginAcpSendOperation\(targetId\);[\s\S]{0,800}if \(created\.preparationError\) throw created\.preparationError;/,
    'native preparation errors must surface only after the send operation is rebound',
  );
  // activeIdRef 只允许在 layout effect 内重指：渲染期赋值会被携带旧 prop 的中间
  // 渲染打回旧值，吞掉 loadSession 的乐观交接（首发标签闪回全局兜底的根因之一）。
  assert.equal((view.match(/activeIdRef\.current = activeId;/g) || []).length, 1,
    'activeIdRef must be assigned from the prop exactly once');
  assert.ok(
    view.indexOf('activeIdRef.current = activeId;') > view.indexOf('useLayoutEffect(() => {')
      && view.indexOf('activeIdRef.current = activeId;') < view.indexOf('acpConfigOperationTracker.switchSession'),
    'activeIdRef must only be re-pointed inside the layout effect so optimistic handoffs survive intermediate renders',
  );
  const clearNativeDraft = view.indexOf('function clearNativeDraftControls(staged)');
  assert.ok(
    clearNativeDraft >= 0
      && view.indexOf('setNativeDraftControls(current => current === staged ? {} : current)', clearNativeDraft) > clearNativeDraft
      && view.indexOf('nativeDraftControlsHandoffRef.current = null;', clearNativeDraft) > clearNativeDraft,
    'clearing draft controls must stay identity-guarded and must drop the matching handoff',
  );
  assert.ok(
    view.includes(': (nativeDraftControlsHandoff?.mountedId ?? null)')
      && view.includes(': Boolean(nativeDraftControlsHandoff?.multiAgent)'),
    'knowledge and multi-agent display must keep the same activation handoff as model/mode',
  );
  const yoloCard = readFileSync(path.join(root, 'src', 'shared', 'yolo-confirm-card.jsx'), 'utf8');
  assert.match(view, /<YoloConfirmCard/, 'CodexAcpView 复用共享 yolo 确认卡');
  assert.match(yoloCard, /data-testid="native-yolo-confirm"/, 'yolo 确认卡渲染');
  assert.match(view, /needsYoloConfirmation\(prefs\)/, '切 yolo 前过确认门');
  assert.doesNotMatch(view, /mountedId: null, mode: 'yolo'/, '不再写死 yolo 初始 mode');
  assert.doesNotMatch(view, /\|\| 'yolo'/, '不再有 || yolo 兜底');

  console.log('code_permission_state: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
