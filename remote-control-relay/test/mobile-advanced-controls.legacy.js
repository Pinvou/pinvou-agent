import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

// 历史 v1 页面回归；完整 WebUI v2 由 web-ui.smoke.cjs 覆盖。
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

  // 审查 #6:kb_search 只在 Plan 模式可用,chip 默认禁用。先注入一份 kb_search_available
  // 的 chips_snapshot,模拟 Plan 模式 + KB 就绪,让 chip 解锁、sheet 允许挂载。
  window.handleDesktopEvent({
    type: 'chips_snapshot',
    payload: { kb_search_available: true, mode: 'plan', mounted_collection: null },
  });
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

// 审查 #6 回归:kb_search 不可用时(非 Plan 模式),KB chip 必须置灰,sheet 不允许挂载。
test('kb_search_available=false(Yolo)时 KB chip 禁用 + sheet 挂载点击不发 mount_kb_collection', (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  // 非 Plan 模式(Yolo):kb_search 未注册到模型。
  window.handleDesktopEvent({
    type: 'chips_snapshot',
    payload: { kb_search_available: false, mode: 'yolo', mounted_collection: null },
  });
  const chip = window.document.getElementById('knowledgeChip');
  assert.ok(chip.disabled, '非 Plan 模式下 knowledgeChip 必须禁用(审查 #6)');
  assert.match(chip.title || '', /Plan/, 'chip title 应提示切到 Plan 模式');

  // 打开 sheet 注入 KB 快照:即便有 collection,点击也不应发 mount_kb_collection。
  window.openSheet('knowledge');
  sent.length = 0;
  window.handleDesktopEvent({
    type: 'kb_collections_snapshot',
    payload: {
      collections: [{ id: 'kb_x', name: 'X KB', description: 'x' }],
      mounted_collection_id: null,
    },
  });
  // sheet 顶部应有「不支持知识库检索」提示。
  const bodyText = window.document.getElementById('sheetBody').textContent;
  assert.match(bodyText, /不支持知识库检索/, 'sheet 应提示当前模式不可用');
  // 点击 collection 条目:不应发 mount_kb_collection(UI 侧预挡,服务端也会拒)。
  findSheetItemByTitle(window, 'X KB').click();
  const mountAction = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'mount_kb_collection',
  );
  assert.equal(mountAction, undefined, 'kb_search 不可用时点击 collection 不应发 mount_kb_collection');
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

  // 5) 模拟纯附件提交：user_message 内容可为空，但手机气泡必须显示附件名。
  window.document.getElementById('input').value = '';
  // 提交前 actionButton 应处于 idle(turn.active=false),直接调用 submitComposer。
  window.submitComposer();
  const userMsg = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'user_message',
  );
  assert.ok(userMsg, '应发送 user_message');
  assert.deepEqual(userMsg.payload.payload.attachment_upload_ids, ['up_1']);
  assert.equal(userMsg.payload.payload.content, '');
  const userBubbles = [...window.document.querySelectorAll('#messages .msg.user .bubble')];
  const userBubble = userBubbles[userBubbles.length - 1];
  assert.ok(userBubble && userBubble.textContent.includes('📎 hello.txt'));
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

test('start_ack 前点击 × 仍用稳定 upload_id 取消桌面上传', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  const file = makeFakeFile({
    name: 'cancel-before-ack.bin',
    type: 'application/octet-stream',
    bytes: Buffer.from('0123456789'),
  });
  const uploadP = window.requestAttachFile(file);
  await sleep(10);

  const start = sent.find(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_start',
  );
  assert.ok(start, '应发送 attach_file_start');
  const uploadId = start.payload.payload.upload_id;
  assert.match(uploadId, /^up_[A-Za-z0-9_-]+$/, 'start 必须携带客户端稳定 upload_id');

  const xBtn = window.document.querySelector('#attachmentPreview .att-x');
  assert.ok(xBtn, '等待 start_ack 时也应允许取消');
  xBtn.click();
  await sleep(10);

  const aborts = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_abort',
  );
  assert.ok(aborts.length >= 1, 'ACK 前取消必须发 attach_file_abort');
  assert.ok(
    aborts.every((m) => m.payload.payload.upload_id === uploadId),
    'start 与 abort 必须使用同一个 upload_id',
  );

  // 迟到 ACK 不得恢复上传，也不得发任何 chunk。
  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: uploadId, chunk_bytes: 4 },
  });
  await sleep(30);
  assert.equal(
    sent.filter((m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk').length,
    0,
    '取消后的迟到 ACK 不得触发分块上传',
  );
  assert.ok(!window.document.querySelector('#attachmentPreview .att-x'));
  await uploadP;
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

// 回归:start_ack 里恶意/异常的 chunk_bytes(负数 / 非整数 / 过大 / 非数字)不得被
// 直接当步长使用,否则会进入无限空块循环(负数 offset 递减)或内存爆炸。必须回退到
// 默认 768KiB(或合理区间)。
test('start_ack 的 chunk_bytes 异常值被回退到默认,不进入负步长循环', async (t) => {
  for (const bad of [-1, -99999, 0, 0.5, 1e12, '768', true, null, NaN, undefined]) {
    const { window, sent, close } = createPage();
    t.after(close);

    const file = makeFakeFile({ name: 'small.txt', type: 'text/plain', bytes: 'abcdef' });
    const uploadP = window.requestAttachFile(file);
    await sleep(10);

    // 注入畸形 chunk_bytes。
    window.handleDesktopEvent({
      type: 'attach_file_start_ack',
      payload: { upload_id: 'up_bad', chunk_bytes: bad },
    });
    // 给循环若干 tick 跑。
    await sleep(40);

    const chunks = sent.filter(
      (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
    );
    // 不应进入无限空块循环:正常情况下 6 字节文件只需 1 块(last=true)。
    // 若 chunk_bytes 被当负步长,offset 会不断递减,瞬间发出海量空块。
    assert.ok(
      chunks.length <= 2,
      `chunk_bytes=${bad} 不应触发海量空块(实际发出 ${chunks.length} 块)`,
    );
    assert.ok(chunks.length >= 1, `chunk_bytes=${bad} 应至少发 1 块(回退默认后正常切分)`);
    // 第一块应包含全部 6 字节(默认 768KiB 步长,6 字节单块),last=true。
    const decoded = Buffer.from(chunks[0].payload.payload.data_base64, 'base64');
    assert.equal(decoded.length, 6, `chunk_bytes=${bad} 回退默认后首块应是全部 6 字节`);
    assert.equal(chunks[0].payload.payload.last, true, `chunk_bytes=${bad} 单块文件首块应 last=true`);

    // 收尾:注入 result 让 uploadP resolve,避免泄漏到下一个用例。
    window.handleDesktopEvent({
      type: 'attach_file_relay_ack',
      payload: { upload_id: 'up_bad', index: 0, ok: true },
    });
    window.handleDesktopEvent({
      type: 'attach_file_result',
      payload: { upload_id: 'up_bad', ok: true, ingest_preview: { kind: 'text', byte_size: 6 } },
    });
    try { await uploadP; } catch { /* 忽略:本用例只验证切分行为 */ }
  }
});

// 回归:用户点 × 取消上传后,迟到的 attach_file_relay_ack / start_ack 不得让已取消
// 的上传「复活」(继续发下一块、重建卡片)。修复前 × 只 clearUploadState,没 reject
// 在途的 waitForRelayAck / start resolver,导致 ack 一到就继续推进。
test('× 取消上传后迟到的 relay_ack 不让上传复活', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  // 多块文件,chunk_bytes=4 → 10 字节切 3 块,便于观察「下一块」是否被发。
  const file = makeFakeFile({
    name: 'cancel.bin',
    type: 'application/octet-stream',
    bytes: Buffer.from('0123456789'),
  });
  const uploadP = window.requestAttachFile(file);
  await sleep(10);

  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: 'up_cancel', chunk_bytes: 4 },
  });
  await sleep(15);
  const chunksBefore = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
  ).length;
  assert.ok(chunksBefore >= 1, '取消前应已发出第一块');

  // 点 × 取消。
  const xBtn = window.document.querySelector('#attachmentPreview .att-x');
  assert.ok(xBtn, '应有取消按钮');
  xBtn.click();
  // 卡片应被清掉。
  assert.ok(
    !window.document.querySelector('#attachmentPreview .att-x'),
    '取消后卡片应消失',
  );
  sent.length = 0;

  // 现在注入第一块的迟到 relay_ack:修复前会 resolve waitForRelayAck → 循环推进 →
  // 发第二块 + 重建卡片;修复后应被忽略。
  window.handleDesktopEvent({
    type: 'attach_file_relay_ack',
    payload: { upload_id: 'up_cancel', index: 0, ok: true },
  });
  await sleep(30);

  const chunksAfter = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
  );
  assert.equal(
    chunksAfter.length,
    0,
    '取消后迟到的 relay_ack 不应触发发出下一块',
  );
  assert.ok(
    !window.document.querySelector('#attachmentPreview .att-x'),
    '迟到的 ack 不应重建已取消的上传卡片',
  );

  // uploadP 不得 hang:requestAttachFile 外层 catch 会吞掉 user_aborted 错误
  // (fire-and-forget 调用方不期望 reject),所以这里只要确认它在合理时间内 settle
  // 即可(无论 resolve 还是 reject),关键是副作用:不再发块、不重建卡片。
  let settled = false;
  await Promise.race([
    uploadP.finally(() => { settled = true; }),
    new Promise((_, rej) => setTimeout(() => rej(new Error('uploadP hung after abort')), 3000)),
  ]);
  assert.ok(settled, '取消后 uploadP 必须 settle,不得 hang');
});

// 回归:tools_snapshot 里 disabled_ids 含未知 id / 重复 id 时,「已启用 N/总数」
// 不得显示负数。
test('tools_snapshot 的 enabled 计数不会因未知/重复 disabled_ids 变负', (t) => {
  const { window, close } = createPage();
  t.after(close);

  window.openSheet('tools');
  window.handleDesktopEvent({
    type: 'tools_snapshot',
    payload: {
      all: [
        { id: 'tool_a', name: 'A', description: '' },
        { id: 'tool_b', name: 'B', description: '' },
      ],
      disabled_ids: ['tool_a', 'tool_a', 'ghost_id', 'another_ghost'], // 重复 + 2 个未知
    },
  });
  const chipText = window.document.getElementById('toolsChip').textContent;
  // 实际启用 = 2 - {tool_a 唯一} = 1;不得因 4 个 disabled(>总数)算成 -2/2。
  assert.ok(
    !/-\d/.test(chipText),
    `toolsChip 不得出现负数 enabled 计数,实际=${chipText}`,
  );
  assert.match(chipText, /1\/2|0\/2/, `enabled 计数应合理,实际=${chipText}`);
});

// 回归(安全):附件文件名是桌面端经半可信中继透传的字符串,绝不能作为 HTML 渲染。
// 用 <img src=x onerror=...> 当文件名,注入完成后必须:① 不执行脚本(window.__xss
// 未被置位)② 卡片/预览把文件名当纯文本显示(含字面量 <)。这条 jsdom 测试替代了
// real-browser-upload.driver.mjs 里那条 driver 自己也承认「无法构造」的同义反复断言。
test('附件文件名含 <img onerror> 时按文本渲染,不执行脚本', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  window.__xss = false;
  const maliciousName = '<img src=x onerror="window.__xss=1">.txt';
  const file = makeFakeFile({ name: maliciousName, type: 'text/plain', bytes: 'hi' });
  const uploadP = window.requestAttachFile(file);
  await sleep(10);

  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: 'up_xss', chunk_bytes: 768 * 1024 },
  });
  await sleep(15);
  window.handleDesktopEvent({
    type: 'attach_file_relay_ack',
    payload: { upload_id: 'up_xss', index: 0, ok: true },
  });
  window.handleDesktopEvent({
    type: 'attach_file_result',
    payload: { upload_id: 'up_xss', ok: true, ingest_preview: { kind: 'text', byte_size: 2 } },
  });
  await uploadP;

  // ① 脚本未执行。
  assert.equal(window.__xss, false, '文件名里的 onerror 脚本不得执行');
  // ② 预览区没有活的 <img> 节点(文件名应是文本,不是元素)。
  const liveImg = window.document.querySelector('#attachmentPreview img');
  assert.equal(liveImg, null, '不得把恶意文件名渲染成活的 img 节点');
  // ③ 文件名字面量应作为文本可见(证明是 textContent 渲染,不是被吞)。
  const previewText = window.document.querySelector('#attachmentPreview').textContent;
  assert.ok(
    previewText.includes('<img') || previewText.includes('&lt;img'),
    '恶意文件名应作为文本可见(转义或原样),实际=' + previewText,
  );
});

// 回归:attach_file_relay_ack 带 ok:false 且 message 不是 'rate_limited' 时,是硬错误,
// 必须立即终止上传(不重试、进入 error 态),而不是静默继续或无限重试。
test('relay_ack ok:false(非 rate_limited)是硬错误,立即终止上传', async (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  const file = makeFakeFile({ name: 'nak.bin', type: 'application/octet-stream', bytes: Buffer.from('0123456789') });
  const uploadP = window.requestAttachFile(file);
  await sleep(10);

  window.handleDesktopEvent({
    type: 'attach_file_start_ack',
    payload: { upload_id: 'up_nak', chunk_bytes: 4 },
  });
  await sleep(15);
  const chunksBefore = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
  ).length;
  assert.ok(chunksBefore >= 1, '应已发出第一块');

  // 第一块收到非 rate_limited 的 NAK(模拟中继/桌面硬拒绝)。
  sent.length = 0;
  window.handleDesktopEvent({
    type: 'attach_file_relay_ack',
    payload: { upload_id: 'up_nak', index: 0, ok: false, message: 'desktop_rejected' },
  });
  await sleep(60);

  // 不应继续发第二块(硬错误不重试)。允许 0~1 块(可能有一次 attempt 内的检查),
  // 但不应发到 index>=1 的块。
  const laterChunks = sent.filter(
    (m) => m.type === 'mobile_action' && m.payload.type === 'attach_file_chunk',
  );
  const reachedIdx2 = laterChunks.some((m) => m.payload.payload.index >= 1);
  assert.ok(
    !reachedIdx2,
    '硬错误 NAK 后不得推进到第二块,实际发出 index>=1 的块',
  );

  // uploadP 应已 settle(error 态),不 hang。
  let settled = false;
  await Promise.race([
    uploadP.finally(() => { settled = true; }),
    new Promise((_, rej) => setTimeout(() => rej(new Error('uploadP hung after hard NAK')), 3000)),
  ]);
  assert.ok(settled, '硬错误 NAK 后 uploadP 必须 settle');
});
