import assert from 'node:assert/strict';
import test from 'node:test';

import {
  CONVERSATION_VIRTUALIZATION_THRESHOLD,
  shouldVirtualizeConversationTurns,
  splitConversationLiveTail,
} from '../src/features/conversation/conversation-virtualization.js';

test('conversation virtualization only activates for long scrollable timelines', () => {
  const scrollElementRef = { current: null };
  assert.equal(shouldVirtualizeConversationTurns(CONVERSATION_VIRTUALIZATION_THRESHOLD, scrollElementRef), false);
  assert.equal(shouldVirtualizeConversationTurns(CONVERSATION_VIRTUALIZATION_THRESHOLD + 1, null), false);
  assert.equal(shouldVirtualizeConversationTurns(CONVERSATION_VIRTUALIZATION_THRESHOLD + 1, scrollElementRef), true);
});

test('the final running turn stays in the normal-flow live tail', () => {
  const turns = [
    { id: 'completed', status: 'completed' },
    { id: 'running', status: 'running' },
  ];
  const split = splitConversationLiveTail(turns, true);
  assert.deepEqual(split.historyTurns, [turns[0]]);
  assert.equal(split.liveTurn, turns[1]);
  assert.equal(split.liveTurnIndex, 1);
  assert.equal(turns.length, 2, 'splitting must not mutate the projection');
});

test('completed tails and non-virtualized timelines preserve the original array', () => {
  const completed = [{ id: 'completed', status: 'completed' }];
  assert.equal(splitConversationLiveTail(completed, true).historyTurns, completed);
  assert.equal(splitConversationLiveTail(completed, false).historyTurns, completed);
});

test('a busy tail without a terminal stays live before its running status arrives', () => {
  const turns = [
    { id: 'completed', status: 'completed', completedAt: 1 },
    { id: 'accepted', status: 'idle', completedAt: null },
  ];
  const split = splitConversationLiveTail(turns, true, true);
  assert.deepEqual(split.historyTurns, [turns[0]]);
  assert.equal(split.liveTurn, turns[1]);
  assert.equal(split.liveTurnIndex, 1);
});

test('busy does not revive a tail that already has a terminal timestamp', () => {
  const completed = [{ id: 'completed', status: 'completed', completedAt: 1 }];
  assert.equal(splitConversationLiveTail(completed, true, true).historyTurns, completed);
});

test('an empty busy conversation has no live tail', () => {
  assert.deepEqual(splitConversationLiveTail([], true, true), {
    historyTurns: [],
    liveTurn: null,
    liveTurnIndex: null,
  });
});
