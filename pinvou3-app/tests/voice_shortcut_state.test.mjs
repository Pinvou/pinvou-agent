#!/usr/bin/env node
import assert from 'assert';
import {
  shouldIgnoreVoiceShortcutEvent,
  voiceShortcutActionForKeyDown,
  voiceShortcutActionForKeyUp,
} from '../src/features/chat/voice-shortcut-state.mjs';

const alt = (patch = {}) => ({
  key: 'Alt',
  code: 'AltLeft',
  altKey: true,
  ctrlKey: false,
  shiftKey: false,
  metaKey: false,
  repeat: false,
  ...patch,
});

const altSpace = (patch = {}) => ({
  key: ' ',
  code: 'Space',
  altKey: true,
  ctrlKey: false,
  shiftKey: false,
  metaKey: false,
  repeat: false,
  ...patch,
});

assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'idle' }),
  { type: 'pending_alt' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'idle', pendingAlt: true }),
  { type: 'pending_alt' },
  'holding Alt should remain pending until keyup instead of starting recording',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'idle', pendingAlt: true }),
  { type: 'trigger', mode: 'dictation' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'idle', pendingAlt: false }),
  { type: 'none' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altSpace(), { status: 'idle' }),
  { type: 'trigger', mode: 'task' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'idle', pendingSpace: true }),
  { type: 'trigger', mode: 'task' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'recording', mode: 'dictation' }),
  { type: 'pending_alt' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'recording', mode: 'dictation', pendingAlt: true }),
  { type: 'trigger', mode: 'dictation' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altSpace(), { status: 'recording', mode: 'dictation' }),
  { type: 'none' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'recording', mode: 'task' }),
  { type: 'none' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'recording', mode: 'task', pendingAlt: true }),
  { type: 'none' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altSpace(), { status: 'recording', mode: 'task' }),
  { type: 'trigger', mode: 'task' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'recording', mode: 'task', pendingSpace: true }),
  { type: 'trigger', mode: 'task' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'recording', mode: 'task' }),
  { type: 'cancel' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'idle' }),
  { type: 'none' },
  'Escape must not be treated as a global idle shortcut',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altSpace({ repeat: true }), { status: 'idle' }),
  { type: 'trigger', mode: 'task' },
  'repeat filtering is handled before action mapping so tests can exercise pure key mapping',
);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ repeat: true })), true);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ defaultPrevented: true })), true);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ target: { tagName: 'TEXTAREA' } })), false);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ target: { tagName: 'DIV' } })), false);

console.log('voice_shortcut_state: ok');
