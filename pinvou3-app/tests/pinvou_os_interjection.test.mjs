import assert from 'node:assert/strict';
import test from 'node:test';

import {
  queuedMessagePresentations,
  visibleUnqueuedUtterance,
} from '../src/features/pinvou_os/pinvou-os-interjection.js';

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
