import assert from 'node:assert/strict';
import test from 'node:test';

import {
  getNextTurnFeedbackDelay,
  getTurnFeedback,
  latestOpenTurnStart,
  queuedMessagePresentations,
  visibleUnqueuedUtterance,
} from '../src/features/pinvou_os/pinvou-os-interjection.js';

test('turn feedback follows the latest unclosed user_start and ignores closed turns', () => {
  const timeline = [
    { turn_id: 'closed', event: 'user_start', timestamp: 1_000 },
    { turn_id: 'closed', event: 'assistant_done', timestamp: 2_000 },
    { turn_id: 'open', event: 'user_start', timestamp: 10_000 },
  ];

  assert.deepEqual(latestOpenTurnStart(timeline), timeline[2]);
  assert.equal(getTurnFeedback(timeline, 12_999), null);
  assert.equal(getTurnFeedback(timeline, 13_000).phase, 'ready');
  assert.equal(getTurnFeedback(timeline, 24_999).phase, 'ready');
  assert.equal(getTurnFeedback(timeline, 25_000).phase, 'long');
  assert.equal(getTurnFeedback(timeline, 39_999).phase, 'long');
  assert.equal(getTurnFeedback(timeline, 40_000).phase, 'extended');
});

test('turn feedback never falls back to unrelated or invalid timing state', () => {
  assert.equal(getTurnFeedback([], 50_000), null);
  assert.equal(getTurnFeedback([
    { turn_id: 'done', event: 'user_start', timestamp: 1_000 },
    { turn_id: 'done', event: 'assistant_done', timestamp: 2_000 },
  ], 50_000), null);
  assert.equal(getTurnFeedback([
    { turn_id: 'bad', event: 'user_start', timestamp: 'not-a-time' },
  ], 50_000), null);
});

test('feedback timer schedules only the remaining 3, 15 and 30 second boundaries', () => {
  const timeline = [{ turn_id: 'open', event: 'user_start', timestamp: 10_000 }];
  assert.equal(getNextTurnFeedbackDelay(timeline, 10_000), 3_000);
  assert.equal(getNextTurnFeedbackDelay(timeline, 13_000), 12_000);
  assert.equal(getNextTurnFeedbackDelay(timeline, 25_000), 15_000);
  assert.equal(getNextTurnFeedbackDelay(timeline, 40_000), null);
});

test('queued interjections expose the spoken phrase and suppress only its duplicate utterance', () => {
  const queued = [
    { id: 7, text: 'payload', displayText: '  帮我先看日志  ' },
    { id: 8, text: '再检查网络' },
    { id: 9, text: '   ' },
  ];

  assert.deepEqual(queuedMessagePresentations(queued), [
    { id: 7, text: '帮我先看日志' },
    { id: 8, text: '再检查网络' },
  ]);
  assert.equal(visibleUnqueuedUtterance('帮我先看日志', queued), '');
  assert.equal(visibleUnqueuedUtterance(' 帮我先看日志\n', queued), '');
  assert.equal(visibleUnqueuedUtterance('帮我先看', queued), '帮我先看');
});
