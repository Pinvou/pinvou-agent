#!/usr/bin/env node
// frameToViewport（screencast 帧像素 → 视口 CSS 像素）纯函数单测：
// object-contain letterbox 黑边剔除 + pageScaleFactor 除法。
import assert from 'node:assert/strict';
import { frameToViewport } from '../src/features/browser/frame-to-viewport.mjs';

// 轻量 img 桩：只需 naturalWidth/naturalHeight + getBoundingClientRect。
const makeImg = (naturalWidth, naturalHeight, rect) => ({
  naturalWidth,
  naturalHeight,
  getBoundingClientRect: () => rect,
});

// 1) 无黑边 1:1：图像与容器同宽高比、同尺寸，坐标恒等映射。
{
  const img = makeImg(1280, 800, { left: 0, top: 0, width: 1280, height: 800 });
  assert.deepEqual(frameToViewport(img, 640, 400, 1), { x: 640, y: 400 });
  assert.deepEqual(frameToViewport(img, 0, 0, 1), { x: 0, y: 0 });
}

// 2) 左右黑边（容器比帧更宽）：绘制区收窄居中，左右各留 100px 黑边。
//    natural 800x600（4:3）放进 1000x600 容器 → 绘制区 800x600，offsetX=100。
{
  const img = makeImg(800, 600, { left: 0, top: 0, width: 1000, height: 600 });
  // 绘制区中心 (500,300) → 帧中心 (400,300)
  assert.deepEqual(frameToViewport(img, 500, 300, 1), { x: 400, y: 300 });
  // 绘制区左缘 (100,0) → (0,0)
  assert.deepEqual(frameToViewport(img, 100, 0, 1), { x: 0, y: 0 });
}

// 3) 上下黑边（帧比容器更宽）：绘制区压扁居中，上下各留 200px 黑边。
//    natural 1280x800 放进 640x800 容器 → 绘制区 640x400，offsetY=200。
{
  const img = makeImg(1280, 800, { left: 0, top: 0, width: 640, height: 800 });
  // 绘制区中心 (320,400) → 帧中心 (640,400)
  assert.deepEqual(frameToViewport(img, 320, 400, 1), { x: 640, y: 400 });
}

// 4) 黑边内点击拒绝映射：绘制区之外返回 null，不产生页面坐标。
{
  const img = makeImg(800, 600, { left: 0, top: 0, width: 1000, height: 600 });
  assert.equal(frameToViewport(img, 50, 300, 1), null, '左黑边内应返回 null');
  assert.equal(frameToViewport(img, 950, 300, 1), null, '右黑边内应返回 null');
  const tall = makeImg(1280, 800, { left: 0, top: 0, width: 640, height: 800 });
  assert.equal(frameToViewport(tall, 320, 100, 1), null, '上黑边内应返回 null');
  assert.equal(frameToViewport(tall, 320, 700, 1), null, '下黑边内应返回 null');
  // 边界：恰好落在绘制区边缘（px === drawnW）仍映射，不视为黑边。
  assert.deepEqual(frameToViewport(img, 900, 600, 1), { x: 800, y: 600 });
}

// 5) pageScaleFactor≠1：帧物理像素除以缩放系数得到页面 CSS 像素。
{
  const img = makeImg(1280, 800, { left: 0, top: 0, width: 1280, height: 800 });
  assert.deepEqual(frameToViewport(img, 640, 400, 2), { x: 320, y: 200 });
  assert.deepEqual(frameToViewport(img, 640, 400, 0.5), { x: 1280, y: 800 });
}

// 6) 图像未就绪（无 naturalWidth / 无元素）：安全返回 null。
assert.equal(frameToViewport(null, 10, 10, 1), null);
assert.equal(frameToViewport(makeImg(0, 0, { left: 0, top: 0, width: 100, height: 100 }), 10, 10, 1), null);

console.log('browser frame-to-viewport tests passed');
