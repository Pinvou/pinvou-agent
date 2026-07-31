import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
// pet-scheduled-notice.js imports ../../shared/i18n.js，用两层目录的临时副本保持相对路径可解析
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-pet-scheduled-notice-'));
const tmp = path.join(dir, 'a', 'b', 'pet-scheduled-notice.mjs');
mkdirSync(path.join(dir, 'a', 'b'), { recursive: true });
mkdirSync(path.join(dir, 'shared'), { recursive: true });
copyFileSync(path.join(here, '..', 'src', 'features', 'pet', 'pet-scheduled-notice.js'), tmp);
copyFileSync(path.join(here, '..', 'src', 'shared', 'i18n.js'), path.join(dir, 'shared', 'i18n.js'));

try {
const {
  acknowledgeScheduledNotice,
  formatScheduledNoticeBody,
  isScheduledSessionPayload,
  readScheduledNoticeAcknowledgedAt,
  selectLatestScheduledNotice,
} = await import(pathToFileURL(tmp).href);

assert.equal(isScheduledSessionPayload({ session_id: 'sched-123' }), true);
assert.equal(isScheduledSessionPayload({ sessionId: 'sched-456' }), true);
assert.equal(
  isScheduledSessionPayload({ id: 'sched-snapshot' }),
  true,
  'scheduled sessions from the activity snapshot must be filtered too',
);
assert.equal(isScheduledSessionPayload({ session_id: 'chat-123' }), false);
assert.equal(isScheduledSessionPayload(null), false);

const tasks = [
  { id: 'automation-a', name: 'AI 新闻速览', hasUnreadRuns: true },
  { id: 'automation-b', name: '项目日报', hasUnreadRuns: true },
];
const runsByTask = {
  'automation-a': [
    {
      id: 'old-run',
      status: 'completed',
      unread: true,
      sessionId: 'sched-old',
      endedAt: '2026-07-15T01:30:00.000Z',
    },
    {
      id: 'failed-run',
      status: 'failed',
      unread: true,
      sessionId: 'sched-failed',
      endedAt: '2026-07-15T03:00:00.000Z',
    },
  ],
  'automation-b': [
    {
      id: 'new-run',
      status: 'completed',
      unread: true,
      session_id: 'sched-new',
      ended_at: '2026-07-15T02:45:00.000Z',
    },
    {
      id: 'read-run',
      status: 'completed',
      unread: false,
      sessionId: 'sched-read',
      endedAt: '2026-07-15T04:00:00.000Z',
    },
  ],
};

const latest = selectLatestScheduledNotice(tasks, runsByTask, 0);
assert.deepEqual(latest, {
  automationId: 'automation-b',
  runId: 'new-run',
  sessionId: 'sched-new',
  taskName: '项目日报',
  endedAt: '2026-07-15T02:45:00.000Z',
  endedAtMs: Date.parse('2026-07-15T02:45:00.000Z'),
});
assert.match(formatScheduledNoticeBody(latest), /^\d{2}:\d{2}「项目日报」已完成$/u);
assert.equal(
  selectLatestScheduledNotice(tasks, runsByTask, Date.parse('2026-07-15T02:45:00.000Z')),
  null,
  'acknowledged and older runs must not reappear',
);

const values = new Map();
const storage = {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => values.set(key, value),
};
assert.equal(readScheduledNoticeAcknowledgedAt(storage), 0);
acknowledgeScheduledNotice(latest, storage);
assert.equal(readScheduledNoticeAcknowledgedAt(storage), latest.endedAtMs);
acknowledgeScheduledNotice({ endedAtMs: latest.endedAtMs - 10_000 }, storage);
assert.equal(
  readScheduledNoticeAcknowledgedAt(storage),
  latest.endedAtMs,
  'the acknowledgement watermark must never move backwards',
);

console.log('pet scheduled notice logic tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
