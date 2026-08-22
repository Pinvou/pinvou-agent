import assert from 'node:assert/strict';
import { acpErrorMessage } from '../src/features/codex/acpErrors.js';

const copy = { operationFailed: 'localized failure', showRawErrors: true };
for (const code of [
  'web_workspace_changes_failed',
  'web_workspace_diff_failed',
  'web_acp_timeline_failed',
  'web_acp_session_create_failed',
  'web_acp_cancel_failed',
  'web_acp_prompt_failed',
  'web_acp_agent_status_failed',
  'web_acp_command_unavailable',
  'web_acp_timeline_response_invalid',
  'web_workspace_authorization_required',
  'web_workspace_session_required',
  'web_session_list_sessions_failed',
  'web_session_load_session_chunk_failed',
  'acp_external_url_invalid',
  'invalid_web_session_id',
  'web_session_unavailable',
]) {
  assert.equal(acpErrorMessage(new Error(code), copy), copy.operationFailed);
}
assert.equal(acpErrorMessage(new Error('desktop diagnostic'), copy), 'desktop diagnostic');
assert.equal(
  acpErrorMessage(new Error('unknown /Users/example/.codex failure'), copy, { allowRaw: false }),
  copy.operationFailed,
);
assert.equal(
  acpErrorMessage(new Error('desktop diagnostic'), copy, { allowRaw: true }),
  'desktop diagnostic',
);

console.log('ACP error message tests passed');
