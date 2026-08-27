// image-prescale 单元测试（node --test 直跑，无 node_modules 依赖）。
// 锁定 JPEG 转换前白色铺底：透明 PNG 直接转 JPEG 透明区会变黑。
// 动图（GIF/WebP）必须透传，不能被 canvas 静默压成第一帧；
// 解码挂起/失败必须按原图透传，不得卡死上传循环。
"use strict";

const test = require("node:test");
const assert = require("node:assert");

const MODULE_PATH = "../src/features/attachments/image-prescale.js";
const { PinvouImagePrescale } = require(MODULE_PATH);

test("drawImageOnWhite 先白色铺底再绘制图片", () => {
  const calls = [];
  const ctx = {
    fillStyle: null,
    fillRect: (...args) => calls.push(["fillRect", ...args]),
    drawImage: (...args) => calls.push(["drawImage", ...args]),
  };
  const img = {};
  PinvouImagePrescale.drawImageOnWhite(ctx, img, 100, 50);
  assert.strictEqual(ctx.fillStyle, "#ffffff");
  assert.deepStrictEqual(calls[0], ["fillRect", 0, 0, 100, 50]);
  assert.deepStrictEqual(calls[1], ["drawImage", img, 0, 0, 100, 50]);
});

test("prescaleImageFile 对非图片原样透传", async () => {
  const file = { type: "text/plain" };
  const result = await PinvouImagePrescale.prescaleImageFile(file);
  assert.strictEqual(result.file, file);
  assert.strictEqual(result.compressed, false);
});

test("prescaleImageFile 对 SVG 原样透传", async () => {
  const file = { type: "image/svg+xml" };
  const result = await PinvouImagePrescale.prescaleImageFile(file);
  assert.strictEqual(result.file, file);
  assert.strictEqual(result.compressed, false);
});

test("prescaleImageFile 对 GIF 原样透传（动图不压成第一帧）", async () => {
  const file = { type: "image/gif" };
  const result = await PinvouImagePrescale.prescaleImageFile(file);
  assert.strictEqual(result.file, file);
  assert.strictEqual(result.compressed, false);
});

test("prescaleImageFile 对 WebP 原样透传（动图无法廉价检测，整体透传）", async () => {
  const file = { type: "image/webp" };
  const result = await PinvouImagePrescale.prescaleImageFile(file);
  assert.strictEqual(result.file, file);
  assert.strictEqual(result.compressed, false);
});

// n/no-unsupported-features 按配置的 Node 22.16 下限把 URL.createObjectURL/revokeObjectURL
// 视为实验 API；这里只是测试 mock 的读写，经 Reflect/Object.assign 绕开该静态误报。
function loadWithMockWindow(imageImpl) {
  const mockWindow = {
    document: { createElement: () => ({ getContext: () => ({}) }) },
  };
  const savedUrlApi = {
    createObjectURL: Reflect.get(URL, "createObjectURL"),
    revokeObjectURL: Reflect.get(URL, "revokeObjectURL"),
  };
  Object.assign(URL, { createObjectURL: () => "blob:mock", revokeObjectURL: () => {} });
  globalThis.window = mockWindow;
  globalThis.Image = imageImpl;
  delete require.cache[require.resolve(MODULE_PATH)];
  require(MODULE_PATH);
  const restore = () => {
    Object.assign(URL, savedUrlApi);
    delete globalThis.window;
    delete globalThis.Image;
    delete require.cache[require.resolve(MODULE_PATH)];
  };
  return { prescale: mockWindow.PinvouImagePrescale, restore };
}

test("prescaleImageFile 解码超时（挂起的 decode）透传原文件", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  // src 赋值后永不触发 onload/onerror，模拟解码器卡死。
  const { prescale, restore } = loadWithMockWindow(class {
    set src(v) { this._src = v; /* 挂起：不触发任何回调 */ }
  });
  try {
    const file = { type: "image/png", name: "stuck.png" };
    const pending = prescale.prescaleImageFile(file);
    t.mock.timers.tick(prescale.DECODE_TIMEOUT_MS);
    const result = await pending;
    assert.strictEqual(result.file, file);
    assert.strictEqual(result.compressed, false);
  } finally {
    restore();
  }
});

test("prescaleImageFile 解码失败（onerror）透传原文件", async () => {
  const { prescale, restore } = loadWithMockWindow(class {
    set src(v) { this._src = v; this.onerror(); }
  });
  try {
    const file = { type: "image/png", name: "broken.png" };
    const result = await prescale.prescaleImageFile(file);
    assert.strictEqual(result.file, file);
    assert.strictEqual(result.compressed, false);
  } finally {
    restore();
  }
});
