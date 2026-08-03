import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const stateUrl = new URL('../src/features/codex/runtimeNoticeState.js', import.meta.url);
const stateSource = await readFile(stateUrl, 'utf8');
const stateModule = await import(`data:text/javascript;base64,${Buffer.from(stateSource).toString('base64')}`);
const {
  classifyAcpServiceFailure,
  isAcpAuthenticationFailure,
  runtimeInstallInProgress,
  runtimeLoginInProgress,
  runtimeNoticeMode,
  runtimeOperationFor,
} = stateModule;

const ready = {
  bridge_ready: true,
  installed: true,
  authenticated: true,
  error: null,
};

assert.equal(runtimeNoticeMode(null), 'checking');
assert.equal(runtimeNoticeMode({ ...ready, bridge_ready: false }), 'bridge_unavailable');

for (const agent_id of ['codex', 'claude', 'kimi']) {
  assert.equal(
    runtimeNoticeMode({ ...ready, agent_id, installed: false }),
    'install',
    `${agent_id} missing CLI must reach the install notice`,
  );
}

assert.equal(runtimeNoticeMode({ ...ready, authenticated: false }), 'login');
assert.equal(runtimeNoticeMode({ ...ready, error: 'failed' }), 'error');
assert.equal(runtimeNoticeMode(ready), 'ready');

const installingClaude = { claude: 'install' };
assert.equal(runtimeOperationFor(installingClaude, 'claude'), 'install');
assert.equal(runtimeOperationFor(installingClaude, 'codex'), '');
assert.equal(runtimeOperationFor(installingClaude, 'kimi'), '');
assert.equal(
  runtimeInstallInProgress(ready, runtimeOperationFor(installingClaude, 'claude')),
  true,
  'Claude installation must only mark Claude as installing',
);
assert.equal(
  runtimeInstallInProgress(ready, runtimeOperationFor(installingClaude, 'codex')),
  false,
  'Claude installation must not mark Codex as installing',
);

const loggingInClaude = { claude: 'login' };
assert.equal(
  runtimeLoginInProgress(ready, runtimeOperationFor(loggingInClaude, 'claude')),
  true,
  'Claude login must only mark Claude as logging in',
);
assert.equal(
  runtimeLoginInProgress(ready, runtimeOperationFor(loggingInClaude, 'codex')),
  false,
  'Claude login must not mark Codex as logging in',
);
assert.equal(runtimeLoginInProgress({ ...ready, login_in_progress: true }), true);

const kimiModelNotConfigured = {
  seq: 7,
  timestamp: '2026-08-03T04:00:00Z',
  event: {
    type: 'turn_completed',
    data: {
      error: 'Kimi Code 请求失败（model.not_configured）：LLM not set, send "/login" to login',
    },
  },
};
assert.equal(
  isAcpAuthenticationFailure(kimiModelNotConfigured),
  true,
  'Kimi missing model configuration must refresh authentication status',
);
assert.equal(
  classifyAcpServiceFailure(kimiModelNotConfigured)?.kind,
  'authentication',
  'Kimi missing model configuration must offer account recovery instead of generic downtime',
);

const view = await readFile(
  new URL('../src/features/codex/CodexAcpView.jsx', import.meta.url),
  'utf8',
);
assert.match(
  view,
  /refreshStatus\(activeAgentId, true\)\.catch\(showError\)/,
  'switching the active agent must force a fresh CLI probe',
);
assert.match(
  view,
  /beginRuntimeOperation\(agentId, 'install'\)/,
  'runtime operations must be recorded for the target Agent',
);
assert.match(
  view,
  /operation=\{activeRuntimeOperation\}/,
  'the runtime notice must only consume the active Agent operation',
);
assert.doesNotMatch(
  view,
  /working \|\| waitingForLogin \? copy\.waitAuth/,
  'unrelated work must not render the active Agent as logging in',
);
assert.doesNotMatch(view, /managed_download|managedDownload|downloadManaged/);

console.log('✓ ACP runtime notice state matrix passed');
