import assert from 'node:assert/strict';
import {
  cancelPendingAcpAttachments,
  isPendingAcpAttachment,
  runAcpAttachmentTask,
} from '../src/features/codex/acp-attachment-lifecycle.js';

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((nextResolve, nextReject) => {
    resolve = nextResolve;
    reject = nextReject;
  });
  return { promise, resolve, reject };
}

assert.equal(isPendingAcpAttachment({ status: 'parsing' }), true);
assert.equal(isPendingAcpAttachment({ status: 'uploading' }), true);
assert.equal(isPendingAcpAttachment({ status: 'ready' }), false);

{
  const cancelledIds = new Set();
  cancelPendingAcpAttachments([
    { id: 'parsing-1', status: 'parsing' },
    { id: 'uploading-1', status: 'uploading' },
    { id: 'ready-1', status: 'ready' },
  ], cancelledIds);
  assert.deepEqual([...cancelledIds].sort(), ['parsing-1', 'uploading-1']);
}

{
  const operation = deferred();
  const cancelledIds = new Set();
  const discarded = [];
  const ready = [];
  const task = runAcpAttachmentTask({
    id: 'parsing-race',
    cancelledIds,
    load: () => operation.promise,
    discard: async result => discarded.push(result),
    onReady: result => ready.push(result),
    onError: error => assert.fail(`unexpected parsing error: ${error}`),
  });

  cancelledIds.add('parsing-race');
  const opaqueResult = { handle: 'opaque-attachment-handle' };
  operation.resolve(opaqueResult);

  assert.equal(await task, false);
  assert.deepEqual(discarded, [opaqueResult], 'a late parsing result must be discarded');
  assert.deepEqual(ready, [], 'a removed parsing attachment must not return to the UI');
  assert.equal(cancelledIds.has('parsing-race'), false, 'the cancellation marker must be cleared');
}

{
  const cancelledIds = new Set(['desktop-path-race']);
  const desktopResult = { path: 'C:\\source\\notes.txt', basename: 'notes.txt' };
  const discarded = [];
  const accepted = await runAcpAttachmentTask({
    id: 'desktop-path-race',
    cancelledIds,
    load: async () => desktopResult,
    discard: async result => discarded.push(result),
    onReady: () => assert.fail('a cleared desktop path attachment must not become ready'),
    onError: error => assert.fail(`unexpected desktop path error: ${error}`),
  });

  assert.equal(accepted, false);
  assert.deepEqual(discarded, [desktopResult], 'desktop path results use the same cleanup path');
  assert.equal(cancelledIds.has('desktop-path-race'), false);
}

{
  const operation = deferred();
  const cancelledIds = new Set(['unmounted-upload']);
  const errors = [];
  const task = runAcpAttachmentTask({
    id: 'unmounted-upload',
    cancelledIds,
    load: () => operation.promise,
    discard: () => assert.fail('a failed upload has no result to discard'),
    onReady: () => assert.fail('an unmounted upload must not become ready'),
    onError: error => errors.push(error),
  });
  operation.reject(new Error('connection closed'));

  assert.equal(await task, false);
  assert.deepEqual(errors, [], 'cancelled failures must not restore an error chip');
  assert.equal(cancelledIds.has('unmounted-upload'), false);
}

{
  const cancelledIds = new Set();
  const result = { path: 'C:\\draft\\ready.txt' };
  const ready = [];
  const accepted = await runAcpAttachmentTask({
    id: 'ready-attachment',
    cancelledIds,
    load: async () => result,
    discard: () => assert.fail('a live attachment must not be discarded'),
    onReady: value => ready.push(value),
    onError: error => assert.fail(`unexpected ready error: ${error}`),
  });

  assert.equal(accepted, true);
  assert.deepEqual(ready, [result]);
}

console.log('acp attachment lifecycle tests passed');
