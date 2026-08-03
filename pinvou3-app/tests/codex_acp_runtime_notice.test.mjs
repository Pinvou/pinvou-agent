import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const stateUrl = new URL('../src/features/codex/runtimeNoticeState.js', import.meta.url);
const stateSource = await readFile(stateUrl, 'utf8');
const stateModule = await import(`data:text/javascript;base64,${Buffer.from(stateSource).toString('base64')}`);
const { runtimeNoticeMode } = stateModule;

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

const view = await readFile(
  new URL('../src/features/codex/CodexAcpView.jsx', import.meta.url),
  'utf8',
);
assert.match(
  view,
  /refreshStatus\(activeAgentId, true\)\.catch\(showError\)/,
  'switching the active agent must force a fresh CLI probe',
);
assert.doesNotMatch(view, /managed_download|managedDownload|downloadManaged/);

console.log('✓ ACP runtime notice state matrix passed');
