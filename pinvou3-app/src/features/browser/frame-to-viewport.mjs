// screencast 帧像素 → 视口 CSS 像素换算（纯函数，供 BrowserView 与单测复用）。
//
// `<img>` 是 object-contain 等比缩放：getBoundingClientRect 返回整盒，
// 宽高比不一致时上下/左右有 letterbox 黑边。必须按实际绘制区换算，
// 绘制区之外的点击/移动不映射进页面坐标（返回 null）。

/**
 * @param {{ naturalWidth: number, naturalHeight: number, getBoundingClientRect: () => { left: number, top: number, width: number, height: number } }} img
 * @param {number} clientX
 * @param {number} clientY
 * @param {number|undefined|null} pageScaleFactor
 * @returns {{ x: number, y: number } | null} 黑边内或图像未就绪返回 null。
 */
export function frameToViewport(img, clientX, clientY, pageScaleFactor) {
  if (!img || !img.naturalWidth) return null;
  const rect = img.getBoundingClientRect();
  const aspect = img.naturalWidth / img.naturalHeight;
  let drawnW = rect.width;
  let drawnH = drawnW / aspect;
  if (drawnH > rect.height) {
    drawnH = rect.height;
    drawnW = drawnH * aspect;
  }
  const offsetX = rect.left + (rect.width - drawnW) / 2;
  const offsetY = rect.top + (rect.height - drawnH) / 2;
  const px = clientX - offsetX;
  const py = clientY - offsetY;
  if (px < 0 || py < 0 || px > drawnW || py > drawnH) return null; // 黑边内
  let x = (px / drawnW) * img.naturalWidth;
  let y = (py / drawnH) * img.naturalHeight;
  // 页面缩放（pageScaleFactor≠1）时坐标换算到 CSS 像素
  if (pageScaleFactor && pageScaleFactor > 0 && pageScaleFactor !== 1) {
    x /= pageScaleFactor;
    y /= pageScaleFactor;
  }
  return { x, y };
}
