import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import {
  createAcpSessionOperationTracker,
  removeAcpDraftItems,
  transferAcpDraftItems,
} from '../src/features/codex/acp-session-operation.js';

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

{
  const tracker = createAcpSessionOperationTracker('session-A');
  const responseA = deferred();
  const operationA = tracker.begin('session-A', 'send');
  const state = { draft: '', error: '', sessionBWorking: false };
  const settleA = responseA.promise.catch(error => {
    if (tracker.isCurrent(operationA)) {
      state.draft = 'message from A';
      state.error = error.message;
    }
  }).finally(() => {
    if (tracker.finish(operationA)) state.sessionBWorking = false;
  });

  tracker.switchSession('session-B');
  const operationB = tracker.begin('session-B', 'send');
  state.draft = 'draft from B';
  state.sessionBWorking = true;
  responseA.reject(new Error('A failed late'));
  await settleA;

  assert.equal(state.draft, 'draft from B', 'A failure must not restore its message into B');
  assert.equal(state.error, '', 'A failure must not show an error in B');
  assert.equal(state.sessionBWorking, true, 'A finally must not finish B sending state');
  assert.equal(tracker.isCurrent(operationB), true);
}

{
  const tracker = createAcpSessionOperationTracker('__codex_draft__');
  const createA = deferred();
  let activeSessionId = null;
  let operationA = tracker.begin('__codex_draft__', 'send');
  const settleCreateA = createA.promise.then(createdSessionId => {
    if (activeSessionId === createdSessionId) {
      tracker.switchSession(createdSessionId);
      operationA = tracker.begin(createdSessionId, 'send');
    }
  });

  activeSessionId = 'session-B';
  tracker.switchSession(activeSessionId);
  const operationB = tracker.begin('session-B', 'send');
  createA.resolve('session-A');
  await settleCreateA;

  assert.equal(tracker.begin('session-A', 'send'), null,
    'the tracker must reject begin for a non-active Session');
  assert.equal(tracker.isCurrent(operationB), true,
    'a late draft Session creation must not replace B current send operation');
  assert.equal(tracker.isCurrent(operationA), false);
}

{
  const tracker = createAcpSessionOperationTracker('session-A');
  const responseA = deferred();
  let visibleInfo = null;
  const operationA = tracker.begin('session-A', 'send');
  const settleA = responseA.promise.then(info => {
    if (tracker.isCurrent(operationA)) visibleInfo = info;
    tracker.finish(operationA);
  });

  tracker.switchSession('session-B');
  const operationB = tracker.begin('session-B', 'send');
  const infoB = { sessionId: 'session-B', mode: 'acceptEdits' };
  if (tracker.isCurrent(operationB)) visibleInfo = infoB;
  tracker.finish(operationB);

  tracker.switchSession('session-A');
  responseA.resolve({ sessionId: 'session-A', model: 'stale-model' });
  await settleA;

  assert.equal(visibleInfo, infoB, 'A -> B -> late A must not overwrite the newer Session state');
}

{
  const tracker = createAcpSessionOperationTracker('session-A');
  const older = tracker.begin('session-A', 'model');
  const newer = tracker.begin('session-A', 'mode');
  assert.equal(tracker.isCurrent(older), false, 'a newer operation invalidates an older response');
  assert.equal(tracker.isCurrent(newer), true);
  assert.equal(tracker.finish(older), false, 'an older finally must not finish the newer operation');
  assert.equal(tracker.isCurrent(newer), true);
  assert.equal(tracker.finish(newer), true);
}

{
  const drafts = {
    draft: [
      { id: 'sent-before-create' },
      { id: 'added-while-create-was-pending' },
    ],
    'session-A': [{ id: 'existing-A' }],
  };
  const transferred = transferAcpDraftItems(
    drafts,
    'draft',
    'session-A',
    [{ id: 'sent-before-create' }],
    item => item.id,
  );
  assert.deepEqual(transferred.draft.map(item => item.id), ['added-while-create-was-pending'],
    'late creation must not steal attachments added to a newer draft');
  assert.deepEqual(transferred['session-A'].map(item => item.id), ['existing-A', 'sent-before-create']);

  const references = { 'session-A': ['src/old.rs', 'src/new.rs'] };
  const afterSend = removeAcpDraftItems(
    references,
    'session-A',
    ['src/old.rs'],
    item => item,
  );
  assert.deepEqual(afterSend['session-A'], ['src/new.rs'],
    'send completion must preserve references added after the request started');
}

const viewSource = await readFile(
  new URL('../src/features/codex/CodexAcpView.jsx', import.meta.url),
  'utf8',
);
assert.match(viewSource, /applySessionInfo\(next, targetId\)/,
  'ACP config responses must apply to their captured Session');
assert.match(viewSource, /let operation = beginAcpSendOperation\(targetId\)/,
  'ACP sends must capture a Session-scoped operation before awaiting');
assert.equal(
  viewSource.match(/shouldActivate: \(\) => canApplyAcpSendOperation\(operation\)/g)?.length,
  2,
  'ACP and native draft creation must not navigate after the user switched Sessions',
);
assert.equal(
  viewSource.match(/if \(created\.activated && activeIdRef\.current === targetId\) \{\s*acpSendOperationTracker\.switchSession\(targetId\);\s*operation = beginAcpSendOperation\(targetId\);/g)?.length,
  2,
  'late draft Session creation may only retarget ACP/native sends while that Session is active',
);
assert.match(viewSource, /if \(canApplyAcpSendOperation\(operation\)\) \{\s*showError\(err\);\s*setDraft\(message\);/,
  'a failed ACP send may only restore the draft and error to its owning Session');
assert.match(viewSource, /item\.sessionId !== targetId \|\| item\.toolCallId !== toolCallId/,
  'permission completion must only remove the request from its owning Session');
assert.match(viewSource, /item\.sessionId !== targetId \|\| item\.elicitationId !== elicitationId/,
  'elicitation completion must only remove the request from its owning Session');
assert.match(viewSource,
  /setRespondingSessionId\(current => current === targetId \? null : current\)/,
  'an old response finally must not clear the current Session response state');
assert.match(viewSource, /const responding = Boolean\(activeId && respondingSessionId === activeId\)/,
  'response controls must only be disabled for their owning Session');
assert.equal(
  viewSource.match(/onOpenResource=\{isWeb \? undefined : openWorkspaceResource\}/g)?.length,
  2,
  'Web ACP timelines must not receive the native-only workspace opener',
);

console.log('ACP Session operation race tests passed');
