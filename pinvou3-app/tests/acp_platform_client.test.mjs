import assert from 'node:assert/strict';

const allowedCommands = new Set([
  'web_access_codex_acp_prompt',
  'web_access_get_codex_acp_timeline',
  'web_access_get_codex_acp_session_info',
  'web_access_get_codex_acp_pending_permissions',
  'web_access_get_codex_acp_pending_elicitations',
  'web_access_list_acp_agents',
  'web_access_get_acp_agent_status',
  'web_access_set_codex_acp_model',
  'web_access_set_codex_acp_mode',
  'web_access_set_codex_acp_config_option',
  'web_access_create_codex_acp_session',
  'web_access_list_codex_workspace',
  'web_access_search_codex_workspace',
  'web_access_preview_codex_workspace_file',
  'web_access_get_codex_workspace_changes',
  'web_access_get_codex_workspace_diff',
  'web_access_cancel_codex_acp',
  'web_access_list_host_files',
  'web_access_upload_attachment_chunk',
  'web_access_abort_attachment_upload',
  'web_access_discard_attachment',
]);
const invocations = [];
let cancelAfterFirstChunk = false;
let cancelled = false;
let timelinePages = [];

const platform = {
  kind: 'web',
  isWeb: true,
  capabilities: { deviceFileUpload: true },
  can(capability) { return capability === 'deviceFileUpload'; },
  canInvoke(command) { return allowedCommands.has(command); },
};
globalThis.window = {
  PinvouPlatform: platform,
  btoa: value => Buffer.from(value, 'binary').toString('base64'),
  PinvouHostFilePicker: {
    async openWorkspace(options) {
      return {
        path: options.defaultPath || 'D:\\Projects\\pinvou',
        workspaceHandle: `workspace_${'c'.repeat(32)}`,
      };
    },
  },
  open() { return null; },
};
globalThis.window.window = globalThis.window;
globalThis.btoa = value => Buffer.from(value, 'binary').toString('base64');
globalThis.__TAURI__ = {
  core: {
    async invoke(command, args) {
      invocations.push({ command, args });
      if (command === 'web_access_get_codex_acp_timeline') {
        return timelinePages.length
          ? timelinePages.shift()
          : { events: [], nextAfterSeq: null, hasMore: false };
      }
      if (command === 'web_access_upload_attachment_chunk') {
        if (cancelAfterFirstChunk && args.offset === 0) cancelled = true;
        return args.commit
          ? {
              handle: `attachment_${'a'.repeat(24)}`,
              basename: args.fileName,
              kind: 'text',
              token_estimate: 1,
              byte_size: args.total,
              warning: null,
            }
          : null;
      }
      return { command, args };
    },
  },
};

await import(`../src/shared/chunked-file-upload.js?test=${Date.now()}`);
const acp = await import(`../src/features/codex/acpClient.js?test=${Date.now()}`);

{
  invocations.length = 0;
  allowedCommands.delete('web_access_get_codex_acp_session_info');
  await assert.rejects(
    acp.getAcpSessionInfo('session-1'),
    error => error.code === 'web_acp_command_unavailable',
  );
  assert.deepEqual(invocations, [], 'Web ACP must not fall back to a native command');
  allowedCommands.add('web_access_get_codex_acp_session_info');
}

{
  invocations.length = 0;
  for (const [webCommand, action] of [
    ['web_access_get_codex_acp_timeline', () => acp.loadAcpTimeline('session-1')],
    ['web_access_codex_acp_prompt', () => acp.submitAcpPrompt({
      sessionId: 'session-1',
      message: 'must stay on the Web lane',
      attachments: [],
      workspaceReferences: [],
    })],
  ]) {
    allowedCommands.delete(webCommand);
    await assert.rejects(action, error => error.code === 'web_acp_command_unavailable');
    allowedCommands.add(webCommand);
  }
  assert.deepEqual(invocations, [],
    'custom Web ACP paths must not fall back to native timeline or prompt commands');
}

{
  invocations.length = 0;
  const selection = await acp.pickAcpWorkspace({
    title: 'Choose project',
    defaultPath: 'E:\\Code\\project',
  });
  assert.deepEqual(selection, {
    path: 'E:\\Code\\project',
    workspaceHandle: `workspace_${'c'.repeat(32)}`,
  });
  await acp.createAcpSession({
    workspacePath: selection.path,
    workspaceHandle: selection.workspaceHandle,
    agentId: 'codex',
  });
  await acp.listAcpWorkspace({ sessionId: 'session-1', relativePath: 'src' });
  await acp.searchAcpWorkspace({ sessionId: 'session-1', query: 'main' });
  await acp.previewAcpWorkspaceFile({ sessionId: 'session-1', relativePath: 'src/main.rs' });
  await acp.loadAcpWorkspaceChanges({ sessionId: 'session-1' });
  await acp.loadAcpWorkspaceDiff({ sessionId: 'session-1', relativePath: 'src/main.rs' });
  await acp.cancelAcpSession('session-1');
  assert.deepEqual(invocations, [
    {
      command: 'web_access_create_codex_acp_session',
      args: { workspaceHandle: selection.workspaceHandle, agentId: 'codex' },
    },
    {
      command: 'web_access_list_codex_workspace',
      args: { sessionId: 'session-1', relativePath: 'src' },
    },
    {
      command: 'web_access_search_codex_workspace',
      args: { sessionId: 'session-1', query: 'main' },
    },
    {
      command: 'web_access_preview_codex_workspace_file',
      args: { sessionId: 'session-1', relativePath: 'src/main.rs' },
    },
    {
      command: 'web_access_get_codex_workspace_changes',
      args: { sessionId: 'session-1' },
    },
    {
      command: 'web_access_get_codex_workspace_diff',
      args: { sessionId: 'session-1', relativePath: 'src/main.rs' },
    },
    {
      command: 'web_access_cancel_codex_acp',
      args: { sessionId: 'session-1' },
    },
  ]);
  assert.equal(JSON.stringify(invocations).includes('E:\\\\Code'), false,
    'Web ACP RPCs must never contain a native workspace path');
  await assert.rejects(
    acp.createAcpSession({ workspacePath: 'C:\\private', agentId: 'codex' }),
    error => error.code === 'web_workspace_authorization_required',
  );
  await assert.rejects(
    acp.listAcpWorkspace({ workspacePath: 'C:\\private', relativePath: '' }),
    error => error.code === 'web_workspace_session_required',
  );
}

{
  invocations.length = 0;
  const attachment = {
    handle: `attachment_${'b'.repeat(24)}`,
    basename: 'notes.txt',
    path: 'C:\\Users\\private\\notes.txt',
    markdown: 'must stay on the desktop',
  };
  await acp.submitAcpPrompt({
    sessionId: 'session-1',
    message: 'review',
    attachments: [attachment],
    workspaceReferences: ['src/main.rs'],
  });
  assert.deepEqual(invocations, [{
    command: 'web_access_codex_acp_prompt',
    args: {
      sessionId: 'session-1',
      message: 'review',
      attachmentHandles: [attachment.handle],
      workspaceReferences: ['src/main.rs'],
    },
  }]);
  assert.equal(JSON.stringify(invocations).includes('Users'), false,
    'Web prompt payloads must never contain desktop paths or parsed attachment content');
}

{
  invocations.length = 0;
  await acp.loadAcpTimeline('session-1');
  await acp.getAcpSessionInfo('session-1');
  await acp.loadAcpPendingPermissions('session-1');
  await acp.loadAcpPendingElicitations('session-1');
  await acp.listAcpAgents();
  await acp.getAcpAgentStatus('claude', true);
  await acp.setAcpModel('session-1', 'gpt-5');
  await acp.setAcpMode('session-1', 'auto');
  await acp.setAcpConfigOption('session-1', 'thinking', 'high');
  assert.deepEqual(invocations.map(item => item.command), [
    'web_access_get_codex_acp_timeline',
    'web_access_get_codex_acp_session_info',
    'web_access_get_codex_acp_pending_permissions',
    'web_access_get_codex_acp_pending_elicitations',
    'web_access_list_acp_agents',
    'web_access_get_acp_agent_status',
    'web_access_set_codex_acp_model',
    'web_access_set_codex_acp_mode',
    'web_access_set_codex_acp_config_option',
  ]);
  assert.deepEqual(invocations[5].args, { agentId: 'claude', recheck: true });
  assert.deepEqual(invocations[0].args, { sessionId: 'session-1', afterSeq: 0, limit: 128 });
}

{
  invocations.length = 0;
  timelinePages = [
    { events: [{ sessionId: 'session-1', seq: 1 }], nextAfterSeq: 1, nextCursor: 120, hasMore: true },
    { events: [{ sessionId: 'session-1', seq: 2 }], nextAfterSeq: 2, nextCursor: 240, hasMore: false },
  ];
  const timeline = await acp.loadAcpTimeline('session-1');
  assert.deepEqual(timeline.map(event => event.seq), [1, 2]);
  assert.deepEqual(invocations.map(item => item.args.afterSeq), [0, 1]);
  assert.deepEqual(invocations.map(item => item.args.afterCursor), [undefined, 120]);
}

{
  invocations.length = 0;
  timelinePages = [
    { events: [{ sessionId: 'session-1', seq: 1 }], nextAfterSeq: 1, hasMore: true },
    { events: [], nextAfterSeq: 1, hasMore: true },
  ];
  await assert.rejects(
    acp.loadAcpTimeline('session-1'),
    error => error.code === 'web_acp_timeline_response_invalid',
  );
}

function mockFile(name, size) {
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
  invocations.length = 0;
  const file = mockFile('two-chunks.txt', acp.acpAttachmentLimits.chunkBytes + 9);
  const progress = [];
  const result = await acp.uploadAcpDeviceAttachment(file, {
    onProgress(value) { progress.push(value); },
  });
  const chunks = invocations.filter(item => item.command === 'web_access_upload_attachment_chunk');
  assert.equal(chunks.length, 2);
  assert.deepEqual(chunks.map(item => item.args.offset), [0, acp.acpAttachmentLimits.chunkBytes]);
  assert.deepEqual(chunks.map(item => item.args.commit), [false, true]);
  assert.ok(chunks.every(item => Buffer.from(item.args.dataBase64, 'base64').length
    <= acp.acpAttachmentLimits.chunkBytes));
  assert.equal(result.handle.startsWith('attachment_'), true);
  assert.equal(progress.at(-1), 100);
}

{
  invocations.length = 0;
  cancelAfterFirstChunk = true;
  cancelled = false;
  const file = mockFile('cancel.txt', acp.acpAttachmentLimits.chunkBytes + 1);
  await assert.rejects(
    acp.uploadAcpDeviceAttachment(file, { isCancelled: () => cancelled }),
    error => error.code === 'device_upload_cancelled',
  );
  cancelAfterFirstChunk = false;
  assert.equal(invocations.some(item => item.command === 'web_access_abort_attachment_upload'), true,
    'a cancelled partial upload must release the desktop buffer');
}

{
  invocations.length = 0;
  await assert.rejects(
    acp.uploadAcpDeviceAttachment(mockFile('empty.txt', 0)),
    error => error.code === 'device_upload_empty',
  );
  assert.equal(invocations.length, 0, 'empty files must fail before crossing Relay');
}

{
  invocations.length = 0;
  const oversized = mockFile('oversized.bin', acp.acpAttachmentLimits.maxBytes + 1);
  await assert.rejects(
    acp.uploadAcpDeviceAttachment(oversized),
    error => error.code === 'device_upload_too_large',
  );
  assert.equal(invocations.length, 0, 'oversized files must fail before crossing Relay');
}

console.log('ACP platform client tests passed');
