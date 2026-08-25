(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: classic script 直拷产物,严格模式是载荷
  "use strict";

  const CHUNK_BYTES = 256 * 1024;
  const MAX_FILE_BYTES = 20 * 1024 * 1024;

  function uploadId(prefix) {
    if (root.crypto && typeof root.crypto.randomUUID === "function") {
      return prefix + "_" + root.crypto.randomUUID(); // safari14-ok: guarded above
    }
    // eslint-disable-next-line sonarjs/pseudo-random -- 非安全用途:上传去重 ID,时间戳前缀已保证基本唯一
    return prefix + "_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2, 12);
  }

  function cancelledError() {
    const error = new Error("device-upload-cancelled");
    error.code = "device_upload_cancelled";
    return error;
  }

  // 鸭子类型入参可能不是真 File(测试桩/宿主注入),防御负值与非法 size,
  // 避免负 size 跳过分块循环直达成功路径。
  function isValidUploadSize(size) {
    return typeof size === "number" && Number.isSafeInteger(size) && size >= 0;
  }

  function bytesToBase64(bytes) {
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 0x8000) {
      // 分块只含 0-255 字节值,fromCharCode/fromCodePoint 等价;保留 apply 分块热路径。
      binary += String.fromCharCode.apply(null, bytes.subarray(offset, offset + 0x8000)); // eslint-disable-line unicorn/prefer-code-point
    }
    return root.btoa(binary);
  }

  async function uploadFile(options) {
    const file = options && options.file;
    if (!file || !isValidUploadSize(file.size)) {
      const invalid = new Error("invalid device attachment");
      invalid.code = "device_upload_invalid";
      throw invalid;
    }
    if (file.size === 0) {
      const empty = new Error("empty files cannot be added");
      empty.code = "device_upload_empty";
      throw empty;
    }
    if (file.size > MAX_FILE_BYTES) {
      const tooLarge = new Error("device attachment exceeds 20 MB");
      tooLarge.code = "device_upload_too_large";
      throw tooLarge;
    }
    if (!options || typeof options.sendChunk !== "function") {
      throw new TypeError("sendChunk is required");
    }

    const id = options.uploadId || uploadId(options.uploadPrefix || "attachment");
    let offset = 0;
    let result = null;
    let commitAcknowledged = false;
    function assertActive() {
      if (typeof options.isCancelled === "function" && options.isCancelled()) {
        throw cancelledError();
      }
    }

    try {
      while (offset < file.size) {
        const end = Math.min(offset + CHUNK_BYTES, file.size);
        const bytes = new Uint8Array(await file.slice(offset, end).arrayBuffer());
        assertActive();
        result = await options.sendChunk({
          uploadId: id,
          fileName: file.name || "attachment",
          offset,
          total: file.size,
          dataBase64: bytesToBase64(bytes),
          commit: end === file.size,
        });
        commitAcknowledged = end === file.size;
        offset = end;
        if (typeof options.onProgress === "function") {
          options.onProgress(Math.min(99, Math.round((offset / file.size) * 100)));
        }
      }
      assertActive();
      if (typeof options.validateResult === "function" && !options.validateResult(result)) {
        const invalidResult = new Error("upload did not return a valid attachment");
        invalidResult.code = "device_upload_invalid_result";
        throw invalidResult;
      }
      if (typeof options.onProgress === "function") options.onProgress(100);
      return { result, uploadId: id };
    } catch (error) {
      if (typeof options.cleanup === "function") {
        try {
          await options.cleanup({
            error,
            uploadId: id,
            result,
            commitAcknowledged,
          });
        } catch { /* 清理失败不得掩盖原始上传错误 */ }
      }
      throw error;
    }
  }

  root.PinvouChunkedFileUpload = Object.freeze({
    CHUNK_BYTES,
    MAX_FILE_BYTES,
    bytesToBase64,
    cancelledError,
    uploadFile,
    uploadId,
  });
})(typeof window === "undefined" ? globalThis : window);
