#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  DRAG_THRESHOLD_BY_POINTER,
  FLOATING_VOICE_CLICK_SUPPRESSION_MS,
  canStartFloatingVoiceDrag,
  clearFloatingVoiceDragClick,
  consumeFloatingVoiceDragClick,
  createFloatingVoiceDragSession,
  finishFloatingVoiceDrag,
  moveFloatingVoiceDrag,
} from '../src/features/chat/floating-voice-drag.mjs';

assert.equal(FLOATING_VOICE_CLICK_SUPPRESSION_MS, 800);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'mouse', isPrimary: true, button: 0 }), true);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'mouse', isPrimary: false, button: 0 }), false);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'pen', isPrimary: true, button: 0 }), true);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'pen', isPrimary: false, button: 0 }), false);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'pen', isPrimary: true, button: 2 }), false);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'touch', isPrimary: true, button: 0 }), true);
assert.equal(canStartFloatingVoiceDrag({ pointerType: 'touch', isPrimary: false, button: 0 }), false);
assert.equal(canStartFloatingVoiceDrag({ pointerType: '', isPrimary: true, button: 0 }), false);

const mouse = createFloatingVoiceDragSession({
  pointerId: 7,
  pointerType: 'mouse',
  clientX: 100,
  clientY: 100,
  offsetX: 20,
  offsetY: 24,
});
assert.equal(DRAG_THRESHOLD_BY_POINTER.mouse, 4);
assert.deepEqual(
  moveFloatingVoiceDrag(mouse, { pointerId: 7, clientX: 103, clientY: 100, buttons: 1 }),
  { kind: 'pending' },
);
assert.equal(mouse.dragging, false);
assert.deepEqual(
  moveFloatingVoiceDrag(mouse, { pointerId: 7, clientX: 106, clientY: 108, buttons: 1 }),
  { kind: 'move', started: true, x: 86, y: 84 },
);
assert.equal(mouse.dragging, true);
assert.equal(mouse.suppressClick, false, 'movement alone must not suppress unrelated activations');
assert.deepEqual(
  moveFloatingVoiceDrag(mouse, { pointerId: 7, clientX: 112, clientY: 116, buttons: 1 }),
  { kind: 'move', started: false, x: 92, y: 92 },
);
assert.deepEqual(finishFloatingVoiceDrag(mouse, 8), { matched: false, wasDragging: false });
assert.deepEqual(
  finishFloatingVoiceDrag(mouse, 7, { suppressCompatibleClick: true }),
  { matched: true, wasDragging: true },
);
assert.equal(mouse.pointerId, null);
assert.equal(mouse.dragging, false);
assert.equal(
  consumeFloatingVoiceDragClick(mouse, { detail: 0, pointerId: 7, pointerType: 'mouse' }),
  false,
  'keyboard and assistive-technology activation must not be consumed',
);
assert.equal(
  consumeFloatingVoiceDragClick(mouse, { detail: 1, pointerId: 8, pointerType: 'mouse' }),
  false,
  'a click from another pointer must not be consumed',
);
assert.equal(
  consumeFloatingVoiceDragClick(mouse, { detail: 1, pointerId: 7, pointerType: 'mouse' }),
  true,
  'the matching compatibility click after a drag must be consumed once',
);
assert.equal(consumeFloatingVoiceDragClick(mouse, { detail: 1, pointerId: 7, pointerType: 'mouse' }), false);

const lostMouseButton = createFloatingVoiceDragSession({
  pointerId: 9,
  pointerType: 'mouse',
  clientX: 10,
  clientY: 10,
  offsetX: 2,
  offsetY: 2,
});
assert.deepEqual(
  moveFloatingVoiceDrag(lostMouseButton, { pointerId: 9, clientX: 12, clientY: 12, buttons: 0 }),
  { kind: 'released' },
);

// buttons-released 路径:拖动已发生(dragging=true)时,released 后调用方
// 应以 suppressCompatibleClick 结束会话,后续兼容 click 必须被消费。
const releasedDrag = createFloatingVoiceDragSession({
  pointerId: 9,
  pointerType: 'mouse',
  clientX: 10,
  clientY: 10,
  offsetX: 2,
  offsetY: 2,
});
assert.equal(moveFloatingVoiceDrag(releasedDrag, { pointerId: 9, clientX: 16, clientY: 16, buttons: 1 }).kind, 'move');
assert.equal(releasedDrag.dragging, true);
assert.deepEqual(
  moveFloatingVoiceDrag(releasedDrag, { pointerId: 9, clientX: 18, clientY: 18, buttons: 0 }),
  { kind: 'released' },
);
assert.deepEqual(
  finishFloatingVoiceDrag(releasedDrag, 9, { suppressCompatibleClick: true }),
  { matched: true, wasDragging: true },
);
assert.equal(releasedDrag.suppressClick, true, 'dragged buttons-released must suppress the compatibility click');
assert.equal(
  consumeFloatingVoiceDragClick(releasedDrag, { detail: 1, pointerId: 9, pointerType: 'mouse' }),
  true,
  'the compatibility click after a buttons-released drag must be consumed once',
);
assert.equal(consumeFloatingVoiceDragClick(releasedDrag, { detail: 1, pointerId: 9, pointerType: 'mouse' }), false);

const lostPenButton = createFloatingVoiceDragSession({
  pointerId: 10,
  pointerType: 'pen',
  clientX: 10,
  clientY: 10,
  offsetX: 2,
  offsetY: 2,
});
assert.deepEqual(
  moveFloatingVoiceDrag(lostPenButton, { pointerId: 10, clientX: 18, clientY: 10, buttons: 0 }),
  { kind: 'released' },
);

const touch = createFloatingVoiceDragSession({
  pointerId: 11,
  pointerType: 'touch',
  clientX: 50,
  clientY: 50,
  offsetX: 10,
  offsetY: 10,
});
assert.deepEqual(
  moveFloatingVoiceDrag(touch, { pointerId: 11, clientX: 57, clientY: 50, buttons: 1 }),
  { kind: 'pending' },
);
assert.equal(moveFloatingVoiceDrag(touch, { pointerId: 11, clientX: 58, clientY: 50, buttons: 1 }).kind, 'move');
clearFloatingVoiceDragClick(touch);
assert.equal(touch.suppressClick, false);

const lostCapture = createFloatingVoiceDragSession({
  pointerId: 12,
  pointerType: 'mouse',
  clientX: 20,
  clientY: 20,
  offsetX: 4,
  offsetY: 4,
});
assert.equal(moveFloatingVoiceDrag(lostCapture, { pointerId: 12, clientX: 28, clientY: 20, buttons: 1 }).kind, 'move');
assert.deepEqual(
  finishFloatingVoiceDrag(lostCapture, 12, { suppressCompatibleClick: true }),
  { matched: true, wasDragging: true },
);
assert.deepEqual(finishFloatingVoiceDrag(lostCapture, 12), { matched: false, wasDragging: false });
assert.equal(
  consumeFloatingVoiceDragClick(lostCapture, { detail: 1, pointerId: 12, pointerType: 'mouse' }),
  true,
);

const cancelled = createFloatingVoiceDragSession({
  pointerId: 13,
  pointerType: 'touch',
  clientX: 20,
  clientY: 20,
  offsetX: 4,
  offsetY: 4,
});
assert.equal(moveFloatingVoiceDrag(cancelled, { pointerId: 13, clientX: 30, clientY: 20, buttons: 1 }).kind, 'move');
assert.deepEqual(finishFloatingVoiceDrag(cancelled, 13), { matched: true, wasDragging: true });
assert.equal(cancelled.suppressClick, false, 'pointercancel and blur must not leave click suppression behind');

const testDir = path.dirname(fileURLToPath(import.meta.url));
const chatSource = fs.readFileSync(path.join(testDir, '..', 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const pointerDownBlock = chatSource.slice(
  chatSource.indexOf('      function handleFloatingVoicePointerDown(e) {'),
  chatSource.indexOf('\n      function handleFloatingVoicePointerMove(e) {'),
);
assert.match(pointerDownBlock, /target\.setPointerCapture\(e\.pointerId\)/);
assert.match(pointerDownBlock, /canStartFloatingVoiceDrag\(e\)/);
assert.ok(pointerDownBlock.indexOf('activeDrag.pointerId !== null') < pointerDownBlock.indexOf('canStartFloatingVoiceDrag(e)'));
assert.doesNotMatch(pointerDownBlock, /'replaced'/);
assert.doesNotMatch(pointerDownBlock, /setTimeout|360/);
assert.match(chatSource, /onLostPointerCapture=\{handleFloatingVoiceLostPointerCapture\}/);
assert.match(chatSource, /window\.addEventListener\('pointerup', finishFromWindow, true\)/);
assert.match(chatSource, /window\.addEventListener\('blur', finishOnBlur\)/);
assert.match(chatSource, /data-pressed=\{floatingVoicePressed \? 'true' : 'false'\}/);
assert.doesNotMatch(chatSource, /active:scale-95/);
assert.match(chatSource, /}, FLOATING_VOICE_CLICK_SUPPRESSION_MS\);/);
assert.match(chatSource, /onClick=\{handleFloatingVoiceClick\}/);
assert.match(chatSource, /data-testid="composer-voice-button"/);
assert.match(chatSource, /(?:reason === 'pointerup' \|\| reason === 'lostpointercapture' \|\| reason === 'buttons-released'|\['pointerup', 'lostpointercapture', 'buttons-released'\]\.includes\(reason\))/);

console.log('floating_voice_drag_logic: ok');
