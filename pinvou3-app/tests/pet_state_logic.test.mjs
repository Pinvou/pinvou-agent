#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const src = path.join(here, '..', 'src', 'features', 'pet', 'pet-state.js');
const i18nSrc = path.join(here, '..', 'src', 'shared', 'i18n.js');
const dir = mkdtempSync(path.join(tmpdir(), 'pinvou3-pet-state-'));
// pet-state.js imports ../../shared/i18n.js，保持两层目录结构以便相对路径解析
const tmp = path.join(dir, 'a', 'b', 'pet-state.mjs');
mkdirSync(path.join(dir, 'a', 'b'), { recursive: true });
mkdirSync(path.join(dir, 'shared'), { recursive: true });
copyFileSync(src, tmp);
copyFileSync(i18nSrc, path.join(dir, 'shared', 'i18n.js'));
// i18n.js 现按语言拆分(zh 内嵌),临时副本需带上 i18n/ 目录才能解析
mkdirSync(path.join(dir, 'shared', 'i18n'), { recursive: true });
for (const f of ['zh.js']) copyFileSync(path.join(here, '..', 'src', 'shared', 'i18n', f), path.join(dir, 'shared', 'i18n', f));

try {
  const {
    ACTIVITY_PRIORITY,
    ACTIVITY_TTL_MS,
    SNAPSHOT_REMOVAL_GRACE_MS,
    applyActivitySnapshot,
    applyEvent,
    createPetState,
    deriveActivities,
    deriveAnimation,
    markSessionViewed,
    removeSessionActivity,
    syncSessionTitles,
  } = await import(`${pathToFileURL(tmp).href}?t=${Date.now()}`);

  assert.deepEqual(ACTIVITY_PRIORITY, { waiting: 0, failed: 1, review: 2, running: 3 });
  assert.deepEqual(ACTIVITY_TTL_MS, {
    running: 3 * 60 * 1000,
    failed: 60 * 60 * 1000,
    waiting: 24 * 60 * 60 * 1000,
    review: 7 * 24 * 60 * 60 * 1000,
  });

  const now = 1_000_000;
  const state = createPetState();
  syncSessionTitles(state, [
    { id: 'run', title: '实现宠物动画' },
    { id: 'wait', title: '确认发布范围' },
    { id: 'fail', title: '修复构建' },
    { id: 'done', title: '完成文档' },
  ]);

  applyEvent(state, 'pet:turn_start', { session_id: 'run' }, now);
  applyEvent(state, 'chat:delta', { session_id: 'run', text: '正在逐帧实现官方动画' }, now + 1);
  applyEvent(state, 'chat:user_input_required', { session_id: 'wait', prompt: '请选择发布范围' }, now + 2);
  applyEvent(state, 'chat:done', { session_id: 'fail', status: 'Failed', error: '构建失败' }, now + 3);
  applyEvent(state, 'chat:done', { session_id: 'done', status: 'Completed' }, now + 4);

  const activities = deriveActivities(state, now + 5);
  assert.deepEqual(activities.map((item) => item.sessionId), ['wait', 'fail', 'done', 'run']);
  assert.deepEqual(activities.map((item) => item.status), ['waiting', 'failed', 'review', 'running']);
  assert.equal(activities[0].title, '确认发布范围');
  assert.equal(deriveAnimation(state, now + 5), 'waiting');

  const authoritative = createPetState();
  applyEvent(authoritative, 'pet:turn_start', { session_id: 'present' }, now);
  applyEvent(authoritative, 'chat:done', { session_id: 'missing', status: 'Completed' }, now + 1);
  syncSessionTitles(authoritative, [{ id: 'present', title: '仍然存在的任务' }]);
  assert.deepEqual(
    deriveActivities(authoritative, now + 2).map((item) => item.sessionId),
    ['present'],
    'the authoritative session snapshot must remove activities for deleted sessions',
  );
  assert.equal(removeSessionActivity(authoritative, 'present'), true);
  assert.deepEqual(deriveActivities(authoritative, now + 3), []);

  markSessionViewed(state, 'done');
  markSessionViewed(state, 'fail');
  markSessionViewed(state, 'wait');
  assert.deepEqual(
    deriveActivities(state, now + 6).map((item) => item.sessionId),
    ['wait', 'run'],
    'viewing clears review/failed but keeps waiting/running work visible',
  );

  applyEvent(state, 'chat:done', { session_id: 'run', status: 'Cancelled' }, now + 7);
  assert.deepEqual(deriveActivities(state, now + 8).map((item) => item.sessionId), ['wait']);

  for (const status of ['Interrupted', 'Canceled']) {
    applyEvent(state, 'pet:turn_start', { session_id: status }, now + 9);
    applyEvent(state, 'chat:done', { session_id: status, status }, now + 10);
  }
  assert.deepEqual(
    deriveActivities(state, now + 11).map((item) => item.sessionId),
    ['wait'],
    'stopped turns must disappear instead of being reported as completed',
  );

  const invokeFailure = createPetState();
  applyEvent(invokeFailure, 'pet:turn_start', { session_id: 'direct-failure' }, now);
  applyEvent(invokeFailure, 'pet:turn_end', { session_id: 'direct-failure' }, now + 1);
  assert.deepEqual(
    deriveActivities(invokeFailure, now + 2).map((item) => item.status),
    ['failed'],
    'turn_end is emitted only when the chat invoke rejects before chat:done',
  );

  const expiry = createPetState();
  applyEvent(expiry, 'pet:turn_start', { session_id: 'r' }, now);
  applyEvent(expiry, 'chat:user_input_required', { session_id: 'w' }, now);
  applyEvent(expiry, 'chat:done', { session_id: 'f', status: 'Failed' }, now);
  applyEvent(expiry, 'chat:done', { session_id: 'v', status: 'Completed' }, now);
  assert.deepEqual(
    deriveActivities(expiry, now + ACTIVITY_TTL_MS.running + 1).map((item) => item.sessionId),
    ['w', 'f', 'v'],
  );
  assert.deepEqual(
    deriveActivities(expiry, now + ACTIVITY_TTL_MS.failed + 1).map((item) => item.sessionId),
    ['w', 'v'],
  );

  const latest = createPetState();
  applyEvent(latest, 'pet:turn_start', { session_id: 'chat' }, now);
  applyEvent(latest, 'chat:delta', { session_id: 'chat', text: '第一轮完整回答的开头，' }, now + 1);
  applyEvent(latest, 'chat:tool_start', { session_id: 'chat', name: 'shell' }, now + 2);
  applyEvent(latest, 'chat:tool_end', { session_id: 'chat', name: 'shell' }, now + 3);
  applyEvent(latest, 'chat:delta', { session_id: 'chat', text: '以及工具后的结论。' }, now + 4);
  applyEvent(latest, 'chat:done', { session_id: 'chat', status: 'Completed' }, now + 5);
  assert.equal(
    deriveActivities(latest, now + 6)[0].body,
    '第一轮完整回答的开头，以及工具后的结论。',
    'tool and done events must not replace the latest assistant reply',
  );

  applyEvent(latest, 'pet:turn_start', { session_id: 'chat' }, now + 7);
  assert.equal(
    deriveActivities(latest, now + 8)[0].body,
    '正在思考…',
    'a new turn must replace the previous reply with the current phase',
  );
  assert.equal(
    deriveActivities(latest, now + 8)[0].latestReply,
    '',
    'a new turn must not expose stale assistant text as the latest reply',
  );
  applyEvent(latest, 'chat:tool_start', { session_id: 'chat', name: 'fetch_url' }, now + 8);
  assert.equal(deriveActivities(latest, now + 8)[0].body, '正在使用 fetch_url');
  applyEvent(latest, 'chat:tool_end', { session_id: 'chat' }, now + 8);
  assert.equal(deriveActivities(latest, now + 8)[0].body, '继续处理…');
  applyEvent(latest, 'chat:delta', { session_id: 'chat', text: '第二轮回答。' }, now + 9);
  assert.equal(deriveActivities(latest, now + 10)[0].body, '第二轮回答。');

  const longReply = `${'最新回复内容'.repeat(80)}结尾`;
  applyEvent(latest, 'chat:delta', { session_id: 'chat', text: longReply }, now + 11);
  assert.equal(
    deriveActivities(latest, now + 12)[0].body,
    `第二轮回答。${longReply}`,
    'state keeps the complete latest reply and leaves visual truncation to CSS',
  );

  assert.equal(applyEvent(latest, 'chat:usage', { session_id: 'a' }, now), false);
  assert.equal(applyEvent(latest, 'chat:delta', {}, now), false);

  // ── 快照对齐:新会话首次对话的时序竞态 ──
  // 完成→自动已读后,迟到的 working:true 快照不得复活卡片。
  const race = createPetState();
  applyActivitySnapshot(race, [{ id: 's1', working: true }], 1, now - 1);
  applyEvent(race, 'pet:turn_start', { session_id: 's1' }, now);
  applyEvent(race, 'chat:done', { session_id: 's1', status: 'Completed' }, now + 100);
  assert.equal(markSessionViewed(race, 's1', now + 200), true);
  assert.equal(
    applyActivitySnapshot(race, [{ id: 's1', working: true }], 2, now + 500),
    false,
    'a stale working snapshot must not resurrect a just-viewed card',
  );
  assert.deepEqual(deriveActivities(race, now + 501), []);

  // 打开运行中的会话后若主窗口失焦/最小化，未来的完成结果仍必须展示。
  const viewedFirst = createPetState();
  applyEvent(viewedFirst, 'pet:turn_start', { session_id: 's-viewed-first' }, now);
  assert.equal(markSessionViewed(viewedFirst, 's-viewed-first'), false);
  assert.deepEqual(deriveActivities(viewedFirst, now + 1).map((item) => item.status), ['running']);
  assert.equal(
    applyEvent(viewedFirst, 'chat:done', { session_id: 's-viewed-first', status: 'Completed' }, now + 2),
    true,
  );
  assert.deepEqual(deriveActivities(viewedFirst, now + 3).map((item) => item.status), ['review']);

  // 完成时主窗口仍聚焦当前会话，确认事件即使先到也必须压住完成卡。
  const completedViewFirst = createPetState();
  applyEvent(completedViewFirst, 'pet:turn_start', { session_id: 's-completed-view-first' }, now);
  assert.equal(markSessionViewed(
    completedViewFirst,
    's-completed-view-first',
    { completed: true },
  ), false);
  assert.equal(
    applyEvent(
      completedViewFirst,
      'chat:done',
      { session_id: 's-completed-view-first', status: 'Completed' },
      now + 2,
    ),
    true,
  );
  assert.deepEqual(deriveActivities(completedViewFirst, now + 3), []);

  // 权威 false 快照经宽限期收尾：先标记待删，宽限期后仍无事件才真正删卡，
  // 给事件流(chat:done / turn_end)留出到达窗口，避免多会话并发时的误删闪现。
  const ghost = createPetState();
  applyEvent(ghost, 'pet:turn_start', { session_id: 's3' }, now);
  assert.equal(
    applyActivitySnapshot(ghost, [{ id: 's3', working: false }], 1, now + 1),
    true,
    'a not-working snapshot must mark a running ghost for removal',
  );
  assert.deepEqual(
    deriveActivities(ghost, now + 2).map((item) => item.status),
    ['running'],
    'the ghost card must survive the grace window in case terminal events are in flight',
  );
  assert.deepEqual(
    deriveActivities(ghost, now + SNAPSHOT_REMOVAL_GRACE_MS + 1),
    [],
    'a ghost with no terminal events must be removed after the grace period',
  );

  // 竞态场景(多会话并发):快照说 working:false 后,终态事件在宽限期内到达
  // → 卡保留为完成状态,而不是被快照删掉再被事件重建(后者会驱动窗口收起
  // 又展开,即用户看到的"桌宠闪现")。
  const raceGrace = createPetState();
  applyEvent(raceGrace, 'pet:turn_start', { session_id: 's-race' }, now);
  assert.equal(
    applyActivitySnapshot(raceGrace, [{ id: 's-race', working: false }], 1, now + 1),
    true,
    'a not-working snapshot must mark the running card for removal',
  );
  applyEvent(raceGrace, 'chat:done', { session_id: 's-race', status: 'Completed' }, now + 2);
  assert.deepEqual(
    deriveActivities(raceGrace, now + 3).map((item) => item.status),
    ['review'],
    'a terminal event inside the grace window must keep the card as completed',
  );
  // 收尾后即使快照重复说 working:false,完成卡也保留(不是 running)。
  applyActivitySnapshot(raceGrace, [{ id: 's-race', working: false }], 2, now + 4);
  assert.deepEqual(
    deriveActivities(raceGrace, now + 5).map((item) => item.status),
    ['review'],
    'a not-working snapshot must not remove a completed card',
  );

  // 反向竞态:快照误说 working:false 但会话仍在流式 → delta 事件取消待删标记。
  const live = createPetState();
  applyEvent(live, 'pet:turn_start', { session_id: 's-live' }, now);
  applyActivitySnapshot(live, [{ id: 's-live', working: false }], 1, now + 1);
  applyEvent(live, 'chat:delta', { session_id: 's-live', text: '仍在输出' }, now + 2);
  assert.deepEqual(
    deriveActivities(live, now + 3).map((item) => item.status),
    ['running'],
    'a delta inside the grace window must cancel the pending removal',
  );

  // 乱序快照不得让已完成的会话倒退回 working。
  applyActivitySnapshot(ghost, [{ id: 's3', working: false }], 3, now + 3);
  assert.equal(
    applyActivitySnapshot(ghost, [{ id: 's3', working: true }], 2, now + 4),
    false,
    'an older snapshot must be ignored',
  );
  assert.deepEqual(
    deriveActivities(ghost, now + SNAPSHOT_REMOVAL_GRACE_MS + 4),
    [],
    'older snapshot must not resurrect the removed ghost',
  );

  // 真实新回合(pet:turn_start)清除已读标记,后续对话不受影响。
  const nextTurn = createPetState();
  applyEvent(nextTurn, 'pet:turn_start', { session_id: 's2' }, now);
  applyEvent(nextTurn, 'chat:done', { session_id: 's2', status: 'Completed' }, now + 1);
  markSessionViewed(nextTurn, 's2', now + 2);
  applyEvent(nextTurn, 'pet:turn_start', { session_id: 's2' }, now + 3);
  assert.deepEqual(
    deriveActivities(nextTurn, now + 4).map((item) => item.status),
    ['running'],
    'a real new turn must clear the viewed tombstone immediately',
  );

  // working:false 不得触碰 review/failed/waiting 等等待用户的卡。
  const keep = createPetState();
  applyEvent(keep, 'chat:done', { session_id: 's4', status: 'Completed' }, now);
  applyEvent(keep, 'chat:user_input_required', { session_id: 's5' }, now);
  applyActivitySnapshot(keep, [
    { id: 's4', working: false },
    { id: 's5', working: false },
  ], 1, now + 60_000);
  assert.deepEqual(
    deriveActivities(keep, now + 60_001).map((item) => item.sessionId).sort(),
    ['s4', 's5'],
    'not-working snapshots must never clear cards that wait for the user',
  );

  console.log('pet state logic tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
