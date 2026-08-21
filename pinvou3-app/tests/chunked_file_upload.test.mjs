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

console.log('chunked file upload tests passed');
