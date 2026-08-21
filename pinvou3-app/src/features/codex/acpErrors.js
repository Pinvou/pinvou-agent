const CONTROLLED_WEB_ERROR = /^(?:web_workspace_(?:listing|search|preview|changes|diff)_failed|web_acp_[a-z0-9_]+_failed|web_session_[a-z0-9_]+_failed|invalid_web_session_id|web_session_unavailable)$/;
const CONTROLLED_CLIENT_ERRORS = new Set([
  'acp_external_url_invalid',
  'device_upload_unavailable',
  'web_acp_attachment_invalid',
  'web_acp_command_unavailable',
  'web_acp_session_required',
  'web_acp_timeline_response_invalid',
  'web_workspace_authorization_invalid',
  'web_workspace_authorization_required',
  'web_workspace_picker_unavailable',
  'web_workspace_session_required',
]);

export function acpErrorMessage(error, copy, options = {}) {
  const raw = String(error?.message || error || '');
  if (CONTROLLED_WEB_ERROR.test(raw) || CONTROLLED_CLIENT_ERRORS.has(raw)) {
    return copy.operationFailed;
  }
  const allowRaw = options.allowRaw ?? copy.showRawErrors;
  return allowRaw ? raw : copy.operationFailed;
}
