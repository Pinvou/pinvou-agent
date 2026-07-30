import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const documentListeners = new Map();
globalThis.window = {
  __PINVOU_TAURI_BRIDGE_FEATURES__: {},
};
globalThis.document = {
  addEventListener(type, listener) {
    documentListeners.set(type, listener);
  },
  removeEventListener(type) {
    documentListeners.delete(type);
  },
};
globalThis.btoa = value => Buffer.from(value, 'binary').toString('base64');

const controllerSource = await readFile(
  new URL('../src/features/attachments/attachment-drop-controller.js', import.meta.url),
  'utf8',
);
const bridgeSource = await readFile(
  new URL('../src/platform/tauri/bridge/artifacts.js', import.meta.url),
  'utf8',
);
assert.match(
  bridgeSource,
  /if \(att\.uploadId && \(att\.cancelled \|\| commitAcknowledged\)\)/,
  'a backend upload error must not cancel and delete a prior completed upload with the same ID',
);
vm.runInThisContext(controllerSource, { filename: 'attachment-drop-controller.js' });
vm.runInThisContext(bridgeSource, { filename: 'artifacts.js' });

const state = { activeSessionId: 'session_test_123', attachments: [], attachmentDragActive: false };
const observedDragStates = [];
let lastObservedDragState = state.attachmentDragActive;
const invokedCommands = [];
const feature = window.__PINVOU_TAURI_BRIDGE_FEATURES__.artifacts;
assert.equal(typeof feature, 'function');

const api = feature({
  state,
  notify() {
    if (state.attachmentDragActive !== lastObservedDragState) {
      lastObservedDragState = state.attachmentDragActive;
      observedDragStates.push(lastObservedDragState);
    }
  },
  async invoke(command, args) {
    invokedCommands.push({ command, args });
    if (command === 'ingest_dropped_file_chunk') {
      return {
        basename: args.filename,
        kind: 'pdf',
        path: `C:\\attachments\\${args.filename}`,
        markdown: 'test',
        token_estimate: 1,
        byte_size: args.total,
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
  async ensureSession() {},
  async discardManagedAttachment(result) {
    invokedCommands.push({
      command: 'discard_dropped_attachment',
      args: {
        sessionId: result.__pinvouManagedAttachmentSessionId,
        path: result.path,
      },
    });
  },
});

for (const eventName of ['dragenter', 'dragleave', 'dragover', 'drop']) {
  assert.ok(documentListeners.has(eventName), `${eventName} listener must be installed`);
}

let preventedDragEvents = 0;
const fakeFile = {
  name: 'a.pdf',
  size: 3,
  slice(start, end) {
    return {
      async arrayBuffer() {
        return Uint8Array.from([1, 2, 3].slice(start, end)).buffer;
      },
    };
  },
};
const dataTransfer = {
  types: ['Files'],
  files: [fakeFile],
  dropEffect: 'none',
};
const dragEvent = {
  dataTransfer,
  preventDefault() {
    preventedDragEvents += 1;
  },
};

documentListeners.get('dragenter')(dragEvent);
documentListeners.get('dragover')(dragEvent);
assert.equal(state.attachmentDragActive, true);
assert.equal(dataTransfer.dropEffect, 'copy');
documentListeners.get('drop')(dragEvent);
await new Promise(resolve => setTimeout(resolve, 0));

assert.deepEqual(observedDragStates, [true, false]);
assert.equal(preventedDragEvents, 3);
assert.equal(state.attachments.length, 1);
assert.equal(state.attachments[0].basename, 'a.pdf');
assert.equal(state.attachments[0].status, 'ready');
assert.equal(invokedCommands.length, 1);
assert.equal(invokedCommands[0].command, 'ingest_dropped_file_chunk');
assert.equal(invokedCommands[0].args.sessionId, 'session_test_123');
assert.equal(invokedCommands[0].args.commit, true);
assert.equal(invokedCommands[0].args.dataBase64, 'AQID');
assert.equal(
  Object.prototype.propertyIsEnumerable.call(
    state.attachments[0].result,
    '__pinvouManagedAttachmentSessionId',
  ),
  false,
  'managed lifecycle metadata must not cross the Tauri serialization boundary',
);

api.removeAttachment(state.attachments[0].id);
await new Promise(resolve => setTimeout(resolve, 0));
assert.equal(state.attachments.length, 0);
assert.equal(invokedCommands[1].command, 'discard_dropped_attachment');
assert.equal(invokedCommands[1].args.sessionId, 'session_test_123');

console.log('attachment drop bridge tests passed');
