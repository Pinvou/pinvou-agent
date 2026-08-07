import assert from 'node:assert/strict';

const invocations = [];
globalThis.window = {
  PinvouPlatform: {
    kind: 'desktop',
    isWeb: false,
    capabilities: { deviceFileUpload: false },
  },
};
globalThis.window.window = globalThis.window;
globalThis.btoa = value => Buffer.from(value, 'binary').toString('base64');
globalThis.__TAURI__ = {
  core: {
    async invoke(command, args) {
      invocations.push({ command, args });
      if (command === 'ingest_draft_file_chunk') {
        return args.commit ? {
          basename: args.filename,
          kind: 'text',
          path: `C:\\drafts\\${args.uploadId}\\${args.filename}`,
          markdown: 'draft',
          token_estimate: 1,
          byte_size: args.total,
          warning: null,
        } : null;
      }
      if (command === 'adopt_draft_attachment') {
        return {
          basename: 'notes.txt',
          kind: 'text',
          path: `C:\\sessions\\${args.sessionId}\\attachments\\notes.txt`,
          markdown: 'draft',
          token_estimate: 1,
          byte_size: 3,
          warning: null,
        };
      }
      return { ok: true };
    },
  },
};

const acp = await import(`../src/platform/acp/client.js?native-test=${Date.now()}`);

function mockFile(name = 'notes.txt') {
  const bytes = Uint8Array.from([1, 2, 3]);
  return {
    name,
    size: bytes.length,
    slice(start, end) {
      const chunk = bytes.slice(start, end);
      return { async arrayBuffer() { return chunk.buffer; } };
    },
  };
}

const attachment = await acp.uploadAcpDeviceAttachment(mockFile());
assert.equal(invocations[0].command, 'ingest_draft_file_chunk');
assert.equal('sessionId' in invocations[0].args, false);
assert.equal(attachment.__pinvouManagedDraftAttachmentId.startsWith('desktop_attach_'), true);
assert.equal(
  Object.prototype.propertyIsEnumerable.call(attachment, '__pinvouManagedDraftAttachmentId'),
  false,
);

const uploadId = attachment.__pinvouManagedDraftAttachmentId;
await acp.submitAcpPrompt({
  sessionId: 'acp-session-1',
  message: 'review',
  attachments: [attachment],
  workspaceReferences: [],
});
assert.deepEqual(invocations.at(-2), {
  command: 'adopt_draft_attachment',
  args: { sessionId: 'acp-session-1', uploadId },
});
assert.equal(invocations.at(-1).command, 'codex_acp_prompt');
assert.match(invocations.at(-1).args.attachments[0].path, /sessions\\acp-session-1/);
assert.equal(attachment.__pinvouManagedDraftAttachmentId, undefined);
assert.equal(attachment.__pinvouManagedAttachmentSessionId, 'acp-session-1');

await acp.discardAcpAttachment(attachment);
assert.equal(invocations.at(-1).command, 'discard_dropped_attachment');

const cancelled = await acp.uploadAcpDeviceAttachment(mockFile('cancel.txt'));
await acp.discardAcpAttachment(cancelled);
assert.equal(invocations.at(-1).command, 'cancel_draft_file_upload');

console.log('ACP native draft attachment client tests passed');
