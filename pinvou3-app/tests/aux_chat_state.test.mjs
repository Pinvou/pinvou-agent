/** Aux-chat pure-logic contract: snapshot normalization, busy/empty checks and turns projection. */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import {
  auxChatBusy,
  auxChatHasContent,
  auxSnapshotsEqual,
  normalizeAuxSnapshot,
  projectAuxChatTurns,
} from '../src/features/aux-chat/aux-chat-state.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));

test('normalizeAuxSnapshot returns an empty structure for invalid input', () => {
  for (const raw of [null, undefined, 42, 'aux-x', { chatItems: 'nope' }]) {
    const snap = normalizeAuxSnapshot(raw);
    assert.deepEqual(snap, { chatItems: [], busy: false, queued: [] });
  }
  const snap = normalizeAuxSnapshot({ chatItems: [{ type: 'user' }], busy: 1, queued: [{}] });
  assert.equal(snap.chatItems.length, 1);
  assert.equal(snap.busy, true);
  assert.equal(snap.queued.length, 1);
});

test('auxChatBusy uses the same criteria as the real bridge send busy-rejection branch', () => {
  assert.equal(auxChatBusy({ chatItems: [], busy: false, queued: [] }), false);
  assert.equal(auxChatBusy({ chatItems: [], busy: true, queued: [] }), true);
  assert.equal(auxChatBusy({ chatItems: [], busy: false, queued: [{ id: 1 }] }), true);
  assert.equal(auxChatBusy(null), false);
});

// The bridge's busy rejection previously only had a "same-criteria" name
// claim and was never executed (the existing harness's isBusyFor is always
// false): here we drive the real aux-chat.js factory, set busy / queued, and
// assert send throws turnAlreadyInProgress — the panel-level auxChatBusy
// precheck and the bridge's rejection must cover the same set of states.
// Since M7 the factory delegates the domain bodies to the shared lane, so the
// harness loads bridge-shared-helpers.js into the same context first (the
// production script order in index.html).
function loadTauriAuxChatWithBusy({ busyFor, queued }) {
  const root = { __PINVOU_SHARED_I18N__: {} };
  const sharedSrc = fs.readFileSync(
    path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'),
    'utf8',
  );
  const src = fs.readFileSync(
    path.join(here, '..', 'src', 'platform', 'tauri', 'bridge', 'aux-chat.js'),
    'utf8',
  );
  const context = { window: root, globalThis: root, setTimeout, clearTimeout };
  vm.runInNewContext(sharedSrc, context);
  vm.runInNewContext(src, context);
  return root.__PINVOU_TAURI_BRIDGE_FEATURES__.auxChat({
    state: { activeSessionId: null },
    sessionStates: queued ? { 'aux-1': { queued: [{ id: 1 }] } } : {},
    bt: key => key,
    invoke: async () => ({}),
    ensureSessionBufferLoaded: async () => {},
    purgeSessionBuffer() {},
    touchSessionBuffer() {},
    isBusyFor: busyFor,
  });
}

test('the real bridge send rejects when busy or queued (turnAlreadyInProgress)', async () => {
  const busyChat = loadTauriAuxChatWithBusy({ busyFor: () => true, queued: false });
  await assert.rejects(
    busyChat.send('aux-1', 'q'),
    /turnAlreadyInProgress/u,
    'a busy session must be rejected by the bridge',
  );
  const queuedChat = loadTauriAuxChatWithBusy({ busyFor: () => false, queued: true });
  await assert.rejects(
    queuedChat.send('aux-1', 'q'),
    /turnAlreadyInProgress/u,
    'a session with queued messages must be rejected by the bridge',
  );
});

test('auxChatHasContent counts only user/assistant items as content', () => {
  assert.equal(auxChatHasContent(null), false);
  assert.equal(auxChatHasContent({ chatItems: [] }), false);
  assert.equal(auxChatHasContent({ chatItems: [{ type: 'system', text: 's' }] }), false);
  assert.equal(auxChatHasContent({ chatItems: [{ type: 'user', text: 'q' }] }), true);
  assert.equal(auxChatHasContent({ chatItems: [{ type: 'assistant', text: 'a' }] }), true);
});

test('projectAuxChatTurns projects a user+assistant snapshot into conversation turns', () => {
  const snapshot = {
    chatItems: [
      { id: 1, type: 'user', text: 'what is the aux chat?' },
      { id: 2, type: 'assistant', text: 'an independent Q&A session.' },
    ],
    busy: false,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-01');
  assert.equal(turns.length, 1);
  assert.equal(turns[0].userText, 'what is the aux chat?');
  const assistant = turns[0].items.find((item) => item.type === 'agent_message');
  assert.ok(assistant, 'assistant items must project to agent_message');
  assert.equal(assistant.text, 'an independent Q&A session.');
  assert.equal(assistant.status, 'completed');
  assert.equal(turns[0].status, 'completed');
});

test('projectAuxChatTurns marks the trailing turn as running while busy', () => {
  const snapshot = {
    chatItems: [
      { id: 1, type: 'user', text: 'q' },
      { id: 2, type: 'assistant', text: 'streaming', streaming: true },
    ],
    busy: true,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-01');
  assert.equal(turns.length, 1);
  assert.equal(turns[0].status, 'running');
  assert.equal(turns[0].completedAt, null);
});

test('projectAuxChatTurns returns empty turns for an empty snapshot', () => {
  assert.deepEqual(projectAuxChatTurns(null, 'aux-01'), []);
  assert.deepEqual(projectAuxChatTurns({ chatItems: [] }, null), []);
});

test('auxSnapshotsEqual judges re-pulled snapshots with identical content as equal', () => {
  const item = { id: 1, type: 'user', text: 'q' };
  const prev = normalizeAuxSnapshot({ chatItems: [{ ...item }], busy: false, queued: [] });
  // The bridge shallow-copies each item on every snapshot() (streaming deltas
  // mutate buffer items in place, so only after copying is a field-wise
  // compare a real content compare): a re-pulled snapshot with unchanged
  // content has different item references but identical fields — it must
  // compare equal so the panel can skip the re-render.
  const next = normalizeAuxSnapshot({ chatItems: [{ ...item }], busy: false, queued: [] });
  assert.equal(auxSnapshotsEqual(prev, next), true);
  assert.notEqual(prev.chatItems[0], next.chatItems[0]);
  assert.equal(auxSnapshotsEqual(prev, prev), true);
  assert.equal(auxSnapshotsEqual(null, null), true);
  assert.equal(auxSnapshotsEqual(null, { chatItems: [] }), true);
});

test('auxSnapshotsEqual catches streaming in-place mutation and busy/queued changes', () => {
  // Real buffer semantics: the same item object is mutated in place by a
  // streaming delta, and two pulls each go through the bridge's per-item copy
  // and produce new objects with different content — they must compare
  // unequal, or the panel freezes on the first frame. (Without the in-bridge
  // copy the two pulls share one reference and auxItemsEqual's reference
  // shortcut would wrongly judge them equal — exactly the point the
  // "bridge must copy per item" behavior test in session_buffer_eviction
  // pins.)
  const bufferItem = { id: 1, type: 'assistant', text: 'stream', streaming: true };
  const prev = normalizeAuxSnapshot({ chatItems: [{ ...bufferItem }], busy: true, queued: [] });
  bufferItem.text = 'streaming';
  const streamed = normalizeAuxSnapshot({ chatItems: [{ ...bufferItem }], busy: true, queued: [] });
  assert.equal(auxSnapshotsEqual(prev, streamed), false);
  assert.equal(auxSnapshotsEqual(prev, normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: 'stream', streaming: true }],
    busy: false,
    queued: [],
  })), false);
  assert.equal(auxSnapshotsEqual(prev, normalizeAuxSnapshot({
    chatItems: [
      { id: 1, type: 'assistant', text: 'stream', streaming: true },
      { id: 2, type: 'user', text: 'q2' },
    ],
    busy: true,
    queued: [],
  })), false);
  assert.equal(auxSnapshotsEqual(prev, normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: 'stream', streaming: true }],
    busy: true,
    queued: [{ id: 9 }],
  })), false);
});

test('auxSnapshotsEqual catches snapshots differing only in key set (incrementally written html)', () => {
  // The streaming bridge writes `text` and `html` incrementally (round-26
  // minor M10): two pulls can differ ONLY in key set. auxItemsEqual's
  // key-count guard is the single line that catches this — deleting it kept
  // the whole suite green while the panel froze on the first frame, so the
  // case is pinned explicitly here.
  const textOnly = normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: 'stream' }],
    busy: true,
    queued: [],
  });
  const withHtml = normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: 'stream', html: '<p>stream</p>' }],
    busy: true,
    queued: [],
  });
  assert.equal(auxSnapshotsEqual(textOnly, withHtml), false);
  assert.equal(auxSnapshotsEqual(withHtml, textOnly), false);
});
