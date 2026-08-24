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
  assert.equal(
    runtimeNoticeMode({
      ...ready,
      agent_id,
      update_available: true,
      version: '1.0.0',
      latest_version: '1.1.0',
    }),
    'install',
    `${agent_id} below official latest must reach the upgrade notice`,
  );
  assert.equal(
    runtimeNoticeMode({
      ...ready,
      agent_id,
      update_available: true,
      version: '1.0.0',
      latest_version: '1.1.0',
    }, true),
    'ready',
    `${agent_id} must remain usable after deferring an advisory upgrade`,
  );
}

assert.equal(
  runtimeNoticeMode({ ...ready, authenticated: false, update_available: true }, true),
  'login',
  'deferring an advisory upgrade must continue into the login flow',
);
assert.equal(
  runtimeNoticeMode({ ...ready, installed: false, update_required: true }, true),
  'install',
  'a mandatory upgrade must not be deferrable',
);
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
const notices = await readFile(
  new URL('../src/features/codex/AcpRuntimeNotices.jsx', import.meta.url),
  'utf8',
);
const runtimeStatus = await readFile(
  new URL('../src/features/codex/runtimeStatus.js', import.meta.url),
  'utf8',
);
const draftStatusEffect = view.match(
  /useEffect\(\(\) => \{\s+\/\/ 草稿态按需读取([\s\S]*?)\n  \}, \[activeAgentId, activeId\]\);/,
);
assert.ok(draftStatusEffect, 'draft Agent status effect must remain explicit');
assert.match(
  draftStatusEffect[1],
  /if \(activeId\) return;[\s\S]*refreshStatus\(activeAgentId\)\.catch\(showError\)/,
  'draft Agent switches must use the cached probe while session loading owns active-session status',
);
assert.doesNotMatch(
  draftStatusEffect[1],
  /refreshStatus\(activeAgentId, true\)/,
  'ordinary Agent switches must not force a duplicate CLI probe',
);
assert.match(
  view,
  /onRefresh=\{\(\) => refreshStatus\(activeAgentId, true\)\}/,
  'the explicit recheck action must still bypass the probe cache',
);
assert.match(
  view,
  /codeAgentsLoading=\{agents === null\}/,
  'the selector must distinguish a loading catalog from a Codex-only catalog',
);
assert.match(
  runtimeStatus,
  /inFlight = Promise\.resolve\(\)\.then\(task\)[\s\S]*await inFlight/,
  'runtime status polling must serialize probes and wait for the in-flight probe on stop',
);
assert.doesNotMatch(
  view,
  /setInterval\(\(\) => refreshStatus/,
  'installation polling must not overlap slow status probes',
);
assert.match(
  runtimeStatus,
  /requestSeqRef = useRef\(\{\}\)[\s\S]*?requestSeqRef\.current\[agentId\] !== sequence/,
  'late status responses must not overwrite the currently selected Agent',
);
assert.match(
  runtimeStatus,
  /mountedRef\.current = false[\s\S]*?if \(!mountedRef\.current\) return false/,
  'late status responses must not update an unmounted code-mode view',
);
assert.match(
  view,
  /function selectDraftAgent\(agentId\)[\s\S]*?activeAgentIdRef\.current = agentId;[\s\S]*?setDraftAgentId\(agentId\)/,
  'Agent selection must close the response race before React renders the new selection',
);
assert.match(
  view,
  /listenTauri\('acp:event',[\s\S]*?if \(disposed\) return;[\s\S]*?\.then\(fn => \{\s*if \(disposed\) fn\(\)/,
  'an asynchronously registered ACP listener must not survive unmount',
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
assert.match(
  notices,
  /copy\.cliUpdateRequired\(agentName, status\.version, status\.latest_version\)/,
  'the mandatory upgrade notice must show the target version',
);
assert.match(
  notices,
  /copy\.cliUpdateAvailable\(agentName, status\.version, status\.latest_version\)/,
  'the advisory upgrade notice must show the official latest target version',
);
assert.match(
  notices,
  /const canDeferUpgrade = status\.update_available && status\.installed && !status\.update_required/,
  'only an advisory latest-version update may be deferred',
);
assert.match(
  notices,
  /\[resetKey, status\?\.agent_id, status\?\.installed, status\?\.latest_version\]/,
  'starting a new code draft or reselecting an Agent must show the advisory again',
);
assert.match(view, /resetKey=\{draftEpoch\}/);
assert.match(
  view,
  /suppressAdvisoryUpgrade=\{Boolean\(activeId\)\}/,
  'existing sessions must suppress the optional latest-version reminder',
);
assert.match(
  notices,
  /runtimeNoticeMode\(status, declinedUpgrade \|\| suppressAdvisoryUpgrade\)/,
  'session suppression must reuse advisory-only behavior without hiding mandatory gates',
);
assert.doesNotMatch(
  notices,
  /working \|\| waitingForLogin \? copy\.waitAuth/,
  'unrelated work must not render the active Agent as logging in',
);
assert.doesNotMatch(notices, /managed_download|managedDownload|downloadManaged/);

console.log('✓ ACP runtime notice state matrix passed');
