#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-throttle-'));
// hook 源码逐字节拷贝后真实 import；唯一的依赖 'react' 通过临时 node_modules
// 桩解析（node --test 没有 React 渲染器，仓库也没有 jsdom），从而可以驱动
// 真实的 hook 实现而不改动被测源文件。
const hookTmp = path.join(dir, 'useThrottledValue.mjs');
copyFileSync(path.join(here, '..', 'src', 'features', 'conversation', 'useThrottledValue.js'), hookTmp);
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
    'export const useState = (...args) => rt().hooks.useState(...args);',
    'export const useRef = (...args) => rt().hooks.useRef(...args);',
    'export const useEffect = (...args) => rt().hooks.useEffect(...args);',
    '',
  ].join('\n'),
);

// ── 最小 React hooks 运行时桩 ──
// 只实现 useThrottledValue 用到的子集，但语义必须与 React 一致：
// - useState：Object.is 幂等写，写入后置 dirty 由驱动器重渲染；
// - useRef：跨渲染返回同一实例；
// - useEffect：依赖逐位 Object.is 比较；同一提交内按声明顺序冲刷、
//   重跑前先执行旧 cleanup。「同一提交内效果按声明顺序冲刷」是该 hook
//   在收流提交（文本与 streaming 标志同 commit 变化）上保持正确性的承重点：
//   ref 同步效果必须先于收敛效果执行。
function createRuntime() {
  const states = [];
  const refs = [];
  const slots = [];
  let stateIndex = 0;
  let refIndex = 0;
  let effectIndex = 0;
  let dirty = false;
  let queue = [];

  const useState = (initial) => {
    const i = stateIndex++;
    if (states.length <= i) states.push(typeof initial === 'function' ? initial() : initial);
    const setState = (update) => {
      const prev = states[i];
      const next = typeof update === 'function' ? update(prev) : update;
      if (!Object.is(next, prev)) {
        states[i] = next;
        dirty = true;
      }
    };
    return [states[i], setState];
  };

  const useRef = (initial) => {
    const i = refIndex++;
    if (refs.length <= i) refs.push({ current: initial });
    return refs[i];
  };

  const useEffect = (fn, deps) => {
    const i = effectIndex++;
    if (slots.length <= i) slots.push({ deps: undefined, cleanup: undefined, ran: false });
    const slot = slots[i];
    const changed = !slot.ran
      || deps === undefined
      || deps.length !== slot.deps.length
      || deps.some((d, k) => !Object.is(d, slot.deps[k]));
    if (changed) queue.push({ fn, slot, deps });
  };

  return {
    hooks: { useState, useRef, useEffect },
    render(hook, value, delayMs, active) {
      stateIndex = 0;
      refIndex = 0;
      effectIndex = 0;
      dirty = false;
      queue = [];
      const ret = hook(value, delayMs, active);
      for (const { fn, slot, deps } of queue) {
        if (typeof slot.cleanup === 'function') slot.cleanup();
        const result = fn();
        slot.cleanup = typeof result === 'function' ? result : undefined;
        slot.deps = deps;
        slot.ran = true;
      }
      return ret;
    },
    get dirty() {
      return dirty;
    },
  };
}

// 驱动器：render/写状态后循环重渲染直到稳定（对应 React 的效果后批处理重渲染）。
// tick() 手动触发采样 interval 的回调，等价于 delayMs 时钟到期。
function createHarness(hook) {
  const fakeWindow = {
    intervals: new Map(),
    nextId: 1,
    setInterval(cb) {
      const id = this.nextId++;
      this.intervals.set(id, cb);
      return id;
    },
    clearInterval(id) {
      this.intervals.delete(id);
    },
  };
  const runtime = createRuntime();
  globalThis.__pinvouReactHooks = runtime;
  let current = { value: undefined, delayMs: 200, active: true };
  let lastReturn;

  const settle = () => {
    // hook 的采样器读取的是全局 window；每个 harness 挂载自己的 fakeWindow。
    globalThis.window = fakeWindow;
    let guard = 0;
    let ret = runtime.render(hook, current.value, current.delayMs, current.active);
    while (runtime.dirty) {
      if (++guard > 25) throw new Error('useThrottledValue did not settle within 25 render passes');
      ret = runtime.render(hook, current.value, current.delayMs, current.active);
    }
    lastReturn = ret;
    return ret;
  };

  return {
    fakeWindow,
    get return() {
      return lastReturn;
    },
    render(value, delayMs = 200, active = true) {
      current = { value, delayMs, active };
      return settle();
    },
    tick() {
      for (const cb of [...fakeWindow.intervals.values()]) cb();
      return settle();
    },
  };
}

try {
  const { useThrottledValue } = await import(pathToFileURL(hookTmp).href);

  // T1: active 流式期间的 200ms 预算 —— 修复前效果在每次 commit 直通冲刷，
  // 预算完全不生效；修复后返回值按 interval 节奏拖尾。
  const streaming = createHarness(useThrottledValue);
  assert.equal(streaming.render('v1'), 'v1', '首次提交采纳初始值');
  assert.equal(
    streaming.render('v2'),
    'v1',
    '间隔未到期时新 delta 不得立即冲刷（时间预算生效）',
  );
  assert.equal(streaming.tick(), 'v2', 'interval tick 发布最新采样值');
  streaming.render('v3');
  streaming.render('v4');
  assert.equal(streaming.return, 'v2', 'burst 期间返回值继续拖尾');
  assert.equal(
    streaming.tick(),
    'v4',
    'ref 随每次提交重同步，burst 不会饿死尾随更新',
  );

  // T2: 合并的收尾提交（最终文本与 streaming 标志同 commit 到达）必须原样
  // 渲染完整文本 —— 直通由标志翻转的同一提交结构性保证，不依赖任何 tick。
  // 这是修复的阻塞缺陷：修复前该场景会永久渲染截断的旧采样。
  const finalText = '最终回答全文，含 Markdown。';
  assert.equal(
    streaming.render(finalText, 200, false),
    finalText,
    '收流提交必须立即渲染完整文本，不等 tick',
  );
  assert.equal(streaming.fakeWindow.intervals.size, 0, '流结束必须拆除采样 interval');
  assert.equal(
    streaming.render(finalText, 200, false),
    finalText,
    'inactive 状态已收敛为最终文本，绝不复活收流前的旧采样 v4',
  );

  // T3: 重激活 —— 从收敛后的最终文本起步（首个 <delayMs 窗口显示最终文本是
  // 文档化的取舍，若未来改为激活时重同步需有意识地更新此断言），首个 tick
  // 后必须发布新流文本。
  assert.equal(
    streaming.render('r1', 200, true),
    finalText,
    '重激活起步于收敛后的最终文本，而非收流前样本',
  );
  assert.equal(streaming.fakeWindow.intervals.size, 1, '重激活重建采样 interval');
  assert.equal(streaming.tick(), 'r1', '首个 tick 发布新流文本');

  // T4: inactive 挂载 —— 直通且不调度采样器。
  const idle = createHarness(useThrottledValue);
  assert.equal(idle.render('x', 200, false), 'x');
  assert.equal(idle.fakeWindow.intervals.size, 0, 'inactive 挂载不得调度采样 interval');
  assert.equal(idle.render('y', 200, false), 'y', 'inactive 期间后续提交继续直通');

  console.log('useThrottledValue regression tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
