import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const source = await readFile(
  new URL('../src/shared/chunked-file-upload.js', import.meta.url),
  'utf8',
);
const windowObject = {
  btoa(value) { return Buffer.from(value, 'binary').toString('base64'); },
};
vm.runInNewContext(source, { window: windowObject }, { filename: 'chunked-file-upload.js' });
const uploader = windowObject.PinvouChunkedFileUpload;

function fileOfSize(size, name = 'notes.txt') {
  const bytes = Uint8Array.from({ length: size }, (_, index) => index % 251);
  return {
    name,
    size,
    slice(start, end) {
      const chunk = bytes.slice(start, end);
      return { async arrayBuffer() { return chunk.buffer; } };
    },
  };
}

{
  const chunks = [];
  const progress = [];
  const file = fileOfSize(uploader.CHUNK_BYTES + 7);
  const completed = await uploader.uploadFile({
    file,
    uploadId: 'upload_two_chunks',
    async sendChunk(chunk) {
      chunks.push(chunk);
      return chunk.commit ? { handle: 'attachment_ok' } : null;
    },
    validateResult: result => Boolean(result?.handle),
    onProgress: value => progress.push(value),
  });
  assert.equal(completed.result.handle, 'attachment_ok');
  assert.deepEqual(chunks.map(chunk => chunk.offset), [0, uploader.CHUNK_BYTES]);
  assert.deepEqual(chunks.map(chunk => chunk.commit), [false, true]);
  assert.ok(chunks.every(chunk => Buffer.from(chunk.dataBase64, 'base64').length
    <= uploader.CHUNK_BYTES));
  assert.equal(progress.at(-1), 100);
}

{
  let cancelled = false;
  let cleanupState = null;
  await assert.rejects(
    uploader.uploadFile({
      file: fileOfSize(uploader.CHUNK_BYTES + 1, 'cancel.txt'),
      uploadId: 'upload_cancelled',
      isCancelled: () => cancelled,
      async sendChunk() { cancelled = true; return null; },
      async cleanup(state) { cleanupState = state; },
    }),
    error => error.code === 'device_upload_cancelled',
  );
  assert.equal(cleanupState.uploadId, 'upload_cancelled');
  assert.equal(cleanupState.commitAcknowledged, false);
}

{
  let cleanupState = null;
  await assert.rejects(
    uploader.uploadFile({
      file: fileOfSize(3, 'invalid.txt'),
      uploadId: 'upload_invalid_result',
      async sendChunk() { return {}; },
      validateResult: () => false,
      async cleanup(state) { cleanupState = state; },
    }),
    error => error.code === 'device_upload_invalid_result',
  );
  assert.equal(cleanupState.commitAcknowledged, true);
}

await assert.rejects(
  uploader.uploadFile({ file: fileOfSize(0), async sendChunk() {} }),
  error => error.code === 'device_upload_empty',
);
await assert.rejects(
  uploader.uploadFile({
    file: { name: 'huge.bin', size: uploader.MAX_FILE_BYTES + 1 },
    async sendChunk() {},
  }),
  error => error.code === 'device_upload_too_large',
);

// SHA-256 integrity: when Web Crypto is available, the whole-file digest rides
// the committing chunk; a transport without crypto (or integrity: false)
// sends no digest instead of failing.
{
  const digestBytes = Uint8Array.from({ length: 32 }, (_, index) => index);
  const digestHex = Array.from(digestBytes, byte => (byte + 0x100).toString(16).slice(1)).join('');
  const cryptoWindow = {
    btoa: windowObject.btoa,
    crypto: { subtle: { digest: async () => digestBytes } },
  };
  vm.runInNewContext(source, { window: cryptoWindow }, { filename: 'chunked-file-upload.js' });
  const cryptoUploader = cryptoWindow.PinvouChunkedFileUpload;
  const file = fileOfSize(10, 'hashed.txt');
  file.arrayBuffer = async () => bytesOf(10).buffer;
  function bytesOf(size) {
    return Uint8Array.from({ length: size }, (_, index) => index % 251);
  }
  const seen = [];
  await cryptoUploader.uploadFile({
    file,
    uploadId: 'upload_hashed',
    async sendChunk(chunk) { seen.push(chunk); return chunk.commit ? { handle: 'ok' } : null; },
    validateResult: () => true,
  });
  assert.equal(seen.at(-1).sha256, digestHex, 'commit chunk must carry the whole-file digest');
  assert.ok(seen.slice(0, -1).every(chunk => chunk.sha256 === undefined));

  // integrity: false skips hashing entirely.
  const plain = [];
  const plainFile = fileOfSize(10, 'plain.txt');
  plainFile.arrayBuffer = async () => { throw new Error('must not hash'); };
  await cryptoUploader.uploadFile({
    file: plainFile,
    uploadId: 'upload_plain',
    integrity: false,
    async sendChunk(chunk) { plain.push(chunk); return chunk.commit ? { handle: 'ok' } : null; },
    validateResult: () => true,
  });
  assert.ok(plain.every(chunk => chunk.sha256 === undefined));

  // A File without arrayBuffer degrades to an unchecked transfer, not a crash.
  const degraded = [];
  await cryptoUploader.uploadFile({
    file: fileOfSize(10, 'degraded.txt'),
    uploadId: 'upload_degraded',
    async sendChunk(chunk) { degraded.push(chunk); return chunk.commit ? { handle: 'ok' } : null; },
    validateResult: () => true,
  });
  assert.ok(degraded.every(chunk => chunk.sha256 === undefined));
}

console.log('chunked file upload tests passed');
