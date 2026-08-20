/**
 * image-prescale.js — 发送前图片预缩放（classic script，全局 PinvouImagePrescale）。
 *
 * 超长边图片先压到 ~1500px、转 JPEG quality 0.9 再入附件：本地引擎侧
 * --image-max-tokens 1024 的视觉编码耗时随 token 数线性增长，预缩放把
 * 4K 截图的识别耗时从分钟级压到秒级，对识图质量影响可忽略。
 * 无 canvas 环境（web 宿主/异常 webview）静默回落原图，绝不拦截添加。
 */
(function (root) {
  "use strict";

  // 长边上限：对齐 Qwen-VL grounding 建议下限对应的分辨率区间。
  var MAX_EDGE = 1500;
  var JPEG_QUALITY = 0.9;

  function passthrough(file) {
    return { file: file, compressed: false };
  }

  /**
   * @param {File|Blob} file 原始图片
   * @returns {Promise<{file: File|Blob, compressed: boolean}>}
   *   compressed=true 时 file 为 JPEG Blob（长边 ≤ MAX_EDGE）。
   */
  function prescaleImageFile(file) {
    return new Promise(function (resolve) {
      try {
        if (!file || !file.type || file.type.indexOf("image/") !== 0) return resolve(passthrough(file));
        // SVG 是矢量图，canvas 光栅化反而损失质量；跳过。
        if (file.type === "image/svg+xml") return resolve(passthrough(file));
        if (!root.document || typeof root.document.createElement !== "function") return resolve(passthrough(file));
        var probe = root.document.createElement("canvas");
        if (!probe || typeof probe.getContext !== "function") return resolve(passthrough(file));
        var url = URL.createObjectURL(file);
        var done = false;
        function finish(result) {
          if (done) return;
          done = true;
          URL.revokeObjectURL(url);
          resolve(result);
        }
        var img = new Image();
        img.onload = function () {
          try {
            var w = img.naturalWidth || 0;
            var h = img.naturalHeight || 0;
            var longEdge = Math.max(w, h);
            if (!w || !h || longEdge <= MAX_EDGE) return finish(passthrough(file));
            var scale = MAX_EDGE / longEdge;
            var tw = Math.max(1, Math.round(w * scale));
            var th = Math.max(1, Math.round(h * scale));
            var canvas = root.document.createElement("canvas");
            canvas.width = tw;
            canvas.height = th;
            var ctx = canvas.getContext("2d");
            if (!ctx) return finish(passthrough(file));
            ctx.drawImage(img, 0, 0, tw, th);
            canvas.toBlob(function (blob) {
              if (!blob) return finish(passthrough(file));
              finish({ file: blob, compressed: true });
            }, "image/jpeg", JPEG_QUALITY);
          } catch (_) {
            finish(passthrough(file));
          }
        };
        img.onerror = function () { finish(passthrough(file)); };
        img.src = url;
      } catch (_) {
        resolve(passthrough(file));
      }
    });
  }

  root.PinvouImagePrescale = {
    prescaleImageFile: prescaleImageFile,
    MAX_EDGE: MAX_EDGE,
    JPEG_QUALITY: JPEG_QUALITY,
  };
})(typeof window !== "undefined" ? window : this);
