import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

// 复现 + 验证 applySessionSnapshot 的 pre-existing bug 修复:
//   原 bug:renderSnapshot 用 messages.innerHTML='' 整体重建历史,会把刚发生
//   的实时 addSystem 提示(附件已就绪 / 下载失败 / 连接恢复等)一并擦掉,用户
//   在下一次 snapshot 到达时看不到刚刚的事件反馈。
//   修复:addSystem 在非 snapshot 渲染期给节点打 data-ephemeral 标记,
//   renderSnapshot 在 wipe 前抽出这些节点,历史渲染完后再追加回去。
//
// 该文件镜像 mobile-download.test.js / mobile-advanced-controls.test.js 的
// jsdom + 真实 web/index.html 模式,DOM 与页面代码全部真实执行。

const html = await readFile(new URL('../web/index.html', import.meta.url), 'utf8');

function createPage() {
  const sent = [];
  const dom = new JSDOM(html, {
    url: 'https://relay.test/pinvou3/remote/r/rc_e2e#token=tok',
    runScripts: 'dangerously',
    pretendToBeVisual: true,
    beforeParse(window) {
      class FakeWebSocket {
        constructor() {
          this.readyState = FakeWebSocket.OPEN;
          FakeWebSocket.instance = this;
        }
        send(raw) { sent.push(JSON.parse(raw)); }
        close() {}
      }
      FakeWebSocket.OPEN = 1;
      window.WebSocket = FakeWebSocket;
    },
  });
  const { window } = dom;
  window.handleRelayMessage({ type: 'mobile_joined', room_id: 'rc_e2e', session_id: 'sess1' });
  // mobile_joined 会置 sessionChoiceRequired=true 并打开 session 选择面板,
  // 这会让 handleDesktopEvent 对 session_snapshot 直接 break。这里调用
  // enterRemoteSession('sess1') 把面板关掉并清掉 choice required,与真实
  // 用户「点 session 进入」一致,使后续 session_snapshot 能被正常处理。
  window.enterRemoteSession('sess1');
  sent.length = 0;
  return { window, sent, close: () => window.close() };
}

function systemTexts(window) {
  return [...window.document.querySelectorAll('#messages > .system')].map((n) => n.textContent);
}

function ephemeralSystemTexts(window) {
  return [...window.document.querySelectorAll('#messages > .system[data-ephemeral="true"]')]
    .map((n) => n.textContent);
}

function pushSnapshot(window, { chatItems = [], messages = [], sessionId = 'sess1', title = 'S1' } = {}) {
  window.handleDesktopEvent({
    type: 'session_snapshot',
    payload: {
      snapshot_source: 'live',
      session: { id: sessionId, title },
      chat_items: chatItems,
      messages,
    },
  });
}

test('实时 addSystem 在下一次 snapshot 重渲染后仍保留', () => {
  const { window, close } = createPage();
  try {
    pushSnapshot(window, {
      chatItems: [{ type: 'assistant', text: '历史消息 #1' }],
    });
    // 模拟实时事件:附件上传完成。
    window.addSystem('附件 small.txt 已就绪。');
    const node = window.document.querySelector('#messages > .system[data-ephemeral="true"]');
    assert.ok(node, 'addSystem 应在非 snapshot 期给节点打 data-ephemeral 标记');
    assert.equal(node.textContent, '附件 small.txt 已就绪。');

    // 推第二个 snapshot,触发 renderSnapshot 的 innerHTML='' 重建。
    pushSnapshot(window, {
      chatItems: [
        { type: 'assistant', text: '历史消息 #1' },
        { type: 'assistant', text: '历史消息 #2' },
      ],
    });

    const texts = systemTexts(window);
    assert.ok(
      texts.some((t) => t.includes('small.txt 已就绪')),
      `snapshot 重渲染后应保留实时 system 提示,实际 system 节点:${JSON.stringify(texts)}`,
    );
    // 历史消息应被正确重建为 2 条 assistant(.msg.assistant)。
    const assistantCount = window.document.querySelectorAll('#messages > .msg.assistant').length;
    assert.equal(assistantCount, 2, 'snapshot 重渲染应重建 2 条 assistant 历史消息');
  } finally {
    close();
  }
});

test('chat_items 重建的 system 节点不 ephemeral,且随新 snapshot 自然替换', () => {
  const { window, close } = createPage();
  try {
    pushSnapshot(window, {
      chatItems: [
        { type: 'careful_blocked' },
        { type: 'system', text: '历史系统提示' },
        { type: 'persona_equip', card: { title: '代码评审员' } },
      ],
    });
    // 三个 chat_item 衍生的 system 节点都不应有 ephemeral 标记。
    const all = [...window.document.querySelectorAll('#messages > .system')];
    assert.equal(all.length, 3, 'careful_blocked / system / persona_equip 各应渲染一个 system 节点');
    assert.equal(
      all.filter((n) => n.getAttribute('data-ephemeral') === 'true').length,
      0,
      'snapshot 渲染期产生的 system 节点都不应 ephemeral',
    );

    // 下一个 snapshot 不再包含这些 chat_items,system 节点应消失。
    pushSnapshot(window, {
      chatItems: [{ type: 'assistant', text: '新一轮对话' }],
    });
    const after = [...window.document.querySelectorAll('#messages > .system')];
    assert.equal(after.length, 0, 'chat_items 衍生 system 节点应随新 snapshot 替换,不应残留');
  } finally {
    close();
  }
});

test('多条实时 addSystem 跨 snapshot 保留出现顺序', () => {
  const { window, close } = createPage();
  try {
    pushSnapshot(window, { chatItems: [{ type: 'assistant', text: 'A' }] });
    window.addSystem('附件 a.txt 已就绪。');
    window.addSystem('附件 b.txt 已就绪。');
    window.addSystem('桌面端已恢复连接。');

    pushSnapshot(window, { chatItems: [{ type: 'assistant', text: 'A' }, { type: 'assistant', text: 'B' }] });

    const texts = ephemeralSystemTexts(window);
    assert.deepEqual(
      texts,
      ['附件 a.txt 已就绪。', '附件 b.txt 已就绪。', '桌面端已恢复连接。'],
      `应按原顺序保留所有 ephemeral system,实际:${JSON.stringify(texts)}`,
    );
  } finally {
    close();
  }
});

test('空 session 推 snapshot 时,有 ephemeral 提示则不再显示「暂无历史消息」占位', () => {
  const { window, close } = createPage();
  try {
    window.addSystem('桌面端已恢复连接。');
    pushSnapshot(window, { chatItems: [] });

    const empty = window.document.querySelector('#messages > .empty');
    assert.equal(empty, null, '有 ephemeral 提示时不应再显示 empty 占位');
    assert.ok(
      ephemeralSystemTexts(window).some((t) => t.includes('桌面端已恢复连接')),
      'ephemeral 提示应保留',
    );
  } finally {
    close();
  }
});

test('空 session 无 ephemeral 提示时仍显示「暂无历史消息」占位(回归基线)', () => {
  const { window, close } = createPage();
  try {
    pushSnapshot(window, { chatItems: [] });
    const empty = window.document.querySelector('#messages > .empty');
    assert.ok(empty, '没有任何提示时空占位应正常显示');
  } finally {
    close();
  }
});

test('snapshot 内 addSystem 的 .text 内容被正确重建(无 ephemeral 残留)', () => {
  const { window, close } = createPage();
  try {
    pushSnapshot(window, {
      chatItems: [{ type: 'system', text: '过去某轮的 system 提示' }],
    });
    pushSnapshot(window, {
      chatItems: [
        { type: 'system', text: '过去某轮的 system 提示' },
        { type: 'system', text: '另一条历史 system 提示' },
      ],
    });
    const texts = systemTexts(window);
    assert.deepEqual(
      texts,
      ['过去某轮的 system 提示', '另一条历史 system 提示'],
      `chat_items 的 system 应按 snapshot 内容重建,实际:${JSON.stringify(texts)}`,
    );
    assert.equal(
      window.document.querySelectorAll('#messages > .system[data-ephemeral="true"]').length,
      0,
      'snapshot 重建的 system 节点不应被错误地标记为 ephemeral',
    );
  } finally {
    close();
  }
});
