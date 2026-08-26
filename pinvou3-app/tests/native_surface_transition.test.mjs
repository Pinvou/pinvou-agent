import assert from 'node:assert/strict';
import test from 'node:test';

import { createNativeSurfaceTransitionGate } from '../src/features/browser/native-surface-transition.mjs';

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
