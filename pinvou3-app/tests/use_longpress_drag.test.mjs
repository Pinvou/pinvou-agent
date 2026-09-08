#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-longpress-'));
// 与 use_throttled_value.test.mjs 同款:hook 源码逐字节复制导入,react 以
// 最小桩解析(node --test 无渲染器、仓库无 jsdom),真实驱动实现。
const hookTmp = path.join(dir, 'useLongPressDrag.mjs');
copyFileSync(path.join(here, '..', 'src', 'hooks', 'useLongPressDrag.js'), hookTmp);
const reactDir = path.join(dir, 'node_modules', 'react');
mkdirSync(reactDir, { recursive: true });
writeFileSync(
  path.join(reactDir, 'package.json'),
  JSON.stringify({ name: 'react', type: 'module', main: 'index.mjs', exports: { default: './index.mjs' } }),
);
writeFileSync(
  path.join(reactDir, 'index.mjs'),
  [
    'const rt = () => globalThis.__pinvouReactHooks;',
    'export const useState = (...args) => rt().useState(...args);',
    'export const useRef = (...args) => rt().useRef(...args);',
    '',
  ].join('\n'),
);
const { useLongPressDrag } = await import(pathToFileURL(hookTmp).href);

// ── 最小 React hooks 桩(语义对齐:useState Object.is 幂等、useRef 跨渲染同实例) ──
function createHookRuntime() {
  const states = [];
  const refs = [];
  let stateIndex = 0;
  let refIndex = 0;
  let dirty = false;
  const useState = (initial) => {
    const i = stateIndex++;
    if (states.length <= i) states.push(initial);
    const setState = (update) => {
      const prev = states[i];
      const next = typeof update === 'function' ? update(prev) : update;
      if (!Object.is(next, prev)) { states[i] = next; dirty = true; }
    };
    return [states[i], setState];
  };
  const useRef = (initial) => {
    const i = refIndex++;
    if (refs.length <= i) refs.push({ current: initial });
    return refs[i];
  };
  return {
    useState,
    useRef,
    beginRender() { stateIndex = 0; refIndex = 0; },
    flush() { dirty = false; },
    get dirty() { return dirty; },
  };
}

// ── 最小 DOM 桩:elementFromPoint 可编程,命中 [data-project-drop-target] ──
function createDom(pointTarget) {
  const dropEl = {
    closest: (sel) => (sel === '[data-project-drop-target]' ? dropEl : null),
    getAttribute: (name) => (name === 'data-drop-key' ? (pointTarget.key ?? null) : null),
  };
  const plain = { closest: () => null, getAttribute: () => null };
  const doc = {
    body: { style: {} },
    dragstartListeners: [],
    addEventListener(type, fn) { if (type === 'dragstart') this.dragstartListeners.push(fn); },
    elementAt: plain,
    elementFromPoint() { return pointTarget.hit ? dropEl : plain; },
  };
  return doc;
}

function makeHarness(options = {}) {
  const pointTarget = options.pointTarget || { hit: false, key: null };
  globalThis.document = createDom(pointTarget);
  const runtime = createHookRuntime();
  globalThis.__pinvouReactHooks = runtime;
  const events = { hover: [], drops: [], pickups: [] };
  const surface = {
    captured: [],
    setPointerCapture(id) { this.captured.push(id); },
    getBoundingClientRect() { return { left: 100, top: 200, width: 240, height: 44 }; },
  };
  const press = (x, y, overrides = {}) => ({
    button: 0,
    pointerId: 7,
    clientX: x,
    clientY: y,
    currentTarget: surface,
    target: { closest: () => null },
    ...overrides,
  });
  const move = (x, y) => ({ pointerId: 7, clientX: x, clientY: y });
  const up = (x, y) => ({ pointerId: 7, clientX: x, clientY: y });
  const render = (kind, onPickUp, moveDrag) => {
    lastArgs = [kind, onPickUp, moveDrag];
    runtime.beginRender();
    const hook = useLongPressDrag(kind, onPickUp, moveDrag);
    runtime.flush();
    return hook;
  };
  // 状态更新后取新快照(React 重渲染语义:旧返回值不热更新)。
  const rerender = () => render(...lastArgs);
  let lastArgs = null;
  return { runtime, events, surface, press, move, up, render, rerender, pointTarget };
}

const dragCfg = (events) => ({
  enabled: true,
  payload: 'sess-1',
  onHover: (key) => events.hover.push(key),
  onDrop: (key, payload) => events.drops.push([key, payload]),
});

// ── 即移拖拽(移动到项目)───────────────────────────────────────────────────

{
  const h = makeHarness();
  const pickUp = (info) => h.events.pickups.push(info);
  const hook = h.render('session', pickUp, dragCfg(h.events));
  hook.handlers.onPointerDown(h.press(150, 220));
  // 未超阈值的小幅移动不进入拖拽(避免误触点击)。
  hook.handlers.onPointerMove(h.move(155, 224));
  assert.equal(h.rerender().moveDragging, false);
  // 超过 10px 立即进入拖拽并捕获指针;此刻不在目标上 → hover(null)。
  hook.handlers.onPointerMove(h.move(150, 245));
  assert.equal(h.rerender().moveDragging, true);
  assert.deepEqual(h.surface.captured, [7], '激活即 setPointerCapture');
  assert.deepEqual(h.events.hover, [], '激活瞬间不上报,首次移动才上报');
  // 移到项目组头上 → hover 上报 drop-key;悬停多久都无所谓(指针路径)。
  h.pointTarget.hit = true;
  h.pointTarget.key = 'project:prj-1';
  hook.handlers.onPointerMove(h.move(150, 400));
  hook.handlers.onPointerMove(h.move(150, 401));
  assert.deepEqual(h.events.hover, ['project:prj-1', 'project:prj-1']);
  // 悬停后松手:drop 正常送达(WebKitGTK 的 HTML5 悬停丢 drop 问题不复存在)。
  hook.handlers.onPointerUp(h.up(150, 401));
  assert.deepEqual(h.events.drops, [['project:prj-1', 'sess-1']]);
  assert.equal(h.rerender().moveDragging, false, '松手复位');
}

// ── 落点不在任何目标上 → onDrop(null);点击被 guardClick 吞掉 ────────────────

{
  const h = makeHarness();
  const hook = h.render('session', undefined, dragCfg(h.events));
  hook.handlers.onPointerDown(h.press(150, 220));
  hook.handlers.onPointerMove(h.move(200, 260));
  hook.handlers.onPointerUp(h.up(200, 262));
  assert.deepEqual(h.events.drops, [[null, 'sess-1']]);
  let clicked = false;
  hook.guardClick(() => { clicked = true; })({ stopPropagation() {}, preventDefault() {} });
  assert.equal(clicked, false, '拖拽后的 click 不得触发选择会话');
  // 下一轮正常点击不受影响。
  const hook2 = h.render('session', undefined, dragCfg(h.events));
  hook2.guardClick(() => { clicked = true; })({ stopPropagation() {}, preventDefault() {} });
  assert.equal(clicked, true);
}

// ── pointercancel 丢弃拖拽(不触发 drop)────────────────────────────────────

{
  const h = makeHarness();
  const hook = h.render('session', undefined, dragCfg(h.events));
  hook.handlers.onPointerDown(h.press(150, 220));
  hook.handlers.onPointerMove(h.move(180, 240));
  h.pointTarget.hit = true;
  hook.handlers.onPointerMove(h.move(180, 300));
  hook.handlers.onPointerCancel();
  assert.deepEqual(h.events.drops, [], 'cancel 不投递落点');
  assert.deepEqual(h.events.hover.slice(-1), [null], 'cancel 清空悬停高亮');
}

// ── 长按 350ms 仍是 tear-off 拆窗,与即移拖拽互斥 ────────────────────────────

{
  const h = makeHarness();
  const hook = h.render('session', (info) => h.events.pickups.push(info), dragCfg(h.events));
  hook.handlers.onPointerDown(h.press(150, 220));
  await new Promise((resolve) => { setTimeout(resolve, 400); });
  assert.equal(h.events.pickups.length, 1, '长按触发 tear-off');
  // picked 之后的大幅移动不得进入即移拖拽。
  hook.handlers.onPointerMove(h.move(150, 300));
  assert.equal(h.rerender().moveDragging, false);
  assert.deepEqual(h.events.hover, []);
}

// ── 未启用即移拖拽时维持旧语义:移动只取消长按等待 ───────────────────────────

{
  const h = makeHarness();
  const hook = h.render('session', (info) => h.events.pickups.push(info));
  hook.handlers.onPointerDown(h.press(150, 220));
  hook.handlers.onPointerMove(h.move(150, 260));
  await new Promise((resolve) => { setTimeout(resolve, 400); });
  assert.equal(h.events.pickups.length, 0, '移动取消长按,不拆窗也不拖拽');
  assert.equal(h.rerender().moveDragging, false);
}

// ── tear-off 不可用(dndPayload 在)仍可即移拖拽 ─────────────────────────────

{
  const h = makeHarness();
  const hook = h.render('session', undefined, dragCfg(h.events));
  assert.equal(typeof hook.handlers.onPointerDown, 'function');
  hook.handlers.onPointerDown(h.press(150, 220));
  hook.handlers.onPointerMove(h.move(150, 250));
  assert.equal(h.rerender().moveDragging, true, '无 onPickUp 也能拖入项目');
}

rmSync(dir, { recursive: true, force: true });
console.log('useLongPressDrag pointer-drag behavior passed');
