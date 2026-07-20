import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

// 用 jsdom 加载真实的 web/index.html(远控手机端页面),驱动其中真实的
// handleDesktopEvent / showPreview / 下载按钮代码,验证 artifact 分块下载的
// 重组、保存与失败路径。WebSocket / createObjectURL / a.click 用桩替代,
// 其余 DOM 与页面代码全部真实执行。

const html = await readFile(new URL('../web/index.html', import.meta.url), 'utf8');
const CHUNK = 768 * 1024; // 与 desktop 端 DOWNLOAD_CHUNK_BYTES 保持一致

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
  return { window, sent, close: () => window.close() };
}

function systemTexts(window) {
  return [...window.document.querySelectorAll('.system')].map((node) => node.textContent);
}

test('分块下载经真实页面代码重组为字节一致的 Blob 并触发保存', async (t) => {
  const { window, close } = createPage();
  t.after(close);

  const big = Buffer.alloc(2_000_000);
  for (let i = 0; i < big.length; i += 1) big[i] = i % 251;
  const chunks = [];
  for (let offset = 0; offset < big.length; offset += CHUNK) {
    chunks.push(big.subarray(offset, Math.min(offset + CHUNK, big.length)));
  }
  assert.equal(chunks.length, 3);

  window.handleDesktopEvent({
    type: 'artifact_download_start',
    payload: {
      session_id: 'sess1',
      download_id: 'dl_1',
      basename: 'e2e-big.bin',
      mime: 'application/octet-stream',
      byte_size: big.length,
      total_chunks: chunks.length,
    },
  });
  chunks.forEach((chunk, index) => {
    window.handleDesktopEvent({
      type: 'artifact_download_chunk',
      payload: { download_id: 'dl_1', index, data: chunk.toString('base64') },
    });
  });
  window.handleDesktopEvent({
    type: 'artifact_download_end',
    payload: { download_id: 'dl_1', total_chunks: chunks.length },
  });

  assert.equal(window.__downloadName, 'e2e-big.bin', '应触发 a[download] 保存');
  assert.ok(window.__capturedBlob, '应创建 Blob');
  assert.equal(window.__capturedBlob.type, 'application/octet-stream');
  const got = Buffer.from(await window.__capturedBlob.arrayBuffer());
  assert.deepEqual(got, big, '重组后的字节必须与源文件一致');
  assert.ok(
    systemTexts(window).some((text) => text.includes('已下载 e2e-big.bin')),
    '消息流应提示下载完成',
  );
});

test('缺块时提示失败且不保存文件', (t) => {
  const { window, close } = createPage();
  t.after(close);

  window.handleDesktopEvent({
    type: 'artifact_download_start',
    payload: {
      session_id: 'sess1',
      download_id: 'dl_bad',
      basename: 'broken.bin',
      mime: 'application/octet-stream',
      byte_size: CHUNK * 2,
      total_chunks: 2,
    },
  });
  window.handleDesktopEvent({
    type: 'artifact_download_chunk',
    payload: { download_id: 'dl_bad', index: 0, data: Buffer.alloc(CHUNK).toString('base64') },
  });
  window.handleDesktopEvent({
    type: 'artifact_download_end',
    payload: { download_id: 'dl_bad', total_chunks: 2 },
  });

  assert.equal(window.__capturedBlob, undefined, '缺块时不应创建 Blob');
  assert.ok(
    systemTexts(window).some((text) => text.includes('下载 broken.bin 失败')),
    '消息流应提示下载失败',
  );
});

test('预览层下载按钮按 artifact_id 发起 request_artifact_download', (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  window.showPreview({
    session_id: 'sess1',
    artifact_id: 'art-1',
    basename: 'report.bin',
    path_tail: 'reports/report.bin',
    preview: { type: 'text', content: 'hello' },
  });
  sent.length = 0;
  window.document.getElementById('previewDownload').click();

  const action = sent.find((msg) => msg.type === 'mobile_action');
  assert.ok(action, '应发送 mobile_action');
  assert.equal(action.payload.type, 'request_artifact_download');
  assert.deepEqual(action.payload.payload, { artifact_id: 'art-1' });
});

test('预览层下载按钮在无 artifact_id 时回退 artifact_path', (t) => {
  const { window, sent, close } = createPage();
  t.after(close);

  window.showPreview({
    session_id: 'sess1',
    artifact_id: null,
    basename: 'hello.txt',
    path_tail: 'notes/hello.txt',
    preview: { type: 'text', content: '你好' },
  });
  sent.length = 0;
  window.document.getElementById('previewDownload').click();

  const action = sent.find((msg) => msg.type === 'mobile_action');
  assert.ok(action, '应发送 mobile_action');
  assert.equal(action.payload.type, 'request_artifact_download');
  assert.deepEqual(action.payload.payload, { artifact_path: 'notes/hello.txt' });
});

test('下载进行中收到 error 事件会中断并提示', (t) => {
  const { window, close } = createPage();
  t.after(close);

  window.handleDesktopEvent({
    type: 'artifact_download_start',
    payload: {
      session_id: 'sess1',
      download_id: 'dl_abort',
      basename: 'half.bin',
      mime: 'application/octet-stream',
      byte_size: CHUNK * 2,
      total_chunks: 2,
    },
  });
  window.handleDesktopEvent({ type: 'error', payload: { message: 'mobile_action_failed' } });

  assert.equal(window.__capturedBlob, undefined);
  assert.ok(
    systemTexts(window).some((text) => text.includes('下载 half.bin 中断')),
    '消息流应提示下载中断',
  );
});
