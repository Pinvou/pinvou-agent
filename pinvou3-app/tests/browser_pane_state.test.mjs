import assert from 'node:assert/strict';
import test from 'node:test';

import {
  activateBrowserPane,
  beginBrowserOpen,
  browserOpenStateFor,
  browserPaneStateFor,
  closeBrowserPane,
  removeBrowserPaneState,
  restoreBrowserPane,
  selectArtifactsPane,
  settleBrowserOpen,
} from '../src/features/browser/browser-pane-state.mjs';

test('browser-pane open and selection intent is isolated by session', () => {
  let states = {};
  states = activateBrowserPane(states, 'session-a');
  states = activateBrowserPane(states, 'session-b');
  states = selectArtifactsPane(states, 'session-b');

  assert.deepEqual(browserPaneStateFor(states, 'session-a'), {
    open: true,
    browserSelected: true,
    activation: 1,
  });
  assert.deepEqual(browserPaneStateFor(states, 'session-b'), {
    open: true,
    browserSelected: false,
    activation: 1,
  });

  states = closeBrowserPane(states, 'session-a');
  assert.equal(browserPaneStateFor(states, 'session-a').open, false);
  assert.equal(browserPaneStateFor(states, 'session-b').open, true);
  assert.equal(browserPaneStateFor(states, 'session-b').browserSelected, false);
});

test('state restoration does not override the user closing the pane in this window', () => {
  let states = restoreBrowserPane({}, 'session-a');
  assert.equal(browserPaneStateFor(states, 'session-a').open, true);

  states = closeBrowserPane(states, 'session-a');
  const afterRestore = restoreBrowserPane(states, 'session-a');
  assert.strictEqual(afterRestore, states);
  assert.equal(browserPaneStateFor(afterRestore, 'session-a').open, false);

  const removed = removeBrowserPaneState(afterRestore, 'session-a');
  assert.deepEqual(browserPaneStateFor(removed, 'session-a'), {
    open: false,
    browserSelected: false,
    activation: 0,
  });
});

test('browser prepare results are isolated by session and attempt so stale results cannot win', () => {
  let states = {};
  states = beginBrowserOpen(states, 'session-a', 1);
  states = beginBrowserOpen(states, 'session-b', 1);
  states = settleBrowserOpen(states, 'session-b', 1, 'failed', 'B failed');
  states = settleBrowserOpen(states, 'session-a', 1, 'idle');

  assert.deepEqual(browserOpenStateFor(states, 'session-a'), {
    attempt: 1,
    status: 'idle',
    error: '',
  });
  assert.deepEqual(browserOpenStateFor(states, 'session-b'), {
    attempt: 1,
    status: 'failed',
    error: 'B failed',
  });

  states = beginBrowserOpen(states, 'session-b', 2);
  const afterStaleFailure = settleBrowserOpen(
    states,
    'session-b',
    1,
    'failed',
    'stale failure',
  );
  assert.strictEqual(afterStaleFailure, states);
  assert.equal(browserOpenStateFor(afterStaleFailure, 'session-b').status, 'starting');
});
