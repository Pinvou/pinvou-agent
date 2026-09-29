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

// The recording-route stale purge must exempt requesting_permission: the
// claim now lands before the permission probe, so while the owner's permission
// dialog is up the target is legitimately not "recording" yet. Purging there
// would unregister a healthy claim; the intended semantic is that the routed
// Alt cancels the owner's pending start (fall through to trigger below).
const purge = source.indexOf("if (payload && payload.route === 'recording' && !recording");
const exemption = source.indexOf("status !== 'requesting_permission'", purge);
const fallthroughTrigger = source.indexOf("target.trigger('dictation');", purge);
assert.ok(purge !== -1 && exemption > purge, 'the stale-recording-route purge must exist with the requesting_permission exemption');
assert.ok(fallthroughTrigger > exemption, 'a recording-routed Alt during the permission probe must cancel the pending start, not purge the claim');

// The ownership-token drop must be gated on an ACTIVE recording, and the gate
// must sit before the token check: an idle window whose held token no longer
// matches the routed one must fall through to the purge above — with the
// token from an earlier recording still on voiceInput, an ungated drop would
// dead-end on a stale native registration that only that purge can release.
const tokenDrop = source.indexOf('voiceInput.ownershipToken !== payload.recording_token');
const tokenGate = source.indexOf("payload.route === 'recording' && recording");
const dropReturn = source.indexOf(') return;', tokenDrop);
assert.ok(tokenDrop !== -1, 'the recording-route token check must exist');
assert.ok(tokenGate !== -1 && tokenGate < tokenDrop, 'the token check must be gated on an active recording');
assert.ok(dropReturn > tokenDrop, 'the gated token check must still drop the stale gesture');

console.log('voice_shortcut_router: ok');
