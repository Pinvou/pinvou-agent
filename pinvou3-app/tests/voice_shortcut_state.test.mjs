#!/usr/bin/env node
import assert from 'assert';
import {
  isVoiceShortcutIntroOpen,
  setVoiceShortcutIntroOpen,
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
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'requesting_permission' }),
  { type: 'cancel' },
  'Escape must cancel a pending permission request',
);
// Alt+Esc (system window cycling): an Esc passed through inside the combo is
// a combo member and must not cancel a recording; a bare Esc (no pending Alt)
// still cancels. Same policy as Alt+Tab (Other keys only clear pending).
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'recording', mode: 'task', pendingAlt: true }),
  { type: 'clear_pending' },
  'Escape inside an Alt combo passthrough must not cancel an active recording',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown({ key: 'Escape' }, { status: 'idle', pendingAlt: true }),
  { type: 'clear_pending' },
  'Escape inside an Alt combo passthrough must clear the pending gesture instead of ignoring it',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'requesting_permission', pendingAlt: true }),
  { type: 'cancel' },
  'releasing Alt while permission is pending must cancel instead of triggering again',
);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ repeat: true })), true);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ defaultPrevented: true })), true);
assert.strictEqual(
  shouldIgnoreVoiceShortcutEvent({ key: 'Escape', repeat: true }),
  true,
  'holding Escape must not storm cancel actions',
);
assert.strictEqual(
  shouldIgnoreVoiceShortcutEvent(key({ isComposing: true })),
  true,
  'IME composition keys must not trigger voice shortcuts',
);
assert.strictEqual(
  shouldIgnoreVoiceShortcutEvent({ key: 'Enter', keyCode: 229 }),
  true,
  'WKWebView late-dispatched IME keys (keyCode 229) must not trigger voice shortcuts',
);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ target: { tagName: 'TEXTAREA' } })), false);
assert.strictEqual(shouldIgnoreVoiceShortcutEvent(alt({ target: { tagName: 'DIV' } })), false);

assert.strictEqual(isVoiceShortcutIntroOpen(), false, 'intro-open flag must default off');
setVoiceShortcutIntroOpen(true);
assert.strictEqual(isVoiceShortcutIntroOpen(), true);
setVoiceShortcutIntroOpen(false);
assert.strictEqual(isVoiceShortcutIntroOpen(), false, 'intro-open flag must reset so Escape cancel works after the modal closes');

// Windows combo passthrough: the hook injects a synthetic Alt down after the
// bare combo keydown, so the page sees [combo down, Alt down (injected),
// combo up, real Alt up]. The tail keyup of the combo key while Alt is
// pending is that injected sequence's signature — it must clear the gesture
// so the trailing real Alt up cannot fire a ghost dictation start (or stop
// an active recording). A human tap never has another key's keyup between
// Alt down and Alt up.
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(key(), { status: 'idle', pendingAlt: true }),
  { type: 'clear_pending' },
  'combo tail keyup while Alt is pending must cancel the ghost tap',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(key(), { status: 'recording', pendingAlt: true }),
  { type: 'clear_pending' },
  'combo tail keyup while recording must not stop the recording',
);
// After the injected pair is dropped, the trailing real Alt up (no longer
// pending) must be inert.
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'idle', pendingAlt: false }),
  { type: 'none' },
);
// A genuine tap (Alt up while pending) must keep triggering.
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), { status: 'idle', pendingAlt: true }),
  { type: 'trigger', mode: 'dictation' },
);

// Right Alt shares key === 'Alt' but must never trigger: the Windows hook
// classifies VK_RMENU as Other ("右 Alt / AltGr 不触发语音快捷键"), so the
// JS gesture channel must agree — otherwise a bare right-Alt tap would fire
// dictation through the page lane the hook deliberately passes through.
const altRight = () => alt({ code: 'AltRight', location: 2 });
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(altRight(), { status: 'idle', pendingAlt: false }),
  { type: 'none' },
  'right Alt keydown must not start a pending gesture',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(altRight(), { status: 'idle', pendingAlt: true }),
  { type: 'clear_pending' },
  'right Alt keyup while pending must clear, not trigger (or stop a recording)',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt({ code: 'AltRight', location: 2, ctrlKey: true }), { status: 'idle', pendingAlt: false }),
  { type: 'none' },
  'AltGr (ctrl+right Alt) must stay inert',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), { status: 'idle', pendingAlt: false }),
  { type: 'pending_alt' },
  'left Alt keydown must keep arming the gesture',
);

// Reverse release order of the injected passthrough: [combo down, Alt down
// (injected), real Alt up, combo up]. The combo-up-first order is covered
// above; here the real Alt up arrives first, so the pending itself must
// carry the injected signature (an Alt down immediately after a non-Alt
// keydown) and clear instead of triggering. The signature window is
// scheduler slack only — a human cannot complete an Alt tap inside it.
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), {
    status: 'idle',
    pendingAlt: false,
    now: 1000,
    lastNonAltKeyDownAt: 990,
  }),
  { type: 'pending_alt', injected: true },
  'an Alt down right after a combo keydown must be marked as the injected sequence',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyUp(alt(), {
    status: 'idle',
    pendingAlt: true,
    pendingInjected: true,
  }),
  { type: 'clear_pending' },
  'real Alt up of an injected pending must clear instead of firing dictation',
);
assert.deepStrictEqual(
  voiceShortcutActionForKeyDown(alt(), {
    status: 'idle',
    pendingAlt: false,
    now: 1200,
    lastNonAltKeyDownAt: 1000,
  }),
  { type: 'pending_alt' },
  'an Alt down well after the last combo keydown is a genuine gesture start',
);

console.log('voice_shortcut_state: ok');
