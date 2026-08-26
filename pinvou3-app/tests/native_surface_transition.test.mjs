import assert from 'node:assert/strict';
import test from 'node:test';

import {
  createNativeSurfaceTransitionGate,
  hideNativeSurfaceWithRetry,
  settleBrowserUiPublicationAfterCommit,
} from '../src/features/browser/native-surface-transition.mjs';

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, reject, resolve };
}

test('publishes synchronously when no native surface can be visible', () => {
  const publications = [];
  const gate = createNativeSurfaceTransitionGate({
    acquireHide: () => assert.fail('hide must not be called'),
    getContext: () => ({ sessionId: 'a', hasWorkspace: false, visible: false }),
  });

  const result = gate.run(() => publications.push('settings'), { channel: 'view' });

  assert.equal(result, true);
  assert.deepEqual(publications, ['settings']);
});

test('withholds React publication until the native hide ACK', async () => {
  const hide = deferred();
  const publications = [];
  let releases = 0;
  const gate = createNativeSurfaceTransitionGate({
    acquireHide: async () => {
      await hide.promise;
      return { release: () => { releases += 1; } };
    },
    getContext: () => ({ sessionId: 'a', hasWorkspace: true, visible: true }),
  });

  const result = gate.run(() => publications.push('overlay'), { channel: 'overlay' });
  await Promise.resolve();
  assert.deepEqual(publications, []);

  hide.resolve();
  assert.equal(await result, true);
  assert.deepEqual(publications, ['overlay']);
  assert.equal(releases, 1);
});

test('hide failure is fail-closed', async () => {
  const publications = [];
  const errors = [];
  const gate = createNativeSurfaceTransitionGate({
    acquireHide: async () => { throw new Error('hide failed'); },
    getContext: () => ({ sessionId: 'a', hasWorkspace: true, visible: true }),
    onError: (error) => errors.push(error.message),
  });

  const result = await gate.run(() => publications.push('settings'), { channel: 'view' });

  assert.equal(result, false);
  assert.deepEqual(publications, []);
  assert.deepEqual(errors, ['hide failed']);
});

test('a late ACK cannot publish an older attempt in the same channel', async () => {
  const hide = deferred();
  const publications = [];
  let releases = 0;
  const gate = createNativeSurfaceTransitionGate({
    acquireHide: async () => {
      await hide.promise;
      return { release: () => { releases += 1; } };
    },
    getContext: () => ({ sessionId: 'a', hasWorkspace: true, visible: true }),
  });

  const first = gate.run(() => publications.push('artifact'), { channel: 'right-dock' });
  const second = gate.run(() => publications.push('closed'), { channel: 'right-dock' });
  hide.resolve();

  assert.equal(await first, false);
  assert.equal(await second, true);
  assert.deepEqual(publications, ['closed']);
  assert.equal(releases, 2);
});

test('a session identity change invalidates a late hide ACK', async () => {
  const hide = deferred();
  let context = { sessionId: 'a', hasWorkspace: true, visible: true };
  let released = false;
  let published = false;
  const gate = createNativeSurfaceTransitionGate({
    acquireHide: async () => {
      await hide.promise;
      return { release: () => { released = true; } };
    },
    getContext: () => context,
  });

  const result = gate.run(() => { published = true; }, { channel: 'view' });
  context = { sessionId: 'b', hasWorkspace: true, visible: true };
  hide.resolve();

  assert.equal(await result, false);
  assert.equal(published, false);
  assert.equal(released, true);
});

test('serialized session transitions finish in order and only the latest publishes', async () => {
  const firstWork = deferred();
  const publications = [];
  const gate = createNativeSurfaceTransitionGate({
    acquireHide: async () => ({ release() {} }),
    getContext: () => ({ sessionId: 'a', hasWorkspace: true, visible: true }),
  });

  const first = gate.run(async ({ isCurrent }) => {
    await firstWork.promise;
    if (!isCurrent()) return false;
    publications.push('session-b');
    return true;
  }, { channel: 'session', hideMode: 'workspace', serialize: true });
  await Promise.resolve();
  await Promise.resolve();
  const second = gate.run(() => {
    publications.push('session-c');
  }, { channel: 'session', hideMode: 'workspace', serialize: true });

  firstWork.resolve();
  assert.equal(await first, false);
  assert.equal(await second, true);
  assert.deepEqual(publications, ['session-c']);
});

test('a started false publication keeps its lease until the React commit ACK', async () => {
  const commit = deferred();
  let settled = 0;
  let completed = false;
  const result = settleBrowserUiPublicationAfterCommit({
    publish: () => false,
    waitForCommit: () => commit.promise,
    onSettled: () => { settled += 1; },
  }).then((value) => {
    completed = true;
    return value;
  });

  await Promise.resolve();
  assert.equal(completed, false);
  assert.equal(settled, 0);
  commit.resolve();
  assert.equal(await result, false);
  assert.equal(settled, 1);
});

test('a started throwing publication keeps its lease until the React commit ACK', async () => {
  const commit = deferred();
  const publicationError = new Error('session switch failed');
  let settled = 0;
  let rejected = false;
  const result = settleBrowserUiPublicationAfterCommit({
    publish: () => { throw publicationError; },
    waitForCommit: () => commit.promise,
    onSettled: () => { settled += 1; },
  }).catch((error) => {
    rejected = true;
    throw error;
  });

  await Promise.resolve();
  assert.equal(rejected, false);
  assert.equal(settled, 0);
  commit.resolve();
  await assert.rejects(result, (error) => error === publicationError);
  assert.equal(settled, 1);
});

test('an async publication settles before its React commit ACK is requested', async () => {
  const publication = deferred();
  const commit = deferred();
  const sequence = [];
  let completed = false;
  const result = settleBrowserUiPublicationAfterCommit({
    publish: async () => {
      sequence.push('publish-started');
      await publication.promise;
      sequence.push('publish-settled');
      return true;
    },
    waitForCommit: () => {
      sequence.push('commit-requested');
      return commit.promise;
    },
  }).then((value) => {
    completed = true;
    return value;
  });

  await Promise.resolve();
  assert.deepEqual(sequence, ['publish-started']);
  publication.resolve();
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(sequence, ['publish-started', 'publish-settled', 'commit-requested']);
  assert.equal(completed, false);
  commit.resolve();
  assert.equal(await result, true);
});

test('native surface hide retries exactly once while its intent remains current', async () => {
  const attempts = [];
  const errors = [];
  const result = await hideNativeSurfaceWithRetry({
    hide: async ({ attempt }) => {
      attempts.push(attempt);
      if (attempt === 1) throw new Error('transient IPC failure');
      return 'hidden';
    },
    isCurrent: () => true,
    onError: (error, details) => errors.push([error.message, details]),
  });

  assert.equal(result, 'hidden');
  assert.deepEqual(attempts, [1, 2]);
  assert.deepEqual(errors, [[
    'transient IPC failure',
    { attempt: 1, willRetry: true },
  ]]);
});

test('a newer visibility intent invalidates a pending hide retry', async () => {
  const retryWait = deferred();
  const attempts = [];
  let current = true;
  const result = hideNativeSurfaceWithRetry({
    hide: async ({ attempt }) => {
      attempts.push(attempt);
      throw new Error('hide IPC failure');
    },
    isCurrent: () => current,
    waitBeforeRetry: () => retryWait.promise,
  });

  await Promise.resolve();
  current = false;
  retryWait.resolve();

  await assert.rejects(result, /hide IPC failure/);
  assert.deepEqual(attempts, [1]);
});

test('native surface hide never retries more than once', async () => {
  const attempts = [];
  const errors = [];
  await assert.rejects(
    hideNativeSurfaceWithRetry({
      hide: async ({ attempt }) => {
        attempts.push(attempt);
        throw new Error(`failure ${attempt}`);
      },
      isCurrent: () => true,
      onError: (error, details) => errors.push([error.message, details]),
    }),
    /failure 2/,
  );

  assert.deepEqual(attempts, [1, 2]);
  assert.deepEqual(errors, [
    ['failure 1', { attempt: 1, willRetry: true }],
    ['failure 2', { attempt: 2, willRetry: false }],
  ]);
});
