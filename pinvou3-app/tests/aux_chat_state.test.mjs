/** 辅助对话（aux-chat）纯逻辑层契约：快照归一化、busy/空态判定与 turns 投影。 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import {
  auxChatBusy,
  auxChatHasContent,
  auxSnapshotsEqual,
  normalizeAuxSnapshot,
  projectAuxChatTurns,
} from '../src/features/aux-chat/aux-chat-state.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));

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

test('auxChatBusy 判定与真实桥 send 的 busy 拒绝分支同口径', () => {
  assert.equal(auxChatBusy({ chatItems: [], busy: false, queued: [] }), false);
  assert.equal(auxChatBusy({ chatItems: [], busy: true, queued: [] }), true);
  assert.equal(auxChatBusy({ chatItems: [], busy: false, queued: [{ id: 1 }] }), true);
  assert.equal(auxChatBusy(null), false);
});

// 桥的 busy 拒绝此前只有「口径一致」的命名、从未被执行过（既有 harness 的
// isBusyFor 恒 false）：这里驱动真实 aux-chat.js 工厂，置 busy / 排队后断言
// send 抛 turnAlreadyInProgress——面板层 auxChatBusy 的预判与桥的拒绝必须
// 对同一状态集合生效。
function loadTauriAuxChatWithBusy({ busyFor, queued }) {
  const root = { __PINVOU_SHARED_I18N__: {} };
  const src = fs.readFileSync(
    path.join(here, '..', 'src', 'platform', 'tauri', 'bridge', 'aux-chat.js'),
    'utf8',
  );
  vm.runInNewContext(src, { window: root, globalThis: root, setTimeout, clearTimeout });
  return root.__PINVOU_TAURI_BRIDGE_FEATURES__.auxChat({
    state: { activeSessionId: null },
    sessionStates: queued ? { 'aux-1': { queued: [{ id: 1 }] } } : {},
    bt: key => key,
    invoke: async () => ({}),
    ensureSessionBufferLoaded: async () => {},
    purgeSessionBuffer() {},
    touchSessionBuffer() {},
    isBusyFor: busyFor,
  });
}

test('真实桥 send 在 busy 或有排队时拒绝（turnAlreadyInProgress）', async () => {
  const busyChat = loadTauriAuxChatWithBusy({ busyFor: () => true, queued: false });
  await assert.rejects(
    busyChat.send('aux-1', 'q'),
    /turnAlreadyInProgress/u,
    'busy 会话必须被桥拒绝',
  );
  const queuedChat = loadTauriAuxChatWithBusy({ busyFor: () => false, queued: true });
  await assert.rejects(
    queuedChat.send('aux-1', 'q'),
    /turnAlreadyInProgress/u,
    '有排队消息的会话必须被桥拒绝',
  );
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
  const prev = normalizeAuxSnapshot({ chatItems: [{ ...item }], busy: false, queued: [] });
  // 桥每次 snapshot() 都对条目逐个浅拷贝（streaming delta 原地改 buffer 条目，
  // 拷贝后字段比较才是真实内容比较）：内容没变的重拉快照条目引用不同、字段
  // 相同——必须判等，面板才能跳过重渲染。
  const next = normalizeAuxSnapshot({ chatItems: [{ ...item }], busy: false, queued: [] });
  assert.equal(auxSnapshotsEqual(prev, next), true);
  assert.notEqual(prev.chatItems[0], next.chatItems[0]);
  assert.equal(auxSnapshotsEqual(prev, prev), true);
  assert.equal(auxSnapshotsEqual(null, null), true);
  assert.equal(auxSnapshotsEqual(null, { chatItems: [] }), true);
});

test('auxSnapshotsEqual 捕捉流式原地修改与 busy/排队变化', () => {
  // 真实 buffer 语义：同一份条目对象被流式 delta 原地改写，两次 pull 各自经
  // 桥的逐条拷贝得到内容不同的新对象——必须判不等，否则面板冻结在首帧。
  // （桥内不拷贝时两次 pull 是同一引用，auxItemsEqual 的引用短路会误判相等
  // ——这正是 session_buffer_eviction 里"桥必须逐条拷贝"行为测试钉住的点。）
  const bufferItem = { id: 1, type: 'assistant', text: '流式', streaming: true };
  const prev = normalizeAuxSnapshot({ chatItems: [{ ...bufferItem }], busy: true, queued: [] });
  bufferItem.text = '流式中';
  const streamed = normalizeAuxSnapshot({ chatItems: [{ ...bufferItem }], busy: true, queued: [] });
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
