#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const src = path.join(here, '..', 'src', 'features', 'pet', 'pet-card-state.js');
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-pet-card-state-'));
const tmp = path.join(dir, 'pet-card-state.mjs');

try {
  copyFileSync(src, tmp);
  const cards = await import(`${pathToFileURL(tmp).href}?t=${Date.now()}`);

  const initial = cards.createPetCardUiState();
  const expanded = cards.petCardUiReducer(initial, { type: 'toggle-expand', sessionId: 'a' });
  assert.equal(expanded.expandedSessionId, 'a');
  assert.equal(initial.expandedSessionId, null, 'the reducer must not mutate input state');
  assert.equal(
    cards.petCardUiReducer(expanded, { type: 'toggle-expand', sessionId: 'a' }).expandedSessionId,
    null,
  );

  let reply = cards.petCardUiReducer(initial, { type: 'open-reply', sessionId: 'a' });
  reply = cards.petCardUiReducer(reply, { type: 'edit-reply', text: '  继续检查这个问题  ' });
  assert.equal(cards.normalizedPetReply(reply.draft), '继续检查这个问题');
  reply = cards.petCardUiReducer(reply, { type: 'submit-reply', requestId: 'req-1' });
  assert.equal(reply.pendingRequestId, 'req-1');

  const ignored = cards.petCardUiReducer(reply, {
    type: 'reply-accepted', requestId: 'another-request',
  });
  assert.equal(ignored.pendingRequestId, 'req-1');

  const failed = cards.petCardUiReducer(reply, {
    type: 'reply-failed', requestId: 'req-1', error: '会话不存在',
  });
  assert.equal(failed.draft, '  继续检查这个问题  ');
  assert.equal(failed.pendingRequestId, null);
  assert.equal(failed.error, '会话不存在');

  const accepted = cards.petCardUiReducer(reply, {
    type: 'reply-accepted', requestId: 'req-1',
  });
  assert.equal(accepted.replySessionId, null);
  assert.equal(accepted.draft, '');
  assert.equal(accepted.pendingRequestId, null);

  const restored = cards.petCardUiReducer(accepted, {
    type: 'reply-failed', requestId: 'req-1', error: 'engine unavailable',
  });
  assert.equal(restored.replySessionId, 'a');
  assert.equal(restored.draft, '  继续检查这个问题  ');
  assert.equal(restored.error, 'engine unavailable');

  const acceptedThenDismissed = cards.petCardUiReducer(accepted, {
    type: 'dismiss', sessionId: 'a',
  });
  assert.equal(acceptedThenDismissed.retrySubmission, null);
  assert.strictEqual(
    cards.petCardUiReducer(acceptedThenDismissed, {
      type: 'reply-failed', requestId: 'req-1', error: 'late failure',
    }),
    acceptedThenDismissed,
    'a late failure must not revive a reply after its activity was dismissed',
  );

  const dismissed = cards.petCardUiReducer(
    { ...reply, expandedSessionId: 'a' },
    { type: 'dismiss', sessionId: 'a' },
  );
  assert.equal(dismissed.expandedSessionId, null);
  assert.equal(dismissed.replySessionId, null);
  assert.equal(dismissed.draft, '');

  console.log('pet card state logic tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
