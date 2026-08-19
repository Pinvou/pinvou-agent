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

const key = (patch = {}) => ({
  key: 'a',
  code: 'KeyA',
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
  { type: 'clear_pending' },
  'Alt+Space must not trigger direct voice task mode',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(key(), { status: 'idle', pendingAlt: true }),
  { type: 'clear_pending' },
  'pressing any other key while Alt is pending must cancel the plain-Alt trigger',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'idle', pendingSpace: true }),
  { type: 'pending_alt' },
  'legacy pending Space state must not turn Alt into task mode',
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
  { type: 'clear_pending' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'recording', mode: 'task' }),
  { type: 'pending_alt' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'recording', mode: 'task', pendingAlt: true }),
  { type: 'trigger', mode: 'task' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altSpace(), { status: 'recording', mode: 'task' }),
  { type: 'clear_pending' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'recording', mode: 'task', pendingSpace: true }),
  { type: 'pending_alt' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'recording', mode: 'edit', pendingAlt: true }),
  { type: 'trigger', mode: 'edit' },
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'recording', mode: 'structured', pendingAlt: true }),
  { type: 'trigger', mode: 'dictation' },
  'legacy structured recordings should stop through the dictation main path',
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
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'failed' }),
  { type: 'none' },
  'Escape must not be captured for failed voice notices',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'postprocessing' }),
  { type: 'cancel' },
  'Escape should still cancel active postprocessing voice work',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altSpace({ repeat: true }), { status: 'idle' }),
  { type: 'clear_pending' },
  'repeat filtering is handled before action mapping so tests can exercise pure key mapping',
);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ repeat: true })), true);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ defaultPrevented: true })), true);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ target: { tagName: 'TEXTAREA' } })), false);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ target: { tagName: 'DIV' } })), false);

console.log('voice_shortcut_state: ok');
