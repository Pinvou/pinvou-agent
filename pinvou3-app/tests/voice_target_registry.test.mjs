#!/usr/bin/env node
import assert from 'assert';
import {
  clearActiveVoiceTarget,
  getActiveVoiceTarget,
  isActiveVoiceTarget,
  registerVoiceTarget,
} from '../src/features/voice-composer/voice-target-registry.mjs';

clearActiveVoiceTarget();

const calls = [];
const unregisterChat = registerVoiceTarget({
  targetId: 'chat-composer',
  ownerKind: 'chat',
  voiceSessionId: 'chat-voice-1',
  workspaceId: 'chat-workspace',
  sessionId: 'chat-session',
  isStillActive: () => true,
  trigger: mode => calls.push(`chat:${mode}`),
  cancel: () => calls.push('chat:cancel'),
});

assert.strictEqual(getActiveVoiceTarget().targetId, 'chat-composer');
assert.strictEqual(isActiveVoiceTarget('chat-composer', 'chat-voice-1'), true);

const unregisterCodex = registerVoiceTarget({
  targetId: 'codex-composer',
  ownerKind: 'codex',
  voiceSessionId: 'codex-voice-1',
  workspaceId: 'C:/repo',
  sessionId: 'codex-session',
  isStillActive: () => true,
  trigger: mode => calls.push(`codex:${mode}`),
  cancel: () => calls.push('codex:cancel'),
});

assert.strictEqual(getActiveVoiceTarget().targetId, 'codex-composer');
assert.strictEqual(isActiveVoiceTarget('chat-composer', 'chat-voice-1'), false);
assert.strictEqual(isActiveVoiceTarget('codex-composer', 'codex-voice-1'), true);
assert.strictEqual(isActiveVoiceTarget('codex-composer', 'stale-voice'), false);

getActiveVoiceTarget().trigger('task');
assert.deepStrictEqual(calls, ['codex:task']);

unregisterChat();
assert.strictEqual(getActiveVoiceTarget().targetId, 'codex-composer', 'old target cleanup must not clear the newer active target');

unregisterCodex();
assert.strictEqual(getActiveVoiceTarget(), null);

registerVoiceTarget({
  targetId: 'inactive-composer',
  ownerKind: 'chat',
  voiceSessionId: 'inactive-voice',
  isStillActive: () => false,
  trigger: () => calls.push('inactive'),
  cancel: () => {},
});
assert.strictEqual(isActiveVoiceTarget('inactive-composer', 'inactive-voice'), false);
clearActiveVoiceTarget();

console.log('voice_target_registry: ok');
