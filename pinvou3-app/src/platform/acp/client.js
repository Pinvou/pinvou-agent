import { can, canInvoke, isWeb } from '../../shared/platform.js';
import { invokeTauri, openTauriDialog } from '../tauri/client.js';

const DEVICE_UPLOAD_CHUNK_BYTES = 256 * 1024;
const DEVICE_UPLOAD_MAX_BYTES = 20 * 1024 * 1024;
const ACP_TIMELINE_PAGE_EVENTS = 128;

function bytesToBase64(bytes) {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return globalThis.btoa(binary);
}

function uploadId(prefix) {
  if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {
    return `${prefix}_${globalThis.crypto.randomUUID()}`;
  }
  return `${prefix}_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 12)}`;
}

function cancelledError() {
  const error = new Error('device-upload-cancelled');
  error.code = 'device_upload_cancelled';
  return error;
}

function invokeAcp(nativeCommand, webCommand, args) {
  const command = isWeb && canInvoke(webCommand) ? webCommand : nativeCommand;
  return args === undefined ? invokeTauri(command) : invokeTauri(command, args);
}

function invokeRequiredWebCommand(command, args) {
  if (!canInvoke(command)) {
    return Promise.reject(new Error(`Web ACP command is unavailable: ${command}`));
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
    throw new Error('Web workspace picker is unavailable');
  }
  const selected = await picker({ title, defaultPath });
  if (!selected) return null;
  const path = typeof selected.path === 'string' ? selected.path.trim() : '';
  const workspaceHandle = typeof selected.workspaceHandle === 'string'
    ? selected.workspaceHandle.trim()
    : '';
  if (!path || !workspaceHandle.startsWith('workspace_')) {
    throw new Error('Web workspace picker returned an invalid authorization');
  }
  return { path, workspaceHandle };
}

export function createAcpSession({ workspacePath, workspaceHandle, agentId }) {
  if (!isWeb) {
    return invokeTauri('create_codex_acp_session', { workspacePath, agentId });
  }
  if (workspacePath && !workspaceHandle) {
    return Promise.reject(new Error('Web project Sessions require a workspace authorization'));
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
  if (!sessionId) return Promise.reject(new Error('Web workspace reads require a Session'));
  return invokeRequiredWebCommand('web_access_list_codex_workspace', { sessionId, relativePath });
}

export function searchAcpWorkspace({ sessionId, query, workspacePath }) {
  if (!isWeb) {
    return invokeTauri('search_codex_workspace', { sessionId, query, workspacePath });
  }
  if (!sessionId) return Promise.reject(new Error('Web workspace reads require a Session'));
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
  if (!sessionId) return Promise.reject(new Error('Web workspace reads require a Session'));
  return invokeRequiredWebCommand('web_access_preview_codex_workspace_file', {
    sessionId,
    relativePath,
  });
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
    const error = new Error('device attachment upload is unavailable');
    error.code = 'device_upload_unavailable';
    throw error;
  }
  if (!file || !Number.isSafeInteger(file.size) || file.size < 0) {
    const error = new Error('invalid device attachment');
    error.code = 'device_upload_invalid';
    throw error;
  }
  if (file.size > DEVICE_UPLOAD_MAX_BYTES) {
    const error = new Error('device attachment exceeds 20 MB');
    error.code = 'device_upload_too_large';
    throw error;
  }

  const id = uploadId(isWeb ? 'webatt' : 'desktop_attach');
  const chunkCommand = isWeb
    ? 'web_access_upload_attachment_chunk'
    : 'ingest_draft_file_chunk';
  let offset = 0;
  let summary = null;
  const assertActive = () => {
    if (typeof options.isCancelled === 'function' && options.isCancelled()) {
      throw cancelledError();
    }
  };
  try {
    do {
      const slice = file.slice(offset, Math.min(offset + DEVICE_UPLOAD_CHUNK_BYTES, file.size));
      const bytes = new Uint8Array(await slice.arrayBuffer());
      assertActive();
      summary = await invokeTauri(chunkCommand, {
        uploadId: id,
        ...(isWeb ? { fileName: file.name } : { filename: file.name }),
        offset,
        total: file.size,
        dataBase64: bytesToBase64(bytes),
        commit: offset + bytes.length >= file.size,
      });
      offset += bytes.length;
      if (typeof options.onProgress === 'function') {
        options.onProgress(file.size ? Math.min(99, Math.round((offset / file.size) * 100)) : 99);
      }
    } while (offset < file.size);
    assertActive();
    if (isWeb && !acpAttachmentHandle(summary)) {
      throw new Error('upload did not return an attachment handle');
    }
    if (!isWeb && (!summary || !summary.basename)) {
      throw new Error('upload did not return a valid attachment');
    }
    if (!isWeb) {
      Object.defineProperty(summary, '__pinvouManagedDraftAttachmentId', {
        configurable: true,
        enumerable: false,
        value: id,
      });
    }
    if (typeof options.onProgress === 'function') options.onProgress(100);
    return summary;
  } catch (error) {
    if (acpAttachmentHandle(summary) && isWeb && canInvoke('web_access_discard_attachment')) {
      await invokeTauri('web_access_discard_attachment', { handle: summary.handle }).catch(() => {});
    } else if (isWeb && canInvoke('web_access_abort_attachment_upload')) {
      await invokeTauri('web_access_abort_attachment_upload', { uploadId: id }).catch(() => {});
    } else if (!isWeb && nativeDraftUpload) {
      await invokeTauri('cancel_draft_file_upload', { uploadId: id }).catch(() => {});
    }
    throw error;
  }
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
  if (!isWeb || !canInvoke('web_access_get_codex_acp_timeline')) {
    return invokeTauri('get_codex_acp_timeline', { sessionId });
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
      throw new Error('invalid ACP timeline pagination response');
    }
    if (page?.nextCursor != null) {
      const nextCursor = Number(page.nextCursor);
      if (!Number.isSafeInteger(nextCursor) || nextCursor < 0
          || (afterCursor !== null && nextCursor <= afterCursor)) {
        throw new Error('invalid ACP timeline cursor response');
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
  if (isWeb && canInvoke('web_access_codex_acp_prompt')) {
    const attachmentHandles = (attachments || []).map(acpAttachmentHandle);
    if (attachmentHandles.some(handle => !handle)) {
      return Promise.reject(new Error('Web ACP attachments require opaque handles'));
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
    return Promise.reject(new Error('invalid external URL'));
  }
  if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) {
    return Promise.reject(new Error('unsupported external URL'));
  }
  const opened = window.open(parsed.href, '_blank', 'noopener,noreferrer');
  if (opened) opened.opener = null;
  return Promise.resolve(Boolean(opened));
}

export const acpAttachmentLimits = Object.freeze({
  chunkBytes: DEVICE_UPLOAD_CHUNK_BYTES,
  maxBytes: DEVICE_UPLOAD_MAX_BYTES,
});
