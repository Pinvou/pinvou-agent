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

test('浏览器侧栏的展开与选中意图按 session 隔离', () => {
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

test('状态恢复不覆盖用户在当前窗口做出的收起选择', () => {
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

test('浏览器 prepare 结果按 session 与 attempt 隔离，迟到结果不能覆盖新状态', () => {
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
