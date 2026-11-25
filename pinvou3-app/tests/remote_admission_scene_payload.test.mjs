// 远程准入（chat:user_message）只把用户亲口说的话上屏为用户气泡。
// 另一台设备在场景模式下发送时，准入事件携带的是送模全文（原文 + 路由提示词），
// 接收端若原样上屏，气泡里就带着整段「系统提示词」——与历史重放同一类泄漏，
// 走同一个解析（features/chat/scene-payload.js 的 splitScenePayload，发布在
// window 全局供 classic-script 桥惰性读取）。两条 lane 各有一份处理函数，逐个驱动。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

import { splitScenePayload } from '../src/features/chat/scene-payload.js';
import {
  SUFFIX_SCENE_ROUTES,
  createDataVisualizationMessageMeta,
  createDocumentWritingMessageMeta,
  createPptDesignMessageMeta,
} from '../src/features/chat/work-scene-routes.js';
import { createVisualPosterMessageMeta } from '../src/features/chat/visual-poster-scene.js';
import { createPersonalWorkbenchMessageMeta } from '../src/features/chat/personal-workbench-scene.js';
import { buildSessionMentionBlock } from '../src/features/chat/session-mention.js';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');
const plain = value => JSON.parse(JSON.stringify(value));

// The handler is a self-contained function inside each lane's bridge; lift its
// exact source out of the real file and run it against the real classic
// classifier with the lane closures stubbed.
function loadAdmission(runtime, { publisher = false } = {}) {
  const window = {};
  const context = vm.createContext({ window, console });
  vm.runInContext(read('src', 'shared', 'bridge-messages.js'), context, { filename: 'bridge-messages.js' });
  if (publisher) window.__PINVOU_SCENE_PAYLOAD__ = { splitScenePayload };
  const source = read(...(runtime === 'web'
    ? ['src', 'platform', 'web', 'bridge.js']
    : ['src', 'platform', 'tauri', 'bridge', 'chat-events.js']));
  const signature = '  function applyRemoteUserMessageEvent(e, force) {';
  const start = source.indexOf(signature);
  // Extraction end marker (rebase review P2): the old web marker was the
  // planCardHydrationKey definition, but that helper now lives in the shared
  // base (Debtsweep L4 moved it to shared/bridge-shared-helpers.js and the
  // bridge consumes the shorthand), so the marker no longer existed in
  // web/bridge.js and every web case died at extraction time. The next
  // standalone function in the same lane file is the stable boundary now.
  const endMarker = runtime === 'web'
    ? '\n  function applyDeferredRemoteUserMessage(_sid, buf)'
    : '\n  listen("chat:user_message", async function (e) {';
  const end = source.indexOf(endMarker, start);
  assert.ok(start > 0 && end > start, `${runtime}: applyRemoteUserMessageEvent must stay a standalone function`);
  const state = { activeSessionId: 's1', chatItems: [], busy: false, thinking: { active: false } };
  const buffer = { messages: [], remoteAdmissionKeys: [], loadedFromDisk: false, runtimeContextUpdatesPending: 0 };
  Object.assign(context, {
    state,
    context: { currentStreamText: '', currentStreamId: 7 },
    getBuffer: () => buffer,
    isInternalRuntimeUserMessage: window.PinvouBridgeMessages.isInternalRuntimeUserMessage,
    userMessageDisplayText: () => '',
    markRemoteTurn() {},
    applyAuthoritativeModeState() {},
    runSyncOnSession(_sid, callback) { callback(); },
    bt: key => key,
    flushPendingStreamRender() {},
    resetPendingAssistant() {},
    startThinking() { state.thinking.active = true; },
    notify() {},
    timeStr: () => '12:00',
    addChatItem(item) { item.id = state.chatItems.length + 1; state.chatItems.push(item); },
  });
  const apply = vm.runInContext(`(${source.slice(start, end).trim()})`, context);
  return {
    // Each admission resets the lane, so none is dropped as a duplicate.
    admit(content) {
      state.chatItems.length = 0;
      buffer.remoteAdmissionKeys.length = 0;
      return apply({ id: 'event-1', payload: { session_id: 's1', content, operation: 'append', base_transcript_revision: 'rev-1' } }, false);
    },
    lastBubble: () => {
      const users = state.chatItems.filter(item => item.type === 'user');
      return users.length ? plain(users.at(-1)) : null;
    },
  };
}

// [scene, message meta, the words the user typed]
const sceneSends = [
  ['公文写作', createDocumentWritingMessageMeta('写一份通知'), '写一份通知'],
  ['数据可视化', createDataVisualizationMessageMeta('画一张销量折线图'), '画一张销量折线图'],
  ['PPT 设计', createPptDesignMessageMeta('做一份年度总结'), '做一份年度总结'],
  ['视觉海报', createVisualPosterMessageMeta('做一张发布会海报'), '做一张发布会海报'],
  ['个人工作台', createPersonalWorkbenchMessageMeta('运动'), '运动'],
];

for (const runtime of ['tauri', 'web']) {
  test(`${runtime}: a scene payload admitted from another device renders as the user's own words`, () => {
    for (const [name, meta, typed] of sceneSends) {
      const h = loadAdmission(runtime, { publisher: true });
      assert.equal(h.admit(meta.pinvouPayloadText), true, `${name}: the admission is accepted`);
      const bubble = h.lastBubble();
      assert.equal(bubble.text, typed, `${name}: the routing copy is model-facing`);
      assert.equal(bubble.pinvouScene, meta.pinvouScene, `${name}: the wrapper carries the scene identity`);
      assert.equal(bubble.time, '12:00');
      if (runtime === 'tauri') {
        assert.equal(bubble.runtimeContextUpdatesBefore, 0, 'the desktop admission keeps its ordinal counter field');
      }
    }
  });

  test(`${runtime}: a workbench admission behind a canonical mention block keeps the block and drops the wrapper`, () => {
    // 另一台设备在个人工作台（默认提示词、带 @ 引用）发送时，准入事件携带
    // 「块 + 载荷」复合全文。逐字节精确那一级在复合形态下同样生效（无需
    // sidecar），块原样保留给气泡解析成引用卡片。
    const h = loadAdmission(runtime, { publisher: true });
    const mentionBlock = buildSessionMentionBlock([{ sessionId: 's-1', title: '上次会话' }]);
    const meta = createPersonalWorkbenchMessageMeta('运动');
    assert.equal(h.admit(mentionBlock + meta.pinvouPayloadText), true);
    const bubble = h.lastBubble();
    assert.equal(bubble.text, mentionBlock + '运动', 'the block survives byte-exact; only the routing wrapper is dropped');
    assert.equal(bubble.pinvouScene, meta.pinvouScene, 'the wrapper carries the scene identity');
    // 仅块无载荷（或块 + 手写文本）不是可识别载荷：原样上屏。
    const plain = loadAdmission(runtime, { publisher: true });
    plain.admit(mentionBlock + '我自己写的话');
    assert.equal(plain.lastBubble().text, mentionBlock + '我自己写的话');
    assert.equal(plain.lastBubble().pinvouScene, undefined);
  });

  test(`${runtime}: an ordinary admission is unchanged`, () => {
    const h = loadAdmission(runtime, { publisher: true });
    for (const content of ['继续', '第一段\n\n---\n第二段', 'Pinvou 公文写作场景路由：\n我手敲的标题']) {
      assert.equal(h.admit(content), true);
      assert.equal(h.lastBubble().text, content);
      assert.equal(h.lastBubble().pinvouScene, undefined, 'no wrapper recognized: nothing to tag');
    }
  });

  test(`${runtime}: the live path strips only byte-exact wrappers`, () => {
    // No scene sidecar exists for a remote admission, so the header-anchored
    // guess for older routing copy stays off.
    const documentRoute = SUFFIX_SCENE_ROUTES.find(route => route.key === 'document-writing');
    const stale = ['写一份通知', '', '---', documentRoute.context.split('\n')[0], '- 旧版本的路由正文。'].join('\n');
    const h = loadAdmission(runtime, { publisher: true });
    h.admit(stale);
    assert.equal(h.lastBubble().text, stale);
  });

  test(`${runtime}: without the features publisher the admission text stays as is`, () => {
    const meta = createDocumentWritingMessageMeta('写一份通知');
    const h = loadAdmission(runtime);
    h.admit(meta.pinvouPayloadText);
    assert.equal(h.lastBubble().text, meta.pinvouPayloadText);
  });

  test(`${runtime}: an internal runtime message still renders nothing`, () => {
    const h = loadAdmission(runtime, { publisher: true });
    h.admit([
      '<context_update>',
      'Session context changed since it was pinned; the pinned system prompt is unchanged. Delta (+ added, - removed):',
      '- stale line',
      '</context_update>',
    ].join('\n'));
    assert.equal(h.lastBubble(), null);
  });
}
