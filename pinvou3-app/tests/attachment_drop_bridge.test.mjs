import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

globalThis.window = {
  __PINVOU_TAURI_BRIDGE_FEATURES__: {},
};
globalThis.btoa = value => Buffer.from(value, 'binary').toString('base64');

const bridgeSource = await readFile(
  new URL('../src/platform/tauri/bridge/artifacts.js', import.meta.url),
  'utf8',
);
assert.match(
  bridgeSource,
  /if \(att\.uploadId && \(att\.cancelled \|\| commitAcknowledged\)\)/,
  'a backend upload error must not cancel a prior completed upload with the same ID',
);
assert.doesNotMatch(
  bridgeSource,
  /PinvouAttachmentDropController\.install/,
  'the platform bridge must not consume drops outside the visible composer',
);
vm.runInThisContext(bridgeSource, { filename: 'artifacts.js' });

const state = { activeSessionId: null, attachments: [] };
const invokedCommands = [];
const feature = window.__PINVOU_TAURI_BRIDGE_FEATURES__.artifacts;
assert.equal(typeof feature, 'function');

const api = feature({
  state,
  notify() {},
  async invoke(command, args) {
    invokedCommands.push({ command, args });
    if (command === 'ingest_draft_file_chunk') {
      return {
        basename: args.filename,
        kind: 'pdf',
        path: `C:\\draft-attachments\\${args.uploadId}\\${args.filename}`,
        markdown: 'test',
        token_estimate: 1,
        byte_size: args.total,
        warning: null,
      };
    }
    if (command === 'adopt_draft_attachment') {
      return {
        basename: 'a.pdf',
        kind: 'pdf',
        path: `C:\\sessions\\${args.sessionId}\\attachments\\a.pdf`,
        markdown: 'test',
        token_estimate: 1,
        byte_size: 3,
        warning: null,
      };
    }
    return {};
  },
  bt: value => value,
  addSystemItem() {},
  dialogOpen: null,
  basename: value => String(value).split(/[\\/]/).pop(),
  isDeliverable: () => false,
  isAbsPath: () => true,
  sessionStates: {},
  async discardManagedAttachment(result) {
    const draftUploadId = result.__pinvouManagedDraftAttachmentId;
    invokedCommands.push(draftUploadId
      ? { command: 'cancel_draft_file_upload', args: { uploadId: draftUploadId } }
      : {
          command: 'discard_dropped_attachment',
          args: {
            sessionId: result.__pinvouManagedAttachmentSessionId,
            path: result.path,
          },
        });
  },
});

function fakeFile(name = 'a.pdf') {
  return {
    name,
    size: 3,
    slice(start, end) {
      return {
        async arrayBuffer() {
          return Uint8Array.from([1, 2, 3].slice(start, end)).buffer;
        },
      };
    },
  };
}

await api.uploadDeviceFiles([fakeFile()]);
assert.equal(state.activeSessionId, null, 'dropping a file must not create a session');
assert.equal(state.attachments.length, 1);
assert.equal(state.attachments[0].status, 'ready');
assert.equal(invokedCommands[0].command, 'ingest_draft_file_chunk');
assert.equal('sessionId' in invokedCommands[0].args, false);
assert.equal(invokedCommands[0].args.commit, true);
assert.equal(invokedCommands[0].args.dataBase64, 'AQID');
assert.equal(
  Object.prototype.propertyIsEnumerable.call(
    state.attachments[0].result,
    '__pinvouManagedDraftAttachmentId',
  ),
  false,
  'draft lifecycle metadata must not cross the Tauri serialization boundary',
);

const firstId = state.attachments[0].id;
api.removeAttachment(firstId);
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal(invokedCommands[1].command, 'cancel_draft_file_upload');

await api.uploadDeviceFiles([fakeFile()]);
const attachment = state.attachments[0];
const uploadId = attachment.result.__pinvouManagedDraftAttachmentId;
await api.adoptManagedAttachments([attachment], 'session_test_123');
assert.deepEqual(invokedCommands.at(-1), {
  command: 'adopt_draft_attachment',
  args: { sessionId: 'session_test_123', uploadId },
});
assert.match(attachment.result.path, /sessions\\session_test_123/);
assert.equal(attachment.result.__pinvouManagedDraftAttachmentId, undefined);
assert.equal(attachment.result.__pinvouManagedAttachmentSessionId, 'session_test_123');

api.removeAttachment(attachment.id);
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal(invokedCommands.at(-1).command, 'discard_dropped_attachment');
assert.equal(invokedCommands.at(-1).args.sessionId, 'session_test_123');

console.log('attachment drop bridge tests passed');
