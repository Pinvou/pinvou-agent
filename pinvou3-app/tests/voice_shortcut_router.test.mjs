#!/usr/bin/env node
// The router component has no DOM harness in this repo's node --test setup, so
// the keydown dispatch order is pinned on the source: the swallow:false early
// return must precede the preventDefault pair, otherwise human Alt/Option combo
// keydowns get swallowed again (the #470 regression where right-Option + letter
// lost the macOS symbol). The keyup lane's clear_pending-before-preventDefault
// order is pinned too, so the two lanes cannot silently diverge again.
import assert from 'assert';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const source = fs.readFileSync(
  path.join(testDir, '..', 'src', 'features', 'voice-composer', 'VoiceShortcutRouter.jsx'),
  'utf8',
);

const keydownStart = source.indexOf('function handleVoiceShortcutKeyDown');
const keyupStart = source.indexOf('function handleVoiceShortcutKeyUp');
const listenersLine = source.indexOf("window.addEventListener('keydown'");
assert.ok(keydownStart !== -1 && keyupStart !== -1 && listenersLine !== -1, 'router key handlers must exist');
assert.ok(keydownStart < keyupStart && keyupStart < listenersLine, 'handler layout changed; update this test');

const keydown = source.slice(keydownStart, keyupStart);
const swallowGuard = keydown.indexOf('action.swallow === false');
const keydownPreventDefault = keydown.indexOf('event.preventDefault();');
assert.ok(swallowGuard !== -1, 'keydown lane must keep the swallow:false passthrough guard');
assert.ok(keydownPreventDefault !== -1, 'keydown lane must keep preventDefault for swallowable actions');
assert.ok(
  swallowGuard < keydownPreventDefault,
  'combo-member keydowns must return before preventDefault so Alt combos keep their own behavior',
);

const keyup = source.slice(keyupStart, listenersLine);
const keyupClearPending = keyup.indexOf("action.type === 'clear_pending'");
const keyupPreventDefault = keyup.indexOf('event.preventDefault();');
assert.ok(keyupClearPending !== -1 && keyupPreventDefault !== -1, 'keyup lane guards must exist');
assert.ok(
  keyupClearPending < keyupPreventDefault,
  'keyup clear_pending must keep passing combo tails through without preventDefault',
);

console.log('voice_shortcut_router: ok');
