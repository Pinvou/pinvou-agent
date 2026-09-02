(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";

  const CHUNK_BYTES = 256 * 1024;
  const MAX_FILE_BYTES = 20 * 1024 * 1024;

  function uploadId(prefix) {
    if (root.crypto && typeof root.crypto.randomUUID === "function") {
      return prefix + "_" + root.crypto.randomUUID(); // safari14-ok: guarded above
    }
    // eslint-disable-next-line sonarjs/pseudo-random -- not security-sensitive: upload dedupe ID; the timestamp prefix already ensures basic uniqueness
    return prefix + "_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2, 12);
  }

  function cancelledError() {
    const error = new Error("device-upload-cancelled");
    error.code = "device_upload_cancelled";
    return error;
  }

  // The duck-typed argument may not be a real File (test stub/host-injected); guard against negative
  // or invalid size so a negative size cannot skip the chunk loop straight into the success path.
  function isValidUploadSize(size) {
    return typeof size === "number" && Number.isSafeInteger(size) && size >= 0;
  }

  function toHex(bytes) {
    let hex = "";
    for (let i = 0; i < bytes.length; i += 1) {
      hex += (bytes[i] + 0x100).toString(16).slice(1);
    }
    return hex;
  }

  // crypto.subtle is undefined on insecure origins (e.g. plain-HTTP remote
  // access) — exactly where transfer corruption is most likely. The unchecked
  // downgrade stays safe, but it must not be invisible.
  function warnUncheckedUpload(reason) {
    if (root.console && typeof root.console.warn === "function") {
      root.console.warn("device upload integrity unavailable (" + reason + "); transfer proceeds unchecked");
    }
  }

  // Web Crypto has no incremental digest, so the whole file (bounded by
  // MAX_FILE_BYTES) is hashed once before the first chunk is sent. The hash
  // rides the final chunk and the desktop re-verifies the assembled bytes.
  async function fileSha256Hex(file) {
    const subtle = root.crypto && root.crypto.subtle;
    if (!subtle || typeof subtle.digest !== "function" || typeof file.arrayBuffer !== "function") {
      warnUncheckedUpload("web crypto unavailable in this context");
      return null;
    }
    try {
      const buffer = await file.arrayBuffer();
      const digest = await subtle.digest("SHA-256", buffer);
      return toHex(new Uint8Array(digest));
    } catch (error) {
      // A hash we cannot compute locally degrades to an unchecked transfer;
      // the desktop side still verifies whatever digest does arrive.
      warnUncheckedUpload("digest failed: " + (error && error.message ? error.message : error));
      return null;
    }
  }

  function bytesToBase64(bytes) {
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 0x8000) {
      // Chunks only contain byte values 0-255, so fromCharCode/fromCodePoint are equivalent; keep the apply chunked hot path.
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
    const integrity = options.integrity !== false;
    let sha256Hex = null;
    function assertActive() {
      if (typeof options.isCancelled === "function" && options.isCancelled()) {
        throw cancelledError();
      }
    }

    try {
      if (integrity) {
        sha256Hex = await fileSha256Hex(file);
      }
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
          ...(end === file.size && sha256Hex ? { sha256: sha256Hex } : {}),
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
        } catch { /* cleanup failure must not mask the original upload error */ }
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
