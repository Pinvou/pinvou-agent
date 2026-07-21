import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

// 用 jsdom 加载真实 web/index.html(远控手机端页面),驱动其中真实的
// handleDesktopEvent / renderKnowledgeSheet / renderToolsSheet / requestAttachFile
// / submitComposer / enterRemoteSession 代码,覆盖 KB 挂载、工具开关、附件上传
// 完整链路、abort 清理、tools_changed debounce、enterRemoteSession 拉 KB+tools
// 共 6 组场景。WebSocket / createObjectURL / a.click 用桩替代,其余 DOM 与页面
// 代码全部真实执行(镜像 mobile-download.test.js 的模式)。

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
      window.URL.createObjectURL = (blob) => {
        window.__capturedBlob = blob;
        return 'blob:captured';
      };
      window.URL.revokeObjectURL = () => {};
      window.HTMLAnchorElement.prototype.click = function click() {
        window.__downloadName = this.download;
      };
    },
  });
  const { window } = dom;
  // 标记 mobile 已加入房间(joined=true),并绑定当前 session。
  window.handleRelayMessage({ type: 'mobile_joined', room_id: 'rc_e2e', session_id: 'sess1' });
  // 清掉 mobile_joined 时 openSheet('sessions') 触发的 request_session_list 等,
  // 让每个测试从干净基线起跑。
  sent.length = 0;
  return { window, sent, close: () => window.close() };
}

function systemTexts(window) {
  return [...window.document.querySelectorAll('.system')].map((node) => node.textContent);
}

// 构造一个 jsdom 可用的 fake File:支持 .name / .size / .type / .slice().arrayBuffer()。
// 注意:Node Buffer 共享 8KB 池,.subarray().buffer 会返回整池;这里先把字节
// copy 进严格等长的独立 ArrayBuffer,保证 arrayBuffer() 返回的就是文件本身。
function makeFakeFile({ name = 'note.txt', type = 'text/plain', bytes }) {
  const src = bytes instanceof Uint8Array ? bytes : Buffer.from(bytes);
  const whole = new ArrayBuffer(src.length);
  new Uint8Array(whole).set(src);
  return {
    name,
    size: src.length,
    type,
    slice(start, end) {
      const s = start || 0;
      const e = end == null ? src.length : Math.min(end, src.length);
      const sub = new Uint8Array(whole, s, Math.max(0, e - s));
      const copy = new ArrayBuffer(sub.length);
      new Uint8Array(copy).set(sub);
      return { async arrayBuffer() { return copy; } };
    },
  };
}

// 在 sheet body 中按 title 文本找到对应 sheet-item 按钮。
function findSheetItemByTitle(window, title) {
  return [...window.document.querySelectorAll('#sheetBody .sheet-item')].find((btn) => {
    const t = btn.querySelector('.sheet-title');
    return t && t.textContent === title;
  });
}

// 等待若干 tick,让 setTimeout 回调与微任务跑完。
function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

test('KB snapshot 渲染列表,点击触发 mount_kb_collection,kb_mount_changed 更新 chip', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  // 打开知识库 sheet(会发 list_kb_collections)。
  window.openSheet('knowledge');
  sent.length = 0;
  // 注入 KB 快照:2 个 collection,未挂载。
  window.handleDesktopEvent({
    type: 'kb_collections_snapshot',
    payload: {
      collections: [
        { id: 'kb_alpha', name: 'Alpha KB', description: 'alpha notes' },
        { id: 'kb_beta', name: 'Beta KB', description: 'beta notes' },
      ],
      mounted_collection_id: null,
    },
  });

  const items = window.document.querySelectorAll('#sheetBody .sheet-item');
  assert.equal(items.length, 2, '应渲染 2 个 collection 条目');
  assert.ok(findSheetItemByTitle(window, 'Alpha KB'), '应包含 Alpha KB 条目');
  assert.ok(findSheetItemByTitle(window, 'Beta KB'), '应包含 Beta KB 条目');

  // 点击 Alpha KB 触发 mount_kb_collection。
  findSheetItemByTitle(window, 'Alpha KB').click();
  const mountAction = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'mount_kb_collection',
  );
  assert.ok(mountAction, '应发送 mount_kb_collection');
  assert.deepEqual(mountAction.payload.payload, { collection_id: 'kb_alpha' });

  // 注入 kb_mount_changed → chip label 更新为挂载中的 collection 名称。
  window.handleDesktopEvent({
    type: 'kb_mount_changed',
    payload: { session_id: 'sess1', collection_id: 'kb_alpha' },
  });
  const chipText = window.document.getElementById('knowledgeChip').textContent;
  assert.match(chipText, /Alpha KB/, 'chip 应显示挂载中的 collection 名');
});

test('Tools snapshot 渲染 3 项,切换后 500ms debounce 触发 set_disabled_connectors', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  // 打开工具 sheet(会发 list_tools)。
  window.openSheet('tools');
  sent.length = 0;
  // 注入 tools 快照:字段名是 id(不是 connector_id)。
  window.handleDesktopEvent({
    type: 'tools_snapshot',
    payload: {
      all: [
        { id: 'tool_a', name: 'ToolA', description: 'a' },
        { id: 'tool_b', name: 'ToolB', description: 'b' },
        { id: 'tool_c', name: 'ToolC', description: 'c' },
      ],
      disabled_ids: [],
    },
  });

  const items = window.document.querySelectorAll('#sheetBody .sheet-item');
  assert.equal(items.length, 3, '应渲染 3 个工具条目');

  // 点击 ToolB(已启用 → 切到禁用)。立即重渲染,mark 应变为「已禁用」。
  const toolB = findSheetItemByTitle(window, 'ToolB');
  assert.ok(toolB, '应能找到 ToolB 条目');
  toolB.click();
  const toolBAfter = findSheetItemByTitle(window, 'ToolB');
  assert.match(toolBAfter.querySelector('.sheet-mark').textContent, /已禁用/, '点击后 mark 应为「已禁用」');

  // debounce 500ms 后才发 set_disabled_connectors。
  assert.ok(
    !sent.some((m) => m.type === 'mobile_action' && m.payload.type === 'set_disabled_connectors'),
    '500ms 内不应发送 set_disabled_connectors',
  );
  await sleep(560);
  const setAction = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'set_disabled_connectors',
  );
  assert.ok(setAction, '500ms 后应发送 set_disabled_connectors');
  assert.deepEqual(setAction.payload.payload, { connector_ids: ['tool_b'] });

  // chip label 也应反映 2/3 启用。
  const chipText = window.document.getElementById('toolsChip').textContent;
  assert.match(chipText, /2\/3/, 'toolsChip 应显示 2/3');
});

test('附件上传完整链路:attach_file_start → chunks → result', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  // 单块能放下:chunk_bytes=768KiB,文件 11 字节 → 只 1 块,且 last=true。
  const file = makeFakeFile({ name: 'hello.txt', type: 'text/plain', bytes: 'hello world' });
  const uploadP = window.requestAttachFile(file);

  // 1) 应先发 attach_file_start,带 filename/byte_size/mime。
  await sleep(10);
  const startAction = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_start',
  );
  assert.ok(startAction, '应发送 attach_file_start');
  assert.equal(startAction.payload.payload.filename, 'hello.txt');
  assert.equal(startAction.payload.payload.byte_size, 11);
  assert.equal(startAction.payload.payload.mime, 'text/plain');

  // 2) 注入 attach_file_start_ack → 页面开始发 attach_file_chunk。
  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: 'up_1', chunk_bytes: 768 * 1024 },
  });
  await sleep(20);
  const chunkActions = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
  );
  assert.ok(chunkActions.length >= 1, '收到 start_ack 后应发 attach_file_chunk');
  const firstChunk = chunkActions[0];
  assert.equal(firstChunk.payload.payload.upload_id, 'up_1');
  assert.equal(firstChunk.payload.payload.index, 0);
  assert.equal(firstChunk.payload.payload.last, true, '单块文件最后一块 last=true');
  // base64 解码后字节应与源一致。
  const decoded = Buffer.from(firstChunk.payload.payload.data_base64, 'base64');
  assert.deepEqual(decoded, Buffer.from('hello world'), 'chunk base64 应解码回源字节');

  // 3) 注入 attach_file_relay_ack(ok) → 解锁下一块(此处只有一块,直接进入等 result)。
  window.handleDesktopEvent({
    type: 'attach_file_relay_ack',
    payload: { upload_id: 'up_1', index: 0, ok: true },
  });

  // 4) 注入 attach_file_result(ok + ingest_preview) → 状态 done + 系统消息。
  window.handleDesktopEvent({
    type: 'attach_file_result',
    payload: {
      upload_id: 'up_1',
      ok: true,
      ingest_preview: { kind: 'text', byte_size: 11, token_estimate: 3 },
    },
  });
  await uploadP;

  // done 的附件卡片应出现「已就绪」相关 sub 文本。
  const subText = window.document.querySelector('#attachmentPreview .att-sub');
  assert.ok(subText && /text/.test(subText.textContent), '附件卡片应显示 preview kind');
  assert.ok(
    systemTexts(window).some((s) => s.includes('hello.txt 已就绪')),
    '系统消息应提示附件已就绪',
  );

  // 5) 模拟提交:input 文本 + 提交 → user_message 应带 attachment_upload_ids。
  window.document.getElementById('input').value = '看看这个';
  // 提交前 actionButton 应处于 idle(turn.active=false),直接调用 submitComposer。
  window.submitComposer();
  const userMsg = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'user_message',
  );
  assert.ok(userMsg, '应发送 user_message');
  assert.deepEqual(userMsg.payload.payload.attachment_upload_ids, ['up_1']);
  assert.equal(userMsg.payload.payload.content, '看看这个');
});

test('多块附件上传:多 chunk 串行 + 每块 relay_ack 解锁下一块', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  // chunk_bytes=4,文件 10 字节 → 3 块(4/4/2)。
  const payload = Buffer.from('0123456789');
  const file = makeFakeFile({ name: 'data.bin', type: 'application/octet-stream', bytes: payload });
  const uploadP = window.requestAttachFile(file);
  await sleep(10);

  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: 'up_multi', chunk_bytes: 4 },
  });

  // 预期 3 块,逐块给 relay_ack 解锁下一块。
  for (let i = 0; i < 3; i += 1) {
    await sleep(15);
    const chunks = sent.filter(
      (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
    );
    assert.ok(chunks.length >= i + 1, `应已发出第 ${i + 1} 块`);
    const c = chunks[i].payload.payload;
    assert.equal(c.upload_id, 'up_multi');
    assert.equal(c.index, i);
    assert.equal(c.last, i === 2, '只有最后一块 last=true');
    window.handleDesktopEvent({
      type: 'attach_file_relay_ack',
      payload: { upload_id: 'up_multi', index: i, ok: true },
    });
  }

  window.handleDesktopEvent({
    type: 'attach_file_result',
    payload: { upload_id: 'up_multi', ok: true, ingest_preview: { kind: 'binary', byte_size: 10 } },
  });
  await uploadP;
  const chunks = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
  );
  assert.equal(chunks.length, 3, '共应发 3 块');
  // 校验字节重组一致性。
  const reassembled = Buffer.concat(
    chunks.map((c) => Buffer.from(c.payload.payload.data_base64, 'base64')),
  );
  assert.deepEqual(reassembled, payload, '3 块重组字节应与源一致');
});

test('attach_file_aborted 清状态并提示', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  const file = makeFakeFile({ name: 'big.bin', type: 'application/octet-stream', bytes: Buffer.alloc(20, 0xab) });
  // 故意 chunk_bytes=4 让上传走多块,中途 abort。
  window.requestAttachFile(file).catch(() => {});
  await sleep(10);
  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: 'up_abort', chunk_bytes: 4 },
  });
  await sleep(15);
  // 上传中,卡片应可见。
  assert.ok(
    !window.document.getElementById('attachmentPreview').classList.contains('hidden'),
    '上传中附件预览应可见',
  );

  // 注入 abort 事件。
  window.handleDesktopEvent({
    type: 'attach_file_aborted',
    payload: { upload_id: 'up_abort', reason: 'user_cancelled' },
  });
  await sleep(10);

  // 附件预览应清空(uploads[up_abort] 被 delete)。
  assert.ok(
    window.document.getElementById('attachmentPreview').classList.contains('hidden'),
    'abort 后附件预览应隐藏',
  );
  assert.ok(
    systemTexts(window).some((s) => s.includes('附件上传已中止') && s.includes('user_cancelled')),
    '系统消息应提示附件已中止及原因',
  );
});

test('tools_changed 500ms debounce 后触发 list_tools', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  sent.length = 0;
  window.handleDesktopEvent({ type: 'tools_changed', payload: {} });
  // 立即不应发 list_tools(debounce 500ms)。
  assert.ok(
    !sent.some((m) => m.type === 'mobile_action' && m.payload.type === 'list_tools'),
    'debounce 窗口内不应发 list_tools',
  );
  await sleep(560);
  const listAction = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'list_tools',
  );
  assert.ok(listAction, '500ms 后应发送 list_tools');
});

test('enterRemoteSession 同 session 时拉 KB + tools(并附带 snapshot/chips/artifacts)', (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  sent.length = 0;
  // 当前 sessionId=sess1,进入同 session → 走 request_snapshot + 拉 KB/tools 分支。
  window.enterRemoteSession('sess1');
  const types = sent
    .filter((m) => m.type === 'mobile_action')
    .map((m) => m.payload.type);
  assert.ok(types.includes('list_kb_collections'), '应发送 list_kb_collections');
  assert.ok(types.includes('list_tools'), '应发送 list_tools');
  assert.ok(types.includes('request_snapshot'), '同 session 进入应发 request_snapshot');
});

test('enterRemoteSession 切到不同 session 时发 switch_remote_session + 拉 KB + tools', (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  sent.length = 0;
  window.enterRemoteSession('sess2');
  const types = sent
    .filter((m) => m.type === 'mobile_action')
    .map((m) => m.payload.type);
  assert.ok(types.includes('switch_remote_session'), '应发送 switch_remote_session');
  assert.ok(types.includes('list_kb_collections'), '切 session 后应发送 list_kb_collections');
  assert.ok(types.includes('list_tools'), '切 session 后应发送 list_tools');
});
