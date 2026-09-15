import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

import { formatAttachmentLimitError } from '../src/features/attachments/attachment-limit-errors.js';
import { dictEn } from '../src/shared/i18n/en.js';
import { dictJa } from '../src/shared/i18n/ja.js';
import { dictZh } from '../src/shared/i18n/zh.js';

globalThis.window = {
  __PINVOU_TAURI_BRIDGE_FEATURES__: {},
  btoa: value => Buffer.from(value, 'binary').toString('base64'),
};
globalThis.btoa = value => Buffer.from(value, 'binary').toString('base64');

const uploaderSource = await readFile(
  new URL('../src/shared/chunked-file-upload.js', import.meta.url),
  'utf8',
);
vm.runInThisContext(uploaderSource, { filename: 'chunked-file-upload.js' });

const bridgeSource = await readFile(
  new URL('../src/platform/tauri/bridge/artifacts.js', import.meta.url),
  'utf8',
);
assert.match(bridgeSource, /att\.cancelled \|\| upload\.commitAcknowledged/,
  'cleanup must only cancel an explicitly cancelled or acknowledged upload');
assert.doesNotMatch(bridgeSource, /192 \* 1024/,
  'desktop attachments must use the shared 256 KiB uploader');
assert.doesNotMatch(
  bridgeSource,
  /PinvouAttachmentDropController\.install/,
  'the platform bridge must not consume drops outside the visible composer',
);
vm.runInThisContext(bridgeSource, { filename: 'artifacts.js' });

const state = { activeSessionId: null, attachments: [] };
const invokedCommands = [];
const systemItems = [];
let pasteImageError = null;
const feature = window.__PINVOU_TAURI_BRIDGE_FEATURES__.artifacts;
assert.equal(typeof feature, 'function');

const api = feature({
  state,
  notify() {},
  async invoke(command, args) {
    invokedCommands.push({ command, args });
    if (command === 'save_paste_image' && pasteImageError) {
      throw new Error(pasteImageError);
    }
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
  addSystemItem(item) { systemItems.push(item); },
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
await new Promise(resolve => { setTimeout(resolve, 0); });
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
await new Promise(resolve => { setTimeout(resolve, 0); });
assert.equal(invokedCommands.at(-1).command, 'discard_dropped_attachment');
assert.equal(invokedCommands.at(-1).args.sessionId, 'session_test_123');

const oversized = fakeFile('oversized.zip');
oversized.size = 20 * 1024 * 1024 + 1;
await api.uploadDeviceFiles([oversized]);
assert.equal(state.attachments.at(-1).status, 'error');
assert.equal(
  state.attachments.at(-1).error,
  'attachment_file_too_large',
  'browser preflight failures must use the same stable code as backend rejection',
);

pasteImageError = 'attachment_file_too_large';
for (const dictionary of [dictZh, dictEn, dictJa]) {
  await api.addPasteImage('oversized.png', [1], error => (
    formatAttachmentLimitError(error, dictionary.uiAttachments)
  ));
  assert.equal(
    systemItems.at(-1),
    dictionary.uiAttachments.fileTooLarge,
    'ordinary chat paste failures must use the shared localized limit formatter',
  );
}

console.log('attachment drop bridge tests passed');
