// date-utils 纯函数测试:formatSessionDate 对不可解析输入不抛(与
// computePickerRows "无法解析按活跃处理" 同口径——显示侧退化为空串)。
import test from 'node:test';
import assert from 'node:assert/strict';

import { formatSessionDate } from '../src/shared/date-utils.js';

test('formatSessionDate never throws on unparseable input', () => {
  assert.equal(formatSessionDate('not-a-date', 'zh'), '');
  assert.equal(formatSessionDate('not-a-date', 'en'), '');
  assert.equal(formatSessionDate('not-a-date', 'ja'), '');
  assert.equal(formatSessionDate('', 'zh'), '');
  assert.equal(formatSessionDate(null, 'en'), '');
  assert.equal(formatSessionDate(undefined, 'ja'), '');
});

test('formatSessionDate renders relative time for valid timestamps', () => {
  const now = Date.now();
  assert.equal(formatSessionDate(new Date(now - 5000).toISOString(), 'zh'), '刚刚');
  assert.equal(formatSessionDate(new Date(now - 5000).toISOString(), 'en'), 'Just now');
  assert.equal(formatSessionDate(new Date(now - 5000).toISOString(), 'ja'), 'たった今');
  assert.match(formatSessionDate(new Date(now - 3 * 86400000).toISOString(), 'en'), /3d ago/);
});
