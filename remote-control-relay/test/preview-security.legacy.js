// 历史 v1 页面的预览安全回归(白名单 raster data URL + object URL 回收)。
// 目标代码是 web/index.html(自标 LEGACY,server.js 默认只服务 v2 构建产物),
// v2 WebUI 的对应场景尚未覆盖,故保留为显式 legacy 入口(npm run test:legacy-v1-ui),
// 不进默认 `npm test` 发现范围。v1 页面退役时应连同本文件一起删除。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { JSDOM } from "jsdom";

const html = await readFile(new URL("../web/index.html", import.meta.url), "utf8");
const PNG_DATA_URL = "data:image/png;base64,iVBORw0KGgo=";

function createPage() {
  const created = [];
  const revoked = [];
  const dom = new JSDOM(html, {
    url: "https://relay.test/pinvou3/remote/r/preview#token=tok",
    runScripts: "dangerously",
    pretendToBeVisual: true,
    beforeParse(window) {
      class FakeWebSocket {
        static OPEN = 1;
        constructor() {
          this.readyState = FakeWebSocket.OPEN;
        }
        send() {}
        close() {}
      }
      window.WebSocket = FakeWebSocket;
      window.URL.createObjectURL = (blob) => {
        const url = `blob:preview-${created.length + 1}`;
        created.push({ url, type: blob.type, size: blob.size });
        return url;
      };
      window.URL.revokeObjectURL = (url) => revoked.push(url);
    },
  });
  return {
    window: dom.window,
    created,
    revoked,
    close: () => dom.window.close(),
  };
}

test("图片预览只接受白名单 raster data URL", (t) => {
  const page = createPage();
  t.after(page.close);

  page.window.showPreview({
    basename: "preview.png",
    preview: { type: "image", data_url: PNG_DATA_URL },
  });

  const image = page.window.document.querySelector("#previewBody img");
  assert.equal(image?.getAttribute("src"), "blob:preview-1");
  assert.deepEqual(page.created, [
    { url: "blob:preview-1", type: "image/png", size: 8 },
  ]);

  const rejected = [
    "data:image/svg+xml;base64,PHN2Zy8+",
    "javascript:alert(1)",
    "data:image/png;base64,not-valid===",
    "data:image/png.html;base64,iVBORw0KGgo=",
  ];
  for (const dataUrl of rejected) {
    page.window.showPreview({
      basename: "invalid",
      preview: { type: "image", data_url: dataUrl },
    });
    assert.equal(page.window.document.querySelector("#previewBody img"), null);
    assert.match(
      page.window.document.querySelector("#previewBody").textContent,
      /图片预览数据无效/,
    );
  }
  assert.equal(page.created.length, 1);
});

test("切换和关闭预览会回收 object URL", (t) => {
  const page = createPage();
  t.after(page.close);
  const showImage = (basename) => page.window.showPreview({
    basename,
    preview: { type: "image", data_url: PNG_DATA_URL },
  });

  showImage("first.png");
  showImage("second.png");
  assert.deepEqual(page.revoked, ["blob:preview-1"]);

  page.window.document.querySelector("#previewClose").click();
  assert.deepEqual(page.revoked, ["blob:preview-1", "blob:preview-2"]);
  assert.equal(page.window.document.querySelector("#previewBody").children.length, 0);

  showImage("overlay.png");
  page.window.document.querySelector("#previewOverlay").click();
  assert.deepEqual(page.revoked, [
    "blob:preview-1",
    "blob:preview-2",
    "blob:preview-3",
  ]);
});

test("接管和终止远程会话会清理图片预览", (t) => {
  const page = createPage();
  t.after(page.close);
  const showImage = (basename) => page.window.showPreview({
    basename,
    preview: { type: "image", data_url: PNG_DATA_URL },
  });

  showImage("takeover.png");
  page.window.showTakeoverScreen();
  assert.deepEqual(page.revoked, ["blob:preview-1"]);
  assert.equal(page.window.document.querySelector("#previewBody").children.length, 0);

  showImage("terminal.png");
  page.window.enterTerminalState("已结束", "会话结束", "连接已关闭", "", "ended");
  assert.deepEqual(page.revoked, ["blob:preview-1", "blob:preview-2"]);
  assert.equal(page.window.document.querySelector("#previewBody").children.length, 0);
});
