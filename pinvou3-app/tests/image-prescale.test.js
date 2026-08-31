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

// 可缩放 mock：canvas 记录 width/height/2d 绘制序列/toBlob 实参，Image 在
// src 赋值时携带 naturalWidth/naturalHeight 触发 onload（与真实解码同步
// 完成的时序一致）。toBlobImpl 返回值即 toBlob 回调收到的 blob。
function loadWithScalingWindow({ width, height, blob = {} }) {
  const calls = { contexts: [], toBlob: [], canvases: [] };
  const mockWindow = {
    document: {
      createElement: () => {
        const canvas = {
          width: 0,
          height: 0,
          getContext: (kind) => {
            const ctx = {
              kind,
              ops: [],
              fillStyle: null,
              fillRect: (...args) => ctx.ops.push(["fillRect", ...args]),
              drawImage: (...args) => ctx.ops.push(["drawImage", ...args]),
            };
            calls.contexts.push(ctx);
            return ctx;
          },
          toBlob: (callback, ...args) => {
            calls.toBlob.push(args);
            callback(blob);
          },
        };
        calls.canvases.push(canvas);
        return canvas;
      },
    },
  };
  const savedUrlApi = {
    createObjectURL: Reflect.get(URL, "createObjectURL"),
    revokeObjectURL: Reflect.get(URL, "revokeObjectURL"),
  };
  Object.assign(URL, { createObjectURL: () => "blob:mock", revokeObjectURL: () => {} });
  globalThis.window = mockWindow;
  globalThis.Image = class {
    constructor() {
      this.naturalWidth = width;
      this.naturalHeight = height;
    }
    set src(v) { this._src = v; this.onload(); }
  };
  delete require.cache[require.resolve(MODULE_PATH)];
  require(MODULE_PATH);
  const restore = () => {
    Object.assign(URL, savedUrlApi);
    delete globalThis.window;
    delete globalThis.Image;
    delete require.cache[require.resolve(MODULE_PATH)];
  };
  return { prescale: mockWindow.PinvouImagePrescale, calls, restore };
}

test("prescaleImageFile 超长边图片缩放到长边 ≤ MAX_EDGE 并转 JPEG", async () => {
  const blob = { type: "image/jpeg" };
  const { prescale, calls, restore } = loadWithScalingWindow({ width: 4000, height: 2000, blob });
  try {
    const file = { type: "image/png", name: "huge.png" };
    const result = await prescale.prescaleImageFile(file);
    // 输出：长边压到 1500（4000×2000 → 1500×750），compressed 标记为真。
    assert.strictEqual(result.compressed, true);
    assert.strictEqual(result.file, blob);
    const workCanvas = calls.canvases[calls.canvases.length - 1];
    assert.strictEqual(workCanvas.width, 1500);
    assert.strictEqual(workCanvas.height, 750);
    assert.deepStrictEqual(calls.toBlob, [["image/jpeg", prescale.JPEG_QUALITY]]);
    // 白色铺底必须先于绘制：透明 PNG 转 JPEG 透明区变黑。
    const ctx = calls.contexts[calls.contexts.length - 1];
    assert.strictEqual(ctx.fillStyle, "#ffffff");
    assert.strictEqual(ctx.ops[0][0], "fillRect");
    assert.strictEqual(ctx.ops[1][0], "drawImage");
  } finally {
    restore();
  }
});

test("prescaleImageFile 竖长图按长边缩放（高为长边）", async () => {
  const blob = { type: "image/jpeg" };
  const { prescale, calls, restore } = loadWithScalingWindow({ width: 900, height: 3000, blob });
  try {
    const result = await prescale.prescaleImageFile({ type: "image/png" });
    assert.strictEqual(result.compressed, true);
    const workCanvas = calls.canvases[calls.canvases.length - 1];
    assert.strictEqual(workCanvas.width, 450);
    assert.strictEqual(workCanvas.height, 1500);
  } finally {
    restore();
  }
});

test("prescaleImageFile 极端宽高比短边缩放结果钳制到 ≥1px", async () => {
  const blob = { type: "image/jpeg" };
  const { prescale, calls, restore } = loadWithScalingWindow({ width: 4000, height: 2, blob });
  try {
    const result = await prescale.prescaleImageFile({ type: "image/png" });
    assert.strictEqual(result.compressed, true);
    const workCanvas = calls.canvases[calls.canvases.length - 1];
    assert.strictEqual(workCanvas.width, 1500);
    assert.strictEqual(workCanvas.height, 1); // Math.max(1, round(2*0.375)) = 1
  } finally {
    restore();
  }
});

test("prescaleImageFile 小图（长边 ≤ MAX_EDGE）原样透传，不重编码", async () => {
  const { prescale, calls, restore } = loadWithScalingWindow({ width: 800, height: 600 });
  try {
    const file = { type: "image/png", name: "small.png" };
    const result = await prescale.prescaleImageFile(file);
    assert.strictEqual(result.file, file);
    assert.strictEqual(result.compressed, false);
    // 未请求 2d 上下文、未触发 toBlob（只有探针 canvas 被创建）。
    assert.strictEqual(calls.contexts.length, 0);
    assert.strictEqual(calls.toBlob.length, 0);
  } finally {
    restore();
  }
});

test("prescaleImageFile toBlob 返回 null 时透传原文件", async () => {
  const { prescale, calls, restore } = loadWithScalingWindow({ width: 4000, height: 2000, blob: null });
  try {
    const file = { type: "image/png", name: "blobfail.png" };
    const result = await prescale.prescaleImageFile(file);
    assert.strictEqual(result.file, file);
    assert.strictEqual(result.compressed, false);
    assert.strictEqual(calls.toBlob.length, 1);
  } finally {
    restore();
  }
});

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
