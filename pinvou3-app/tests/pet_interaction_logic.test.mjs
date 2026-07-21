#!/usr/bin/env node
import assert from 'node:assert';
import { copyFileSync, existsSync, mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const dragSrc = path.join(here, '..', 'src', 'features', 'pet', 'pet-interaction.js');
assert.ok(existsSync(dragSrc), 'pet-interaction.js must exist');

const dragTmp = path.join(mkdtempSync(path.join(tmpdir(), 'pet-interaction-')), 'pet-interaction.mjs');
copyFileSync(dragSrc, dragTmp);
const drag = await import(pathToFileURL(dragTmp));

assert.strictEqual(typeof drag.readPetDragContext, 'function');
assert.strictEqual(typeof drag.setPetWindowPosition, 'function');
assert.strictEqual(typeof drag.stepPetDrag, 'function', 'stepPetDrag must be exported');
assert.strictEqual(typeof drag.attachPetDragGeometry, 'function', 'attachPetDragGeometry must be exported');
assert.strictEqual(typeof drag.releasePetDrag, 'function', 'releasePetDrag must be exported');
assert.strictEqual(typeof drag.scaleFromResizeDrag, 'function', 'scaleFromResizeDrag must be exported');
assert.strictEqual(typeof drag.dragAnimationFromMotion, 'function', 'dragAnimationFromMotion must be exported');
assert.strictEqual(typeof drag.petWindowBounds, 'function', 'visible-pet bounds must be exported');
assert.strictEqual(
  typeof drag.petElementHorizontalBounds,
  'function',
  'visible activity-card bounds must be exported',
);
assert.strictEqual(typeof drag.petMonitorAtPosition, 'function', 'monitor lookup must be exported');
assert.strictEqual(
  typeof drag.clampPetDragToDesktop,
  'function',
  'desktop-hole rejection must be exported',
);
assert.strictEqual(
  typeof drag.petScreenAnchorFromRect,
  'function',
  'resize screen-anchor calculation must be exported',
);
assert.strictEqual(
  typeof drag.petAlignmentAtDragEdge,
  'function',
  'drag-edge alignment must be exported',
);
assert.strictEqual(
  typeof drag.rebasePetDragForAlignment,
  'function',
  'alignment changes must preserve the visible pet anchor',
);

{
  let moduleMonitorCalls = 0;
  let availableMonitorCalls = 0;
  let instanceMonitorCalls = 0;
  let receivedPosition = null;
  const position = { x: 120, y: -40 };
  const size = { width: 240, height: 330 };
  const monitor = { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 } };
  const win = {
    outerPosition: async () => position,
    outerSize: async () => size,
    currentMonitor: () => { instanceMonitorCalls += 1; throw new Error('instance API must not run'); },
    setPosition: async (next) => { receivedPosition = next; },
  };
  class PhysicalPosition {
    constructor(x, y) { this.x = x; this.y = y; }
  }
  const T = {
    window: {
      getCurrentWindow: () => win,
      currentMonitor: async () => { moduleMonitorCalls += 1; return monitor; },
      availableMonitors: async () => { availableMonitorCalls += 1; return [monitor]; },
      PhysicalPosition,
    },
  };

  const context = await drag.readPetDragContext(T);
  assert.strictEqual(context.win, win);
  assert.strictEqual(context.position, position);
  assert.strictEqual(context.size, size);
  assert.strictEqual(context.monitor, monitor);
  assert.deepStrictEqual(context.monitors, [monitor]);
  assert.strictEqual(moduleMonitorCalls, 1);
  assert.strictEqual(availableMonitorCalls, 1);
  assert.strictEqual(instanceMonitorCalls, 0);

  await drag.setPetWindowPosition(T, win, 10.6, -2.4);
  assert.ok(receivedPosition instanceof PhysicalPosition);
  assert.deepStrictEqual({ x: receivedPosition.x, y: receivedPosition.y }, { x: 11, y: -2 });
}

{
  const T = { window: { getCurrentWindow: () => ({}) } };
  await assert.rejects(() => drag.readPetDragContext(T), /currentMonitor/);
}

{
  const T = {
    window: {
      PhysicalPosition: class PhysicalPosition {},
    },
  };
  await assert.rejects(
    () => drag.setPetWindowPosition(T, { setPosition: () => { throw new Error('move failed'); } }, 1, 2),
    /move failed/,
  );
}

const base = {
  holding: true,
  x: 0, y: 0, tx: 100, ty: 0,
  vx: 0, vy: 0,
  bounds: null,
};
const closeTo = (actual, expected) => assert.ok(Math.abs(actual - expected) < 1e-9, `${actual} != ${expected}`);

{
  const pending = {
    holding: true,
    released: true,
    startCX: 100,
    startCY: 200,
    currentCX: 140,
    currentCY: 260,
    dpr: 2,
    x: 0,
    y: 0,
    tx: 0,
    ty: 0,
    vx: 1,
    vy: 2,
    bounds: null,
  };
  const next = drag.attachPetDragGeometry(pending, {
    position: { x: 300, y: 400 },
    size: { width: 240, height: 330 },
    monitor: { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 } },
  });
  assert.strictEqual(next.x, 300);
  assert.strictEqual(next.y, 400);
  assert.strictEqual(next.tx, 380);
  assert.strictEqual(next.ty, 520);
  closeTo(next.vx, 15.4);
  closeTo(next.vy, 23.6);
  assert.strictEqual(next.holding, false);
  assert.deepStrictEqual(next.bounds, { l: 0, t: 0, r: 1680, b: 750 });
  assert.strictEqual(pending.x, 0, 'attachPetDragGeometry must not mutate input');
}

{
  assert.deepStrictEqual(
    drag.petScreenAnchorFromRect({
      position: { x: 850, y: 400 },
      rect: { left: 230, top: 119 },
      scaleFactor: 1.25,
    }),
    { x: 1137.5, y: 548.75 },
    'the resize anchor must be the visible character top-left in physical screen pixels',
  );
  assert.strictEqual(
    drag.petScreenAnchorFromRect({ position: null, rect: null, scaleFactor: 1 }),
    null,
  );
}

{
  const readyBeforeFirstFrame = {
    holding: true,
    released: false,
    base: { x: 300, y: 400 },
    x: 300,
    y: 400,
    tx: 380,
    ty: 520,
    vx: 0,
    vy: 0,
    physicsSteps: 0,
  };
  const next = drag.releasePetDrag(readyBeforeFirstFrame);
  assert.strictEqual(next.released, true);
  assert.strictEqual(next.holding, false);
  closeTo(next.vx, 14.4);
  closeTo(next.vy, 21.6);
  assert.strictEqual(readyBeforeFirstFrame.holding, true, 'releasePetDrag must not mutate input');
}

{
  const next = drag.stepPetDrag(base);
  closeTo(next.x, 10.36);
  closeTo(next.vx, 10.36);
  assert.strictEqual(next.stopped, false);
  assert.strictEqual(base.x, 0, 'stepPetDrag must not mutate input');
}

{
  assert.strictEqual(drag.dragAnimationFromMotion('running-left', 20, 0), 'running-right');
  assert.strictEqual(drag.dragAnimationFromMotion('running-right', -12, 1), 'running-left');
  assert.strictEqual(drag.dragAnimationFromMotion('running-left', 0, 8), 'running-left');
  assert.strictEqual(
    drag.dragAnimationFromMotion('running-right', -1, 0),
    'running-right',
    'a ±1px jitter event must not flip the running direction',
  );
  assert.strictEqual(
    drag.dragAnimationFromMotion('running-right', -2, 0),
    'running-right',
    'reversing direction requires at least a 3px delta (hysteresis)',
  );
  assert.strictEqual(
    drag.dragAnimationFromMotion('running-right', -3, 0),
    'running-left',
    'a decisive 3px reverse delta must flip the direction',
  );
  assert.strictEqual(
    drag.dragAnimationFromMotion('running-right', 1, 0),
    'running-right',
    'keeping the current direction still works from a 1px delta',
  );
  assert.strictEqual(
    drag.dragAnimationFromMotion(null, -1, 0),
    'running-left',
    'establishing the initial direction still works from a 1px delta',
  );

  const reversing = {
    ...base,
    x: -47.39,
    tx: -80,
    vx: -20.08,
    lastTx: -100,
    lastTy: 0,
  };
  const next = drag.stepPetDrag(reversing);
  assert.ok(
    next.x > reversing.x,
    'while held, a pointer reversal must move the pet in the new direction on the next frame',
  );
}

{
  const next = drag.stepPetDrag({ ...base, holding: false, tx: 0, vx: 10 });
  closeTo(next.vx, 9.55);
  closeTo(next.x, 9.55);
  assert.strictEqual(next.stopped, false);
}

{
  const next = drag.stepPetDrag({
    ...base,
    holding: false,
    x: 99, tx: 99, vx: 10,
    bounds: { l: 0, t: 0, r: 100, b: 100 },
  });
  assert.strictEqual(next.x, 100);
  assert.ok(next.vx < 0, 'right-edge collision must reverse horizontal velocity');
}

{
  const next = drag.stepPetDrag({
    ...base,
    holding: true,
    x: 99, tx: 120, vx: 10,
    bounds: { l: 0, t: 0, r: 100, b: 100 },
  });
  assert.strictEqual(next.x, 100);
  assert.strictEqual(next.vx, 0, 'a held pet must stop at the edge instead of bouncing under the pointer');
}

{
  const next = drag.stepPetDrag({
    ...base,
    holding: false,
    x: 1, y: 1, tx: 1, ty: 1, vx: -10, vy: -10,
    bounds: { l: 0, t: 0, r: 100, b: 100 },
  });
  assert.strictEqual(next.x, 0);
  assert.strictEqual(next.y, 0);
  assert.ok(next.vx > 0 && next.vy > 0, 'top-left collision must reverse both velocities');
}

{
  const next = drag.stepPetDrag({ ...base, holding: false, tx: 0, vx: 0.2, vy: -0.2 });
  assert.strictEqual(next.stopped, true);
}

{
  const next = drag.stepPetDrag({ ...base, tx: 1000 });
  assert.strictEqual(next.tilt, 16);
}

{
  assert.strictEqual(drag.scaleFromResizeDrag(1, 2.4, 3.3), 1.01);
  assert.strictEqual(drag.scaleFromResizeDrag(1, 24, 33), 1.1);
  assert.strictEqual(drag.scaleFromResizeDrag(1, 240, 330), 1.2);
  assert.strictEqual(drag.scaleFromResizeDrag(1, -240, -330), 0.5);
  assert.strictEqual(drag.scaleFromResizeDrag(Number.NaN, 24, 33), 1.1);
}

{
  const monitor = { position: { x: 100, y: 0 }, size: { width: 1200, height: 800 } };
  assert.strictEqual(
    drag.petEdgeAlignment({ position: { x: 120, y: 50 }, size: { width: 350, height: 228 }, monitor }),
    'left',
  );
  assert.strictEqual(
    drag.petEdgeAlignment({ position: { x: 900, y: 50 }, size: { width: 350, height: 228 }, monitor }),
    'right',
  );
  assert.strictEqual(
    drag.petEdgeAlignment({
      position: { x: 100, y: 50 },
      size: { width: 350, height: 228 },
      monitor: { position: { x: 0, y: 0 }, size: { width: 1200, height: 800 } },
      currentAlignment: 'right',
      characterWidth: 192,
      horizontalPadding: 24,
    }),
    'right',
    'the activity side must not change while the pet is in the middle of a display',
  );
  assert.strictEqual(
    drag.petEdgeAlignment({
      position: { x: 984, y: 50 },
      size: { width: 350, height: 228 },
      monitor: { position: { x: 0, y: 0 }, size: { width: 1200, height: 800 } },
      currentAlignment: 'left',
      characterWidth: 192,
      horizontalPadding: 24,
    }),
    'right',
    'the activity side changes only when the visible pet reaches the far edge',
  );
  assert.strictEqual(
    drag.petEdgeAlignment({
      position: { x: 900, y: 50 },
      size: { width: 350, height: 228 },
      monitor: null,
      fallback: 'right',
    }),
    'right',
    'keep the previous alignment when monitor geometry is unavailable',
  );
}

{
  const left = { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 }, scaleFactor: 1 };
  const right = { position: { x: 1920, y: 0 }, size: { width: 1920, height: 1080 }, scaleFactor: 1.5 };
  const geometry = {
    monitors: [left, right],
    monitor: left,
    size: { width: 350, height: 332 },
    alignment: 'left',
    characterWidth: 192,
    characterHeight: 208,
    horizontalPadding: 24,
    verticalPadding: 8,
  };
  assert.deepStrictEqual(
    drag.petWindowBounds(geometry),
    { l: -24, t: -116, r: 3624, b: 756 },
    'connected displays must share one outer boundary so their seam does not bounce',
  );
  assert.strictEqual(
    drag.petMonitorAtPosition({
      position: { x: 2100, y: 500 },
      size: geometry.size,
      alignment: 'left',
      characterWidth: 192,
      characterHeight: 208,
      horizontalPadding: 24,
      verticalPadding: 8,
      monitors: geometry.monitors,
    }),
    right,
    'the active monitor must follow the visible pet across the seam',
  );
}

// 异高 / 错位 / L 形布局：相连显示器的外接矩形包含实际没有屏幕的空洞。
{
  // 左屏 1920x1080 @ (0,0)，右屏 2560x1440 @ (1920,0)——右屏高出 360px，
  // 于是左屏正下方 (x<1920, y>1080) 是外接矩形里的空洞。
  const short = { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 }, scaleFactor: 1 };
  const tall = { position: { x: 1920, y: 0 }, size: { width: 2560, height: 1440 }, scaleFactor: 1 };
  const metrics = {
    size: { width: 350, height: 332 },
    alignment: 'left',
    characterWidth: 192,
    characterHeight: 208,
    horizontalPadding: 24,
    verticalPadding: 8,
  };
  // 人物中心相对窗口左上角的偏移：x=120, y=220。
  const petCenter = (state) => ({ x: state.x + 120, y: state.y + 220 });
  const onAnyMonitor = ({ x, y }) => [short, tall].some((m) => (
    x >= m.position.x && x < m.position.x + m.size.width
    && y >= m.position.y && y < m.position.y + m.size.height
  ));

  const hole = { x: 1000, y: 1100, vx: 0, vy: 6, holding: false };
  assert.ok(
    !onAnyMonitor(petCenter(hole)),
    'the probe position must really sit in the bounding-box hole, else this test proves nothing',
  );
  const rescued = drag.clampPetDragToDesktop(hole, { monitors: [short, tall], ...metrics });
  assert.ok(
    onAnyMonitor(petCenter(rescued)),
    'a pet dropped into the hole must be projected back onto a real display',
  );
  assert.strictEqual(rescued.x, 1000, 'projection must not move the pet sideways when only y is out');
  assert.strictEqual(rescued.y, 859, 'the pet must land on the short display bottom edge (1079-220)');
  assert.ok(rescued.vy < 0, 'inertia into the hole must bounce back, not keep pushing');

  // 连续动画帧：单次钳制证明不了不抖、不逃逸，必须按真实帧循环跑。
  const geo = { monitors: [short, tall], ...metrics };
  const bounds = drag.petWindowBounds({ monitors: [short, tall], monitor: short, ...metrics });
  const frame = (state) => drag.clampPetDragToDesktop(drag.stepPetDrag(state), geo);

  // 1) 按住不放、往空洞深处拽 60 帧：每一帧人物都必须还在真实屏幕上。
  let held = {
    base: { x: 1000, y: 800 },
    x: 1000,
    y: 800,
    vx: 0,
    vy: 0,
    tx: 1000,
    ty: 1400, // 目标落在空洞深处
    lastTx: 1000,
    lastTy: 800,
    holding: true,
    bounds,
  };
  const heldTrail = [];
  for (let i = 0; i < 60; i += 1) {
    held = frame(held);
    heldTrail.push(held.y);
    assert.ok(
      onAnyMonitor(petCenter(held)),
      `holding frame ${i} must keep the pet on a real display (y=${held.y})`,
    );
  }
  // 贴住空洞边缘后应当停住，而不是每帧被弹回来又冲出去地抖。
  const settled = heldTrail.slice(-10);
  assert.ok(
    settled.every((y) => y === settled[0]),
    `holding against the hole edge must settle, not oscillate: ${settled.join(',')}`,
  );
  assert.strictEqual(held.vy, 0, 'holding against the hole edge must not accumulate velocity');

  // 2) 松手后带惯性甩进空洞：跑到停下为止，中途任何一帧都不许落在空洞里。
  let flung = {
    base: { x: 1000, y: 800 },
    x: 1000,
    y: 800,
    vx: 0,
    vy: 45, // 朝空洞猛甩
    tx: 1000,
    ty: 800,
    lastTx: 1000,
    lastTy: 800,
    holding: false,
    bounds,
  };
  let frames = 0;
  while (!flung.stopped && frames < 600) {
    flung = frame(flung);
    frames += 1;
    assert.ok(
      onAnyMonitor(petCenter(flung)),
      `inertia frame ${frames} must not strand the pet in the hole (y=${flung.y})`,
    );
  }
  assert.ok(flung.stopped, 'the fling must come to rest instead of bouncing forever');

  // 屏幕内的位置必须原样返回(同一对象)，避免每帧无谓地重建 state。
  const inside = { x: 1000, y: 500, vx: 2, vy: 2, holding: false };
  assert.strictEqual(
    drag.clampPetDragToDesktop(inside, { monitors: [short, tall], ...metrics }),
    inside,
    'a position already on a display must pass through untouched',
  );

  // 右屏独有的下半段(y>1080)是真实屏幕，不能被误判成空洞。
  const tallOnly = { x: 3000, y: 1200, vx: 0, vy: 0, holding: false };
  assert.strictEqual(
    drag.clampPetDragToDesktop(tallOnly, { monitors: [short, tall], ...metrics }),
    tallOnly,
    'the taller display must keep its exclusive lower band',
  );

  // 拿不到显示器信息时不要瞎猜，原样返回。
  const blind = { x: 1000, y: 1100, vx: 0, vy: 0, holding: false };
  assert.strictEqual(
    drag.clampPetDragToDesktop(blind, { monitors: [], ...metrics }),
    blind,
    'without monitor geometry the position must be left alone',
  );
}

// L 形布局：副屏在主屏上方且只占右半边，主屏左上方是空洞。
{
  const main = { position: { x: 0, y: 0 }, size: { width: 1920, height: 1080 }, scaleFactor: 1 };
  const above = { position: { x: 960, y: -1080 }, size: { width: 960, height: 1080 }, scaleFactor: 1 };
  const metrics = {
    size: { width: 350, height: 332 },
    alignment: 'left',
    characterWidth: 192,
    characterHeight: 208,
    horizontalPadding: 24,
    verticalPadding: 8,
  };
  // 目标人物中心 (200,-500)：在外接矩形内，但主屏上方、副屏左边——是空洞。
  const state = { x: 80, y: -720, vx: -4, vy: 0, holding: false };
  const fixed = drag.clampPetDragToDesktop(state, { monitors: [main, above], ...metrics });
  const cx = fixed.x + 120;
  const cy = fixed.y + 220;
  assert.ok(
    [main, above].some((m) => (
      cx >= m.position.x && cx < m.position.x + m.size.width
      && cy >= m.position.y && cy < m.position.y + m.size.height
    )),
    'an L-shaped layout must not strand the pet in the notch',
  );
}

{
  const current = {
    base: { x: 581, y: 50 },
    x: 581,
    y: 50,
    tx: 620,
    ty: 50,
    lastTx: 610,
    lastTy: 50,
    vx: 12,
    vy: 0,
  };
  const next = drag.rebasePetDragForAlignment(current, {
    from: 'left',
    to: 'right',
    windowWidth: 350,
    characterWidth: 192,
    horizontalPadding: 24,
  });
  const beforePetCenter = current.x + 24 + 192 / 2;
  const afterPetCenter = next.x + 350 - 24 - 192 / 2;
  assert.strictEqual(afterPetCenter, beforePetCenter, 'the pet must not jump when its card changes side');
  assert.strictEqual(next.tx - next.x, current.tx - current.x, 'spring displacement must stay continuous');
  assert.strictEqual(next.lastTx - next.x, current.lastTx - current.x, 'pointer motion delta must stay continuous');
  assert.strictEqual(next.base.x, 471, 'future pointer targets must use the rebased window origin');
  assert.strictEqual(next.vx, current.vx, 'alignment must not reverse or damp horizontal velocity');
  assert.strictEqual(current.x, 581, 'alignment rebasing must not mutate the input state');

  const roundTrip = drag.rebasePetDragForAlignment(next, {
    from: 'right',
    to: 'left',
    windowWidth: 350,
    characterWidth: 192,
    horizontalPadding: 24,
  });
  assert.deepStrictEqual(
    { x: roundTrip.x, tx: roundTrip.tx, lastTx: roundTrip.lastTx, baseX: roundTrip.base.x },
    { x: current.x, tx: current.tx, lastTx: current.lastTx, baseX: current.base.x },
    'repeated left/right crossings must not accumulate position drift',
  );
}

{
  const bounds = { l: -24, t: 0, r: 1000, b: 700 };
  assert.strictEqual(
    drag.petAlignmentAtDragEdge({
      currentAlignment: 'right',
      x: 80,
      tx: -24,
      holding: true,
      bounds,
    }),
    'left',
    'the card direction must change as soon as the held drag target touches the left edge',
  );
  assert.strictEqual(
    drag.petAlignmentAtDragEdge({
      currentAlignment: 'left',
      x: 900,
      tx: 1000,
      holding: true,
      bounds,
    }),
    'right',
    'the card direction must change as soon as the held drag target touches the right edge',
  );
  assert.strictEqual(
    drag.petAlignmentAtDragEdge({
      currentAlignment: 'right',
      x: 80,
      tx: -24,
      holding: false,
      bounds,
    }),
    'right',
    'released inertia must use the visible pet position instead of the stale pointer target',
  );
}

{
  const monitor = { position: { x: 0, y: 0 }, size: { width: 1200, height: 800 } };
  const bubbleBounds = drag.petElementHorizontalBounds({
    monitors: [monitor],
    monitor,
    localLeft: 34,
    localRight: 318,
  });
  assert.deepStrictEqual(
    bubbleBounds,
    { l: -34, r: 882 },
    'activity-card collision bounds must use the actual card edges inside the window',
  );
  assert.strictEqual(
    drag.petAlignmentAtDragEdge({
      currentAlignment: 'right',
      x: 50,
      tx: -34,
      holding: true,
      bounds: bubbleBounds,
    }),
    'left',
    'alignment must flip when the activity card touches the display, before the pet touches it',
  );
}

const viewSrc = path.join(here, '..', 'src', 'features', 'pet', 'PetWindow.jsx');
const viewCode = readFileSync(viewSrc, 'utf8');
assert.match(viewCode, /const DEFAULT_SCALE = 0\.5;/);
assert.match(
  viewCode,
  /useState\(startupScale\)/,
  'the first pet render must match the configured startup window',
);
assert.match(viewCode, /readPetDragContext\(T\)/);
assert.match(viewCode, /setPetWindowPosition\(T, drag\.win, drag\.x, drag\.y\)/);
assert.match(viewCode, /stepPetDrag\(drag\)/);
assert.match(viewCode, /dragAnimationFromMotion\(/);
assert.match(viewCode, /petEdgeAlignment\(/);
assert.match(viewCode, /petElementHorizontalBounds\(/);
assert.match(
  viewCode,
  /activityVisibleRef\.current\s*&&\s*!cardsCollapsedRef\.current\s*&&\s*activityCardRectRef\.current/,
  'a visually collapsed activity card must not affect drag edge alignment',
);
assert.match(viewCode, /rebasePetDragForAlignment\(/);
assert.doesNotMatch(viewCode, /win\.currentMonitor\(/);
assert.match(viewCode, /scaleFromResizeDrag\(/);
assert.match(
  viewCode,
  /function PetWindow\(\{ allowResize = true, configuredScale = null \}\)/,
);
assert.match(viewCode, /Number\.isFinite\(configuredScale\)/);
assert.match(viewCode, /useState\(startupScale\)/);
assert.match(viewCode, /invoke\('set_pet_scale',[\s\S]{0,120}?scale:\s*startupScale/);
assert.match(viewCode, /\{allowResize && \(\s*<div\s+className="pet-resize-grip"/);
// 右键菜单为窗口内 DOM 浮层(不再 invoke 原生菜单窗口:GB10/WebKitGTK 下
// 新起第二个透明窗口会 malloc 堆损坏闪退)。
assert.match(viewCode, /onContextMenu=\{onCharacterContextMenu\}/);
assert.match(viewCode, /const onCharacterContextMenu = \(event\) => \{/);
assert.match(viewCode, /setCtxMenu\(\{ x, y \}\)/);
assert.match(viewCode, /className="pet-context-menu"/);
assert.match(viewCode, /invoke\('set_pet_enabled',\s*\{\s*enabled:\s*false\s*\}\)/);
assert.match(viewCode, /if \(event\.button !== 0\) return;/);
assert.doesNotMatch(viewCode, /invoke\('show_pet_context_menu'/);
assert.doesNotMatch(viewCode, /invoke\('hide_pet_context_menu'/);
assert.match(viewCode, /petScreenAnchorFromRect\(/);
assert.match(viewCode, /anchor:\s*hasCharacterAnchor\s*\?\s*'character_top_left'/);
assert.match(viewCode, /anchorX:\s*hasCharacterAnchor\s*\?\s*drag\.anchorX/);
assert.match(viewCode, /ref=\{characterSlotRef\}/);
assert.match(viewCode, /persist:\s*pending\.persist/);
assert.match(viewCode, /queueResizeScale\(drag, next, false\)/);
assert.match(viewCode, /queueResizeScale\(drag, drag\.currentScale, true\)/);
assert.match(viewCode, /className="pet-resize-grip"/);
assert.match(viewCode, /className="pet-resize-grip-icon"/);
assert.match(viewCode, /d="M2 14H14V2"/);
assert.match(
  viewCode,
  /className="pet-character-slot"[\s\S]{0,2800}?\{allowResize && \([\s\S]{0,240}?className="pet-resize-grip"/,
  'the optional resize grip must live inside the scaled character slot',
);
assert.doesNotMatch(viewCode, /onWheel=/);

const cssSrc = path.join(here, '..', 'src', 'features', 'pet', 'pet.css');
const cssCode = readFileSync(cssSrc, 'utf8');
assert.match(cssCode, /overflow-y:\s*auto/);
assert.match(cssCode, /\.pet-resize-grip\s*\{/);
assert.doesNotMatch(cssCode, /\.pet-menu/);

assert.match(
  cssCode,
  /\.pet-character:focus\s*\{[\s\S]{0,80}?outline:\s*0;/,
  'mouse focus must not draw the WebView default black frame around the sprite',
);
assert.match(
  cssCode,
  /\.pet-character:focus-visible\s+\.pet-sprite\s*\{[\s\S]{0,120}?drop-shadow/,
  'keyboard focus should remain visible without a rectangular frame',
);
assert.match(
  cssCode,
  /\.pet-character-slot\s*>\s*\.pet-resize-grip\s*\{[\s\S]{0,140}?right:\s*6px;[\s\S]{0,80}?bottom:\s*0;/,
  'the grip should sit inside the transparent frame padding near the visible character edge',
);
assert.match(cssCode, /opacity:\s*0/);
assert.match(cssCode, /\.pet-root:hover\s+\.pet-resize-grip/);
assert.match(
  cssCode,
  /\.pet-resize-grip-icon\s*\{[\s\S]{0,220}?width:\s*16px;[\s\S]{0,100}?height:\s*16px;/,
  'the resize grip must render a complete inline SVG icon',
);

console.log('pet_interaction_logic: all assertions passed');
