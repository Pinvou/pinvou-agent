import { can, canInvoke, isWeb } from '../../shared/platform.js';
import { invokeTauri, openTauriDialog } from '../../platform/tauri/client.js';

const ACP_TIMELINE_PAGE_EVENTS = 128;

function acpClientError(code) {
  const error = new Error(code);
  error.code = code;
  return error;
}

function chunkedUploader() {
  const uploader = globalThis.window?.PinvouChunkedFileUpload;
  if (!uploader || typeof uploader.uploadFile !== 'function') {
    throw acpClientError('device_upload_unavailable');
  }
  return uploader;
}

function invokeAcp(nativeCommand, webCommand, args) {
  if (isWeb) return invokeRequiredWebCommand(webCommand, args);
  return args === undefined ? invokeTauri(nativeCommand) : invokeTauri(nativeCommand, args);
}

function invokeRequiredWebCommand(command, args) {
  if (!canInvoke(command)) {
    return Promise.reject(acpClientError('web_acp_command_unavailable'));
  }
  return args === undefined ? invokeTauri(command) : invokeTauri(command, args);
}

export async function pickAcpWorkspace({ title, defaultPath } = {}) {
  if (!isWeb) {
    const selected = await openTauriDialog({
      directory: true,
      multiple: false,
      title,
      ...(defaultPath ? { defaultPath } : {}),
    });
    const path = Array.isArray(selected) ? selected[0] : selected;
    return path ? { path, workspaceHandle: null } : null;
  }
  const picker = globalThis.window?.PinvouHostFilePicker?.openWorkspace;
  if (typeof picker !== 'function' || !canInvoke('web_access_list_host_files')) {
    throw acpClientError('web_workspace_picker_unavailable');
  }
  const selected = await picker({ title, defaultPath });
  if (!selected) return null;
  const path = typeof selected.path === 'string' ? selected.path.trim() : '';
  const workspaceHandle = typeof selected.workspaceHandle === 'string'
    ? selected.workspaceHandle.trim()
    : '';
  if (!path || !workspaceHandle.startsWith('workspace_')) {
    throw acpClientError('web_workspace_authorization_invalid');
  }
  return { path, workspaceHandle };
}

export function createAcpSession({ workspacePath, workspaceHandle, agentId }) {
  if (!isWeb) {
    return invokeTauri('create_codex_acp_session', { workspacePath, agentId });
  }
  if (workspacePath && !workspaceHandle) {
    return Promise.reject(acpClientError('web_workspace_authorization_required'));
  }
  return invokeRequiredWebCommand('web_access_create_codex_acp_session', {
    workspaceHandle: workspaceHandle || null,
    agentId,
  });
}

export function listAcpWorkspace({ sessionId, relativePath, workspacePath }) {
  if (!isWeb) {
    return invokeTauri('list_codex_workspace', { sessionId, relativePath, workspacePath });
  }
  if (!sessionId) return Promise.reject(acpClientError('web_workspace_session_required'));
  return invokeRequiredWebCommand('web_access_list_codex_workspace', { sessionId, relativePath });
}

export function searchAcpWorkspace({ sessionId, query, workspacePath }) {
  if (!isWeb) {
    return invokeTauri('search_codex_workspace', { sessionId, query, workspacePath });
  }
  if (!sessionId) return Promise.reject(acpClientError('web_workspace_session_required'));
  return invokeRequiredWebCommand('web_access_search_codex_workspace', { sessionId, query });
}

export function previewAcpWorkspaceFile({ sessionId, relativePath, workspacePath }) {
  if (!isWeb) {
    return invokeTauri('preview_codex_workspace_file', {
      sessionId,
      relativePath,
      workspacePath,
    });
  }
  if (!sessionId) return Promise.reject(acpClientError('web_workspace_session_required'));
  return invokeRequiredWebCommand('web_access_preview_codex_workspace_file', {
    sessionId,
    relativePath,
  });
}

export function loadAcpWorkspaceChanges({ sessionId }) {
  if (!isWeb) return invokeTauri('get_codex_workspace_changes', { sessionId });
  if (!sessionId) return Promise.reject(acpClientError('web_workspace_session_required'));
  return invokeRequiredWebCommand('web_access_get_codex_workspace_changes', { sessionId });
}

export function loadAcpWorkspaceDiff({ sessionId, relativePath }) {
  if (!isWeb) {
    return invokeTauri('get_codex_workspace_diff', { sessionId, relativePath });
  }
  if (!sessionId) return Promise.reject(acpClientError('web_workspace_session_required'));
  return invokeRequiredWebCommand('web_access_get_codex_workspace_diff', {
    sessionId,
    relativePath,
  });
}

export function cancelAcpSession(sessionId) {
  if (!isWeb) return invokeTauri('cancel_codex_acp', { sessionId });
  if (!sessionId) return Promise.reject(acpClientError('web_acp_session_required'));
  return invokeRequiredWebCommand('web_access_cancel_codex_acp', { sessionId });
}

export function acpAttachmentHandle(result) {
  return result && typeof result.handle === 'string' ? result.handle : '';
}

export async function ingestAcpAttachmentPath(path) {
  return invokeAcp('ingest_file', 'web_access_ingest_file', { path });
}

export async function uploadAcpDeviceAttachment(file, options = {}) {
  const nativeDraftUpload = !isWeb && canInvoke('ingest_draft_file_chunk');
  if (!can('deviceFileUpload') && !nativeDraftUpload) {
    throw acpClientError('device_upload_unavailable');
  }
  const uploader = chunkedUploader();
  const id = uploader.uploadId(isWeb ? 'webatt' : 'desktop_attach');
  const chunkCommand = isWeb
    ? 'web_access_upload_attachment_chunk'
    : 'ingest_draft_file_chunk';
  const completed = await uploader.uploadFile({
    file,
    uploadId: id,
    isCancelled: options.isCancelled,
    onProgress: options.onProgress,
    sendChunk: chunk => invokeTauri(chunkCommand, {
      uploadId: chunk.uploadId,
      ...(isWeb ? { fileName: chunk.fileName } : { filename: chunk.fileName }),
      offset: chunk.offset,
      total: chunk.total,
      dataBase64: chunk.dataBase64,
      commit: chunk.commit,
    }),
    validateResult: result => isWeb ? Boolean(acpAttachmentHandle(result)) : Boolean(result?.basename),
    cleanup: async upload => {
      const handle = acpAttachmentHandle(upload.result);
      if (isWeb && handle && canInvoke('web_access_discard_attachment')) {
        await invokeTauri('web_access_discard_attachment', { handle });
      } else if (isWeb && canInvoke('web_access_abort_attachment_upload')) {
        await invokeTauri('web_access_abort_attachment_upload', { uploadId: upload.uploadId });
      } else if (!isWeb && nativeDraftUpload
          && (upload.error?.code === 'device_upload_cancelled' || upload.commitAcknowledged)) {
        await invokeTauri('cancel_draft_file_upload', { uploadId: upload.uploadId });
      }
    },
  });
  const summary = completed.result;
  if (!isWeb) {
    Object.defineProperty(summary, '__pinvouManagedDraftAttachmentId', {
      configurable: true,
      enumerable: false,
      value: id,
    });
  }
  return summary;
}

export async function discardAcpAttachment(result) {
  const draftUploadId = result && result.__pinvouManagedDraftAttachmentId;
  if (!isWeb && draftUploadId) {
    await invokeTauri('cancel_draft_file_upload', { uploadId: draftUploadId });
    return true;
  }
  const managedSessionId = result && result.__pinvouManagedAttachmentSessionId;
  if (!isWeb && managedSessionId && result.path) {
    await invokeTauri('discard_dropped_attachment', {
      sessionId: managedSessionId,
      path: result.path,
    });
    return true;
  }
  const handle = acpAttachmentHandle(result);
  if (!handle || !isWeb || !canInvoke('web_access_discard_attachment')) return false;
  await invokeTauri('web_access_discard_attachment', { handle });
  return true;
}

export async function loadAcpTimeline(sessionId) {
  if (!isWeb) return invokeTauri('get_codex_acp_timeline', { sessionId });
  if (!canInvoke('web_access_get_codex_acp_timeline')) {
    throw acpClientError('web_acp_command_unavailable');
  }

  const events = [];
  let afterSeq = 0;
  let afterCursor = null;
  for (;;) {
    const page = await invokeTauri('web_access_get_codex_acp_timeline', {
      sessionId,
      afterSeq,
      ...(afterCursor === null ? {} : { afterCursor }),
      limit: ACP_TIMELINE_PAGE_EVENTS,
    });
    // Tolerate the short-lived pre-pagination wrapper during rolling desktop
    // upgrades; older clients that lack the complete ACP capability still fail
    // closed in bootstrap.
    if (Array.isArray(page)) return page;
    const nextEvents = Array.isArray(page?.events) ? page.events : [];
    events.push(...nextEvents);
    if (!page?.hasMore) return events;
    const nextAfterSeq = Number(page?.nextAfterSeq);
    if (!Number.isSafeInteger(nextAfterSeq) || nextAfterSeq <= afterSeq || nextEvents.length === 0) {
      throw acpClientError('web_acp_timeline_response_invalid');
    }
    if (page?.nextCursor != null) {
      const nextCursor = Number(page.nextCursor);
      if (!Number.isSafeInteger(nextCursor) || nextCursor < 0
          || (afterCursor !== null && nextCursor <= afterCursor)) {
        throw acpClientError('web_acp_timeline_response_invalid');
      }
      afterCursor = nextCursor;
    }
    afterSeq = nextAfterSeq;
  }
}

export function getAcpSessionInfo(sessionId) {
  return invokeAcp('get_codex_acp_session_info', 'web_access_get_codex_acp_session_info', { sessionId });
}

export function loadAcpPendingPermissions(sessionId) {
  return invokeAcp('get_codex_acp_pending_permissions',
    'web_access_get_codex_acp_pending_permissions', { sessionId });
}

export function loadAcpPendingElicitations(sessionId) {
  return invokeAcp('get_codex_acp_pending_elicitations',
    'web_access_get_codex_acp_pending_elicitations', { sessionId });
}

export function setAcpModel(sessionId, modelId) {
  return invokeAcp('set_codex_acp_model', 'web_access_set_codex_acp_model', { sessionId, modelId });
}

export function setAcpMode(sessionId, modeId) {
  return invokeAcp('set_codex_acp_mode', 'web_access_set_codex_acp_mode', { sessionId, modeId });
}

export function setAcpConfigOption(sessionId, configId, valueId) {
  return invokeAcp('set_codex_acp_config_option', 'web_access_set_codex_acp_config_option',
    { sessionId, configId, valueId });
}

export function listAcpAgents() {
  return invokeAcp('list_acp_agents', 'web_access_list_acp_agents');
}

export function listAcpSessions() {
  // Web 端走投影命令，主机绝对路径降级为目录名；桌面端保留完整路径。
  return invokeAcp('list_codex_acp_sessions', 'web_access_list_codex_acp_sessions');
}

export function getAcpAgentStatus(agentId, recheck = false) {
  const args = recheck ? { agentId, recheck: true } : { agentId };
  return invokeAcp('get_acp_agent_status', 'web_access_get_acp_agent_status', args);
}

async function adoptNativeDraftAttachments(sessionId, attachments) {
  const prepared = Array.from(attachments || []);
  for (const result of prepared) {
    const draftUploadId = result && result.__pinvouManagedDraftAttachmentId;
    if (!draftUploadId) continue;
    const adopted = await invokeTauri('adopt_draft_attachment', {
      sessionId,
      uploadId: draftUploadId,
    });
    for (const key of Object.keys(result)) delete result[key];
    Object.assign(result, adopted);
    delete result.__pinvouManagedDraftAttachmentId;
    Object.defineProperty(result, '__pinvouManagedAttachmentSessionId', {
      configurable: true,
      enumerable: false,
      value: sessionId,
    });
  }
  return prepared;
}

export async function submitAcpPrompt({ sessionId, message, attachments, workspaceReferences }) {
  if (isWeb) {
    if (!canInvoke('web_access_codex_acp_prompt')) {
      throw acpClientError('web_acp_command_unavailable');
    }
    const attachmentHandles = (attachments || []).map(acpAttachmentHandle);
    if (attachmentHandles.some(handle => !handle)) {
      return Promise.reject(acpClientError('web_acp_attachment_invalid'));
    }
    return invokeTauri('web_access_codex_acp_prompt', {
      sessionId,
      message,
      attachmentHandles,
      workspaceReferences,
    });
  }
  const preparedAttachments = await adoptNativeDraftAttachments(sessionId, attachments);
  return invokeTauri('codex_acp_prompt', {
    sessionId,
    message,
    attachments: preparedAttachments,
    workspaceReferences,
  });
}

export function openAcpExternalUrl(value) {
  if (!isWeb) return invokeTauri('open_user_external_url', { url: value });
  let parsed;
  try {
    parsed = new URL(String(value || ''));
  } catch {
    return Promise.reject(acpClientError('acp_external_url_invalid'));
  }
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) {
    return Promise.reject(acpClientError('acp_external_url_invalid'));
  }
  const opened = window.open(parsed.href, '_blank', 'noopener,noreferrer');
  if (opened) opened.opener = null;
  return Promise.resolve(Boolean(opened));
}

export const acpAttachmentLimits = Object.freeze({
  get chunkBytes() { return chunkedUploader().CHUNK_BYTES; },
  get maxBytes() { return chunkedUploader().MAX_FILE_BYTES; },
});
