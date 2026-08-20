import assert from 'node:assert/strict';
import test from 'node:test';

import {
  activateVoiceControl,
  getVoiceControlState,
  isVoiceCaptureActive,
} from '../src/features/pinvou_os/pinvou-os-voice-control.js';

function createVoiceSpy() {
  const calls = [];
  return {
    calls,
    voice: {
      cancelVoiceInput() {
        calls.push('cancel');
      },
      startVoiceInput() {
        calls.push('start');
      },
    },
  };
}

function clickMicrophone(bs, spy, beginCalls) {
  const control = getVoiceControlState(bs.voiceInput.status, spy.voice);
  const activated = activateVoiceControl(control, spy.voice, () => beginCalls.push('begin'));
  return { control, activated };
}

test('backend busy never locks starting or stopping voice input', () => {
  const spy = createVoiceSpy();
  const beginCalls = [];

  const idle = clickMicrophone({ busy: true, voiceInput: { status: 'idle' } }, spy, beginCalls);
  assert.equal(idle.control.disabled, false);
  assert.equal(idle.activated, true);
  assert.deepEqual(beginCalls, ['begin'], 'busy idle state must still start an interjection');

  const recording = clickMicrophone({ busy: true, voiceInput: { status: 'recording' } }, spy, beginCalls);
  assert.equal(recording.control.disabled, false);
  assert.equal(recording.activated, true);
  assert.deepEqual(spy.calls, ['start'], 'recording click must use the bridge stop-and-transcribe protocol');
});

test('permission request and transcription stay cancellable while backend is busy', () => {
  const spy = createVoiceSpy();
  const beginCalls = [];

  for (const status of ['requesting_permission', 'transcribing']) {
    const result = clickMicrophone({ busy: true, voiceInput: { status } }, spy, beginCalls);
    assert.equal(result.control.disabled, false, `${status} must remain clickable`);
    assert.equal(result.activated, true);
  }

  assert.deepEqual(spy.calls, ['cancel', 'cancel']);
  assert.deepEqual(beginCalls, []);
});

test('voice control calls only the bridge owner for cancellation and disables missing capabilities', () => {
  const spy = createVoiceSpy();
  const control = getVoiceControlState('transcribing', spy.voice);

  activateVoiceControl(control, spy.voice, () => assert.fail('cancel must not start another session'));
  assert.deepEqual(spy.calls, ['cancel'], 'one click must delegate cleanup exactly once');

  const unavailable = getVoiceControlState('recording', { cancelVoiceInput() {} });
  assert.equal(unavailable.disabled, true);
  assert.equal(activateVoiceControl(unavailable, {}, () => {}), false);
});

test('background answers are deferred for the complete voice capture lifecycle', () => {
  for (const status of ['requesting_permission', 'recording', 'transcribing']) {
    assert.equal(isVoiceCaptureActive(status), true, `${status} must keep capture presentation priority`);
  }
  for (const status of ['idle', 'completed', 'cancelled', 'failed']) {
    assert.equal(isVoiceCaptureActive(status), false, `${status} may reveal deferred background content`);
  }
});
