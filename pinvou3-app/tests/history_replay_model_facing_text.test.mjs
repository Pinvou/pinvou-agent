// 历史重放（切会话 / 重开会话 / 压缩后的权威重载）只把用户亲口说的话上屏为
// 用户气泡。落盘的 user 消息里有两类只给模型看的文本，此前被原样渲染成
// 「用户输入」：
//   1. CodeWhale 压缩检查点——摘要以 role=user 提交进历史（带出处戳的载体）；
//   2. 场景路由载荷——发送时气泡显示原文，落盘的却是「原文 + 路由提示词」。
// 两条 lane（tauri / web）共用 bridge-shared-helpers.js 的
// rerenderFromMessagesCore；这里用真实宿主包装 + 真实 userMessageDisplayText /
// bridge-messages.js 驱动它，场景载荷的互逆解析用 features 侧发布的真实实现。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

import { splitScenePayload } from '../src/features/chat/scene-payload.js';
import {
  SUFFIX_SCENE_ROUTES,
  createDocumentWritingMessageMeta,
} from '../src/features/chat/work-scene-routes.js';
import { createVisualPosterMessageMeta } from '../src/features/chat/visual-poster-scene.js';
import { createPersonalWorkbenchMessageMeta } from '../src/features/chat/personal-workbench-scene.js';
import { projectDeepSeekConversation, turnHasUserText } from '../src/features/conversation/deepseek-conversation.js';
import { forkEntriesByTurnId } from '../src/features/chat/forkTurnEntries.js';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

const SUMMARY_HEADER = 'Another language model started to solve this problem and produced a summary of its thinking process.';
const SUMMARY_TEXT = `${SUMMARY_HEADER}\n\n## Task\nKeep the compaction fixture short.`;
const PROVENANCE = '<!-- codewhale.compaction-checkpoint.v1 -->';
const stampedCheckpoint = () => ({
  role: 'user',
  content: [{ type: 'text', text: SUMMARY_TEXT }, { type: 'text', text: PROVENANCE }],
});
const bareHeaderTurn = () => ({ role: 'user', content: [{ type: 'text', text: `${SUMMARY_TEXT}\n\nPlease continue from this pasted handoff.` }] });

// A bare string becomes a text block; block objects pass through.
const block = value => (typeof value === 'string' ? { type: 'text', text: value } : value);
const tool = (id, name = 'read', input = {}) => ({ type: 'tool_use', id, name, input });
const result = (id, content, is_error = false) => ({ type: 'tool_result', tool_use_id: id, content, is_error });
const assistant = (...content) => ({ role: 'assistant', content: content.map(block) });
const user = (...content) => ({ role: 'user', content: content.map(block) });
const plain = value => JSON.parse(JSON.stringify(value));

function createReplay(runtime, { bridgeMessages = true, scenePublisher = false } = {}) {
  const state = { messages: [], chatItems: [], activeSessionId: 'replay-session', busy: false, pinvouSceneEvents: [], personaEvents: [] };
  const window = {};
  const context = vm.createContext({ window, console, TextEncoder });
  if (bridgeMessages) {
    vm.runInContext(read('src', 'shared', 'bridge-messages.js'), context, { filename: 'bridge-messages.js' });
  }
  if (scenePublisher) {
    // What features/chat/scene-payload.js publishes at module load.
    window.__PINVOU_SCENE_PAYLOAD__ = { splitScenePayload };
  }
  vm.runInContext(read('src', 'shared', 'bridge-shared-helpers.js'), context, { filename: 'bridge-shared-helpers.js' });
  let itemIdSeq = 0;
  const deps = {
    state,
    addChatItem(item) { item.id = ++itemIdSeq; state.chatItems.push(item); },
    bt: key => key,
    BT_TABLE: { zh: {}, en: {}, ja: {} },
    DELIVERABLE_EXTS: ['.md'],
  };
  const shared = window.PinvouBridgeShared.create(runtime === 'web' ? 'web' : 'tauriMain', deps);
  const terminal = window.PinvouBridgeShared.create(runtime === 'web' ? 'web' : 'tauriTerminal', deps);
  const hostSource = read('src', 'platform', runtime, 'bridge.js');
  const start = hostSource.indexOf('.rerenderFromMessagesCore({') + '.rerenderFromMessagesCore('.length;
  const end = hostSource.indexOf('}, opts);', start) + 1;
  assert.ok(start > 0 && end > start, `${runtime}: rerenderFromMessages lane wrapper must exist`);
  const steerChecks = [];
  Object.assign(context, {
    state, itemIdSeq: 0, toolMeta: {},
    emitPersonaAt: () => {},
    userMessageDisplayText: shared.userMessageDisplayText,
    updateToolItem: terminal.updateToolItem,
    PLAN_TOOLS: new Set(['update_plan', 'checklist_write', 'todo_write']),
    renderMarkdown: text => `<p>${text}</p>`,
    isSteeredMessagePos: (position, text) => { steerChecks.push([position, text]); return false; },
    getBuffer: () => null,
  });
  const lane = vm.runInContext(`(${hostSource.slice(start, end)})`, context);
  return {
    state, steerChecks, shared,
    replay(messages, sceneEvents = []) {
      state.messages = messages;
      state.chatItems = [];
      state.pinvouSceneEvents = sceneEvents;
      steerChecks.length = 0;
      shared.rerenderFromMessagesCore(lane, {});
      return state.chatItems;
    },
  };
}

const lanes = ['tauri', 'web'];

for (const runtime of lanes) {
  test(`${runtime}: a stamped compaction checkpoint replays as a system notice, never as a user bubble`, () => {
    const h = createReplay(runtime);
    const rows = h.replay([
      user('修复登录页样式'),
      assistant({ type: 'text', text: '先看代码' }, tool('c1', 'read', { path: 'a.css' })),
      user(result('c1', 'body of a.css')),
      assistant({ type: 'text', text: '第一步完成' }),
      stampedCheckpoint(),
      assistant({ type: 'text', text: '压缩后继续' }),
      user('继续'),
    ]);
    assert.deepEqual(plain(rows.map(row => row.type)), ['user', 'assistant', 'tool', 'assistant', 'system', 'assistant', 'user']);
    assert.deepEqual(plain(rows.filter(row => row.type === 'user').map(row => row.text)), ['修复登录页样式', '继续']);
    assert.ok(!JSON.stringify(rows).includes('summary of its thinking process'), 'the model-facing summary must not reach any item');
    assert.ok(!JSON.stringify(rows).includes(PROVENANCE), 'the provenance marker must not reach any item');
    // The notice reuses the live "context compacted" copy and carries no live
    // request identity: the manual-compact reconciliation in ChatView only
    // claims start items, and nothing may mistake the marker for one.
    assert.deepEqual(plain(rows[4]), { type: 'system', text: 'compactDone', time: '', compactionCheckpoint: true, id: 5 });
    // Neighbouring history is untouched: the tool card before the checkpoint
    // still backfills from its tool_result.
    assert.equal(rows[2].output, 'body of a.css');
    assert.equal(rows[2].state, 'done');
    assert.deepEqual(
      plain(projectDeepSeekConversation({ chatItems: rows }).turns.map(turn => turn.userText)),
      ['修复登录页样式', '继续'],
      'the checkpoint never becomes a turn',
    );
  });

  test(`${runtime}: fork entry ordinals are identical with and without a checkpoint between two turns`, () => {
    const turnsFor = rows => projectDeepSeekConversation({ chatItems: rows }).turns;
    const without = createReplay(runtime).replay([user('第一问'), assistant({ type: 'text', text: '答一' }), user('第二问'), assistant({ type: 'text', text: '答二' })]);
    const withCheckpoint = createReplay(runtime).replay([user('第一问'), assistant({ type: 'text', text: '答一' }), stampedCheckpoint(), user('第二问'), assistant({ type: 'text', text: '答二' })]);
    const ordinals = turns => [...forkEntriesByTurnId(turns).values()];
    assert.deepEqual(ordinals(turnsFor(withCheckpoint)), [1, 2]);
    assert.deepEqual(ordinals(turnsFor(withCheckpoint)), ordinals(turnsFor(without)),
      'the checkpoint must not shift the ordinals the backend counts (Editable classification skips it too)');
  });

  test(`${runtime}: a bare header-prefixed turn stays a user bubble — with or without a stamped carrier in the history`, () => {
    for (const history of [
      [user('接着做'), bareHeaderTurn(), assistant({ type: 'text', text: '好的' })],
      [stampedCheckpoint(), user('接着做'), bareHeaderTurn(), assistant({ type: 'text', text: '好的' })],
    ]) {
      const rows = createReplay(runtime).replay(history);
      const pasted = bareHeaderTurn().content[0].text;
      assert.ok(rows.some(row => row.type === 'user' && row.text === pasted),
        'a user turn that merely begins with the summary header is user content and stays visible');
      const notices = rows.filter(row => row.type === 'system');
      assert.equal(notices.length, history.length === 3 ? 0 : 1, 'only the stamped carrier produces the notice');
    }
  });

  test(`${runtime}: a scene payload replays as the user's own words and keeps its scene`, () => {
    const h = createReplay(runtime, { scenePublisher: true });
    const meta = createDocumentWritingMessageMeta('写一份通知');
    const rows = h.replay(
      [user(meta.pinvouPayloadText), assistant({ type: 'text', text: '已生成' })],
      [{ pos: 0, scene: meta.pinvouScene }],
    );
    const bubble = rows.find(row => row.type === 'user');
    assert.equal(bubble.text, '写一份通知', 'the routing boilerplate is model-facing and must not render as typed input');
    assert.equal(bubble.pinvouScene, 'work:document-writing', 'the sidecar scene is still applied');
    assert.equal(bubble.messageIndex, 0);
    assert.ok(!JSON.stringify(rows).includes('场景路由'), 'no scene boilerplate may reach any item');
    // Steer marking keeps reading the unstripped text (pre-strip behavior).
    // Only the desktop lane has a steer marker.
    if (runtime === 'tauri') {
      assert.ok(h.steerChecks.some(([, text]) => text === meta.pinvouPayloadText));
    }
  });

  test(`${runtime}: the payload itself restores the scene when the sidecar position drifted`, () => {
    // The engine inserts runtime messages ahead of a turn, so the sidecar's
    // client-side position no longer names the persisted message.
    const h = createReplay(runtime, { scenePublisher: true });
    const meta = createDocumentWritingMessageMeta('我刚刚触发了上下文压缩，你能看到之前你干了啥吗');
    const rows = h.replay(
      [user('第一问'), assistant({ type: 'text', text: '答一' }), user(meta.pinvouPayloadText)],
      [{ pos: 1, scene: meta.pinvouScene }],
    );
    const bubble = rows.filter(row => row.type === 'user')[1];
    assert.equal(bubble.text, '我刚刚触发了上下文压缩，你能看到之前你干了啥吗');
    assert.equal(bubble.pinvouScene, 'work:document-writing');
  });

  test(`${runtime}: every scene payload shape is inverted`, () => {
    const h = createReplay(runtime, { scenePublisher: true });
    const metas = [
      createDocumentWritingMessageMeta('写一份通知'),
      createVisualPosterMessageMeta('做一张发布会海报'),
      createPersonalWorkbenchMessageMeta('运动'),
    ];
    const rows = h.replay(metas.flatMap((meta, index) => [
      user(meta.pinvouPayloadText), assistant({ type: 'text', text: `回复 ${index}` }),
    ]));
    assert.deepEqual(
      plain(rows.filter(row => row.type === 'user').map(row => [row.text, row.pinvouScene])),
      [
        ['写一份通知', 'work:document-writing'],
        ['做一张发布会海报', 'design:poster'],
        ['运动', 'work:personal-workbench'],
      ],
    );
  });

  test(`${runtime}: an ordinary message with a horizontal rule is left alone`, () => {
    const text = '第一段\n\n---\n第二段\n\n---\nPinvou 随手写的一行\n不是路由';
    const rows = createReplay(runtime, { scenePublisher: true }).replay([user(text)]);
    assert.equal(rows.find(row => row.type === 'user').text, text);
  });

  test(`${runtime}: stale boilerplate is stripped only when the sidecar names that scene`, () => {
    const route = SUFFIX_SCENE_ROUTES.find(entry => entry.key === 'document-writing');
    // Routing copy changed across releases; the first header line never did.
    const stalePayload = [
      '写一份通知',
      '',
      '---',
      `${route.context.split('\n')[0]}`,
      '- 旧版本的路由正文。',
      '',
      '生成完成前执行旧版自检：',
      '1. 旧条目。',
    ].join('\n');
    const withSidecar = createReplay(runtime, { scenePublisher: true }).replay(
      [user(stalePayload)],
      [{ pos: 0, scene: 'work:document-writing' }],
    );
    assert.equal(withSidecar.find(row => row.type === 'user').text, '写一份通知');
    const withoutSidecar = createReplay(runtime, { scenePublisher: true }).replay([user(stalePayload)]);
    assert.equal(withoutSidecar.find(row => row.type === 'user').text, stalePayload,
      'without the scene on record the header anchor is only a guess: keep the text');
  });

  test(`${runtime}: without the features publisher the persisted payload stays visible (pre-fix behavior)`, () => {
    const meta = createDocumentWritingMessageMeta('写一份通知');
    const rows = createReplay(runtime).replay(
      [user(meta.pinvouPayloadText)],
      [{ pos: 0, scene: meta.pinvouScene }],
    );
    const bubble = rows.find(row => row.type === 'user');
    assert.equal(bubble.text, meta.pinvouPayloadText);
    assert.equal(bubble.pinvouScene, meta.pinvouScene, 'the sidecar scene is applied independently of the publisher');
  });

  test(`${runtime}: the compaction notice is still emitted when scene payloads are in the same history`, () => {
    const meta = createDocumentWritingMessageMeta('写一份通知');
    const rows = createReplay(runtime, { scenePublisher: true }).replay([
      user(meta.pinvouPayloadText), assistant({ type: 'text', text: '已生成' }), stampedCheckpoint(),
    ]);
    assert.deepEqual(plain(rows.map(row => row.type)), ['user', 'assistant', 'system']);
    assert.equal(rows[0].text, '写一份通知');
    assert.equal(rows[2].compactionCheckpoint, true);
  });
}

test('the projection still skips a header-prefixed userItem for fork/rewind ordinals', () => {
  // Pasted headers and pre-stamp carriers do reach the projection as
  // userItems; the backend does not count them as turns either.
  const pasted = bareHeaderTurn().content[0].text;
  assert.equal(turnHasUserText({ id: 'p', userItem: { type: 'user', text: pasted } }), false);
  assert.equal(turnHasUserText({ id: 'u', userItem: { type: 'user', text: '真实提问' } }), true);
});
