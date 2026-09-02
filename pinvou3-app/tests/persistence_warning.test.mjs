import assert from 'node:assert/strict';
import test from 'node:test';

import {
  EMPTY_PERSISTENCE_WARNING,
  isPersistenceStatusCurrent,
  persistenceWarningReducer,
  visiblePersistenceWarning,
} from '../src/features/browser/persistence-warning.mjs';

test('dismiss hides only the currently displayed persistence warning', () => {
  let state = persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
    type: 'report',
    message: 'save failed',
  });
  assert.equal(visiblePersistenceWarning(state), 'save failed');

  state = persistenceWarningReducer(state, { type: 'dismiss' });
  assert.equal(visiblePersistenceWarning(state), '');
  assert.equal(state.message, 'save failed', 'dismiss must not pretend backend recovery');
});

test('status hydration preserves dismissal for unchanged warning text', () => {
  const dismissed = persistenceWarningReducer(
    persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
      type: 'report',
      message: 'save failed',
    }),
    { type: 'dismiss' },
  );

  const repeated = persistenceWarningReducer(dismissed, {
    type: 'hydrate',
    message: 'save failed',
  });
  assert.equal(visiblePersistenceWarning(repeated), '');
  assert.equal(repeated.dismissed, true);

  const changed = persistenceWarningReducer(dismissed, {
    type: 'hydrate',
    message: 'different failure',
  });
  assert.equal(visiblePersistenceWarning(changed), 'different failure');
});

test('a new backend event is visible even when its warning text is unchanged', () => {
  const dismissed = persistenceWarningReducer(
    persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
      type: 'report',
      message: 'save failed',
    }),
    { type: 'dismiss' },
  );

  const repeated = persistenceWarningReducer(dismissed, {
    type: 'report',
    message: 'save failed',
  });
  assert.equal(visiblePersistenceWarning(repeated), 'save failed');
  assert.equal(repeated.dismissed, false);
});

test('clear and remount state remove stale persistence details', () => {
  const reported = persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
    type: 'report',
    message: 'save failed',
  });
  assert.equal(
    persistenceWarningReducer(reported, { type: 'clear' }),
    EMPTY_PERSISTENCE_WARNING,
  );
  assert.equal(visiblePersistenceWarning(EMPTY_PERSISTENCE_WARNING), '');
});

test('empty status hydration clears a stale warning', () => {
  const reported = persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
    type: 'report',
    message: 'save failed',
  });
  assert.equal(
    persistenceWarningReducer(reported, { type: 'hydrate', message: '' }),
    EMPTY_PERSISTENCE_WARNING,
  );
});

test('a stale warning status cannot resurrect after a restored event', () => {
  let eventEpoch = 0;
  const requestEventEpoch = eventEpoch;
  let state = persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
    type: 'report',
    message: 'save failed',
  });

  eventEpoch += 1;
  state = persistenceWarningReducer(state, { type: 'clear' });
  if (isPersistenceStatusCurrent(requestEventEpoch, eventEpoch)) {
    state = persistenceWarningReducer(state, { type: 'hydrate', message: 'save failed' });
  }

  assert.equal(state, EMPTY_PERSISTENCE_WARNING);
});

test('a stale empty status cannot clear a newer warning event', () => {
  let eventEpoch = 0;
  const requestEventEpoch = eventEpoch;
  eventEpoch += 1;
  let state = persistenceWarningReducer(EMPTY_PERSISTENCE_WARNING, {
    type: 'report',
    message: 'new failure',
  });

  if (isPersistenceStatusCurrent(requestEventEpoch, eventEpoch)) {
    state = persistenceWarningReducer(state, { type: 'hydrate', message: '' });
  }

  assert.equal(visiblePersistenceWarning(state), 'new failure');
});
