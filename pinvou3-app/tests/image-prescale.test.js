// image-prescale 单元测试（node --test 直跑，无 node_modules 依赖）。
// 锁定 JPEG 转换前白色铺底：透明 PNG 直接转 JPEG 透明区会变黑。
"use strict";

const test = require("node:test");
const assert = require("node:assert");

const { PinvouImagePrescale } = require("../src/features/attachments/image-prescale.js");

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
