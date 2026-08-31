/**
 * image-prescale.js — 发送前图片预缩放（classic script，全局 PinvouImagePrescale）。
 *
 * 超长边图片先压到 ~1500px、转 JPEG quality 0.9 再入附件：本地引擎侧
 * --image-max-tokens 1024 的视觉编码耗时随 token 数线性增长，预缩放把
 * 4K 截图的识别耗时从分钟级压到秒级，对识图质量影响可忽略。
 * 动图（GIF/WebP）不重编码：canvas 只能画第一帧，动图会被静默压成静态图。
 * 解码加 15s 超时：挂起的 decode 不得卡住整个上传循环。
 * 无 canvas 环境（web 宿主/异常 webview）静默回落原图，绝不拦截添加。
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";

  // 长边上限：对齐 Qwen-VL grounding 建议下限对应的分辨率区间。
  const MAX_EDGE = 1500;
  const JPEG_QUALITY = 0.9;
  // 解码超时：损坏文件或 WebView 解码器卡死时按原图透传，不阻塞上传循环。
  const DECODE_TIMEOUT_MS = 15000;

  function passthrough(file) {
    return { file, compressed: false };
  }

  // JPEG 无透明通道：透明 PNG 直接转 JPEG 后透明区变黑，先白色铺底再绘制。
  function drawImageOnWhite(ctx, img, w, h) {
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, w, h);
    ctx.drawImage(img, 0, 0, w, h);
  }

  /**
   * @param {File|Blob} file 原始图片
   * @returns {Promise<{file: File|Blob, compressed: boolean}>}
   *   compressed=true 时 file 为 JPEG Blob（长边 ≤ MAX_EDGE）。
   *   任何失败/超时/动图一律透传原文件，绝不丢图或卡死。
   */
  function prescaleImageFile(file) {
    return new Promise((resolve) => {
      try {
        if (!file || !file.type || file.type.indexOf("image/") !== 0) { resolve(passthrough(file)); return; }
        // SVG 是矢量图，canvas 光栅化反而损失质量；跳过。
        if (file.type === "image/svg+xml") { resolve(passthrough(file)); return; }
        // 动图透传：GIF 一定按动图对待；WebP 是否动图需解析文件头，
        // 廉价检测不可靠，故对整个 image/webp 透传——代价是静态 WebP
        // 不被压缩，可接受（丢帧压成静态图是不可接受的）。
        if (file.type === "image/gif" || file.type === "image/webp") { resolve(passthrough(file)); return; }
        if (!root.document || typeof root.document.createElement !== "function") { resolve(passthrough(file)); return; }
        const probe = root.document.createElement("canvas");
        if (!probe || typeof probe.getContext !== "function") { resolve(passthrough(file)); return; }
        const url = URL.createObjectURL(file);
        let done = false;
        const finish = (result) => {
          if (done) return;
          done = true;
          clearTimeout(decodeTimer);
          URL.revokeObjectURL(url);
          resolve(result);
        };
        const decodeTimer = setTimeout(() => finish(passthrough(file)), DECODE_TIMEOUT_MS);
        const img = new Image();
        img.onload = () => {
          try {
            const w = img.naturalWidth || 0;
            const h = img.naturalHeight || 0;
            const longEdge = Math.max(w, h);
            if (!w || !h || longEdge <= MAX_EDGE) { finish(passthrough(file)); return; }
            const scale = MAX_EDGE / longEdge;
            const tw = Math.max(1, Math.round(w * scale));
            const th = Math.max(1, Math.round(h * scale));
            const canvas = root.document.createElement("canvas");
            canvas.width = tw;
            canvas.height = th;
            const ctx = canvas.getContext("2d");
            if (!ctx) { finish(passthrough(file)); return; }
            drawImageOnWhite(ctx, img, tw, th);
            canvas.toBlob((blob) => {
              if (!blob) { finish(passthrough(file)); return; }
              finish({ file: blob, compressed: true });
            }, "image/jpeg", JPEG_QUALITY);
          } catch {
            finish(passthrough(file));
          }
        };
        img.onerror = () => { finish(passthrough(file)); };
        img.src = url;
      } catch {
        resolve(passthrough(file));
      }
    });
  }

  root.PinvouImagePrescale = {
    prescaleImageFile,
    drawImageOnWhite,
    MAX_EDGE,
    JPEG_QUALITY,
    DECODE_TIMEOUT_MS,
  };
// Node 单测经 require 加载本文件（无 window），CJS 顶层 this 即 module.exports，
// 作为挂载点兜底；浏览器经典脚本中顶层 this 即 window。
// eslint-disable-next-line unicorn/no-this-outside-of-class -- CJS 测试通道依赖顶层 this(=module.exports)
})(typeof window === "undefined" ? this : window);
