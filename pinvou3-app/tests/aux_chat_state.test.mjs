/** 辅助对话（aux-chat）纯逻辑层契约：快照归一化、busy/空态判定与 turns 投影。 */
import assert from 'node:assert/strict';
import test from 'node:test';
import {
  auxChatBusy,
  auxChatHasContent,
  auxSnapshotsEqual,
  normalizeAuxSnapshot,
  projectAuxChatTurns,
} from '../src/features/aux-chat/aux-chat-state.mjs';

test('normalizeAuxSnapshot 对非法输入返回空结构', () => {
  for (const raw of [null, undefined, 42, 'aux-x', { chatItems: 'nope' }]) {
    const snap = normalizeAuxSnapshot(raw);
    assert.deepEqual(snap, { chatItems: [], busy: false, queued: [] });
  }
  const snap = normalizeAuxSnapshot({ chatItems: [{ type: 'user' }], busy: 1, queued: [{}] });
  assert.equal(snap.chatItems.length, 1);
  assert.equal(snap.busy, true);
  assert.equal(snap.queued.length, 1);
});

test('auxChatBusy 与 bridge send 的拒绝口径一致（busy 或排队均不可发送）', () => {
  assert.equal(auxChatBusy({ chatItems: [], busy: false, queued: [] }), false);
  assert.equal(auxChatBusy({ chatItems: [], busy: true, queued: [] }), true);
  assert.equal(auxChatBusy({ chatItems: [], busy: false, queued: [{ id: 1 }] }), true);
  assert.equal(auxChatBusy(null), false);
});

test('auxChatHasContent 只把 user/assistant 条目算作内容', () => {
  assert.equal(auxChatHasContent(null), false);
  assert.equal(auxChatHasContent({ chatItems: [] }), false);
  assert.equal(auxChatHasContent({ chatItems: [{ type: 'system', text: 's' }] }), false);
  assert.equal(auxChatHasContent({ chatItems: [{ type: 'user', text: 'q' }] }), true);
  assert.equal(auxChatHasContent({ chatItems: [{ type: 'assistant', text: 'a' }] }), true);
});

test('projectAuxChatTurns 把 user+assistant 快照投影成对话 turns', () => {
  const snapshot = {
    chatItems: [
      { id: 1, type: 'user', text: '什么是辅助对话？' },
      { id: 2, type: 'assistant', text: '一条独立问答会话。' },
    ],
    busy: false,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-01');
  assert.equal(turns.length, 1);
  assert.equal(turns[0].userText, '什么是辅助对话？');
  const assistant = turns[0].items.find((item) => item.type === 'agent_message');
  assert.ok(assistant, 'assistant 条目应投影为 agent_message');
  assert.equal(assistant.text, '一条独立问答会话。');
  assert.equal(assistant.status, 'completed');
  assert.equal(turns[0].status, 'completed');
});

test('projectAuxChatTurns 在 busy 时把末尾 turn 标为 running', () => {
  const snapshot = {
    chatItems: [
      { id: 1, type: 'user', text: 'q' },
      { id: 2, type: 'assistant', text: '流式中', streaming: true },
    ],
    busy: true,
    queued: [],
  };
  const turns = projectAuxChatTurns(snapshot, 'aux-01');
  assert.equal(turns.length, 1);
  assert.equal(turns[0].status, 'running');
  assert.equal(turns[0].completedAt, null);
});

test('projectAuxChatTurns 对空快照返回空 turns', () => {
  assert.deepEqual(projectAuxChatTurns(null, 'aux-01'), []);
  assert.deepEqual(projectAuxChatTurns({ chatItems: [] }, null), []);
});

test('auxSnapshotsEqual 对内容相同的重拉快照判定相等', () => {
  const item = { id: 1, type: 'user', text: 'q' };
  const prev = normalizeAuxSnapshot({ chatItems: [item], busy: false, queued: [] });
  // 桥每次 snapshot() 都返回浅拷贝的新数组，条目引用不变（主会话流式 tick
  // 不碰辅助会话 buffer）：必须判等，面板才能跳过重渲染。
  const next = normalizeAuxSnapshot({ chatItems: [item], busy: false, queued: [] });
  assert.equal(auxSnapshotsEqual(prev, next), true);
  assert.equal(auxSnapshotsEqual(prev, prev), true);
  assert.equal(auxSnapshotsEqual(null, null), true);
  assert.equal(auxSnapshotsEqual(null, { chatItems: [] }), true);
});

test('auxSnapshotsEqual 捕捉流式原地修改与 busy/排队变化', () => {
  const prev = normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: '流式', streaming: true }],
    busy: true,
    queued: [],
  });
  // 流式 delta 是原地改条目字段（bridge 对 item 做 Object.assign / text 追加），
  // 新快照拿到的是字段已变的同名条目：必须判不等，否则面板冻结。
  const streamed = normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: '流式中', streaming: true }],
    busy: true,
    queued: [],
  });
  assert.equal(auxSnapshotsEqual(prev, streamed), false);
  assert.equal(auxSnapshotsEqual(prev, normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: '流式', streaming: true }],
    busy: false,
    queued: [],
  })), false);
  assert.equal(auxSnapshotsEqual(prev, normalizeAuxSnapshot({
    chatItems: [
      { id: 1, type: 'assistant', text: '流式', streaming: true },
      { id: 2, type: 'user', text: 'q2' },
    ],
    busy: true,
    queued: [],
  })), false);
  assert.equal(auxSnapshotsEqual(prev, normalizeAuxSnapshot({
    chatItems: [{ id: 1, type: 'assistant', text: '流式', streaming: true }],
    busy: true,
    queued: [{ id: 9 }],
  })), false);
});
