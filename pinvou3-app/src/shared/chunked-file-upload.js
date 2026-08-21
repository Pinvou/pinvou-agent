(function (root) {
  "use strict";

  var CHUNK_BYTES = 256 * 1024;
  var MAX_FILE_BYTES = 20 * 1024 * 1024;

  function uploadId(prefix) {
    if (root.crypto && typeof root.crypto.randomUUID === "function") {
      return prefix + "_" + root.crypto.randomUUID();
    }
    return prefix + "_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2, 12);
  }

  function cancelledError() {
    var error = new Error("device-upload-cancelled");
    error.code = "device_upload_cancelled";
    return error;
  }

  function bytesToBase64(bytes) {
    var binary = "";
    for (var offset = 0; offset < bytes.length; offset += 0x8000) {
      binary += String.fromCharCode.apply(null, bytes.subarray(offset, offset + 0x8000));
    }
    return root.btoa(binary);
  }

  async function uploadFile(options) {
    var file = options && options.file;
    if (!file || !Number.isSafeInteger(file.size) || file.size < 0) {
      var invalid = new Error("invalid device attachment");
      invalid.code = "device_upload_invalid";
      throw invalid;
    }
    if (file.size === 0) {
      var empty = new Error("empty files cannot be added");
      empty.code = "device_upload_empty";
      throw empty;
    }
    if (file.size > MAX_FILE_BYTES) {
      var tooLarge = new Error("device attachment exceeds 20 MB");
      tooLarge.code = "device_upload_too_large";
      throw tooLarge;
    }
    if (!options || typeof options.sendChunk !== "function") {
      throw new TypeError("sendChunk is required");
    }

    var id = options.uploadId || uploadId(options.uploadPrefix || "attachment");
    var offset = 0;
    var result = null;
    var commitAcknowledged = false;
    function assertActive() {
      if (typeof options.isCancelled === "function" && options.isCancelled()) {
        throw cancelledError();
      }
    }

    try {
      while (offset < file.size) {
        var end = Math.min(offset + CHUNK_BYTES, file.size);
        var bytes = new Uint8Array(await file.slice(offset, end).arrayBuffer());
        assertActive();
        result = await options.sendChunk({
          uploadId: id,
          fileName: file.name || "attachment",
          offset: offset,
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
        var invalidResult = new Error("upload did not return a valid attachment");
        invalidResult.code = "device_upload_invalid_result";
        throw invalidResult;
      }
      if (typeof options.onProgress === "function") options.onProgress(100);
      return { result: result, uploadId: id };
    } catch (error) {
      if (typeof options.cleanup === "function") {
        try {
          await options.cleanup({
            error: error,
            uploadId: id,
            result: result,
            commitAcknowledged: commitAcknowledged,
          });
        } catch (_) {}
      }
      throw error;
    }
  }

  root.PinvouChunkedFileUpload = Object.freeze({
    CHUNK_BYTES: CHUNK_BYTES,
    MAX_FILE_BYTES: MAX_FILE_BYTES,
    bytesToBase64: bytesToBase64,
    cancelledError: cancelledError,
    uploadFile: uploadFile,
    uploadId: uploadId,
  });
})(typeof window !== "undefined" ? window : globalThis);
