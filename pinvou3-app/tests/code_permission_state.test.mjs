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

try {
  const {
    CODE_MODE_FALLBACK,
    nativeModeFallback,
    needsYoloConfirmation,
    resolveNativeModeValue,
  } = await import(`${pathToFileURL(path.join(temp, 'codex', 'code-permission-state.js')).href}?t=${Date.now()}`);

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
  const view = readFileSync(path.join(root, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
  assert.match(view, /invoke\('get_code_permission_prefs'\)/, '启动/切换拉取全局 code 权限偏好');
  assert.match(view, /invoke\('confirm_code_yolo'\)/, '确认卡【确认】写全局标志');
  assert.match(view, /resolveNativeModeValue\(/, 'chip 展示值经纯逻辑解析');
  assert.match(view, /data-testid="native-yolo-confirm"/, 'yolo 确认卡渲染');
  assert.match(view, /needsYoloConfirmation\(prefs\)/, '切 yolo 前过确认门');
  assert.doesNotMatch(view, /mountedId: null, mode: 'yolo'/, '不再写死 yolo 初始 mode');
  assert.doesNotMatch(view, /\|\| 'yolo'/, '不再有 \|\| \'yolo\' 兜底');

  console.log('code_permission_state: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
