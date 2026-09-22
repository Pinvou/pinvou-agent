import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const stateUrl = new URL('../src/features/codex/runtimeNoticeState.js', import.meta.url);
const stateSource = await readFile(stateUrl, 'utf8');
const stateModule = await import(`data:text/javascript;base64,${Buffer.from(stateSource).toString('base64')}`);
const {
  classifyAcpServiceFailure,
  isAcpAuthenticationFailure,
  latestAgentRuntimeNotice,
  runtimeInstallInProgress,
  runtimeLoginInProgress,
  runtimeNoticeMode,
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
assert.equal(
  runtimeInstallInProgress(ready, installingClaude.claude || ''),
  true,
  'Claude installation must only mark Claude as installing',
);
assert.equal(
  runtimeInstallInProgress(ready, installingClaude.codex || ''),
  false,
  'Claude installation must not mark Codex as installing',
);

const loggingInClaude = { claude: 'login' };
assert.equal(
  runtimeLoginInProgress(ready, loggingInClaude.claude || ''),
  true,
  'Claude login must only mark Claude as logging in',
);
assert.equal(
  runtimeLoginInProgress(ready, loggingInClaude.codex || ''),
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
  /useEffect\(\(\) => \{\s+\/\/ In draft, read the selected agent's status([\s\S]*?)\n {2}\}, \[activeAgentId, activeId\]\);/,
);
assert.ok(draftStatusEffect, 'draft Agent status effect must remain explicit');
assert.match(
  draftStatusEffect[1],
  /if \(activeId\) return;[\s\S]*refreshStatus\(activeAgentId, true\)\.catch\(showError\)/,
  'draft Agent switches must force a fresh CLI probe (installs outside the app) while session loading owns active-session status',
);
assert.doesNotMatch(
  draftStatusEffect[1],
  /refreshStatus\(activeAgentId\)\.catch\(showError\)/,
  'draft Agent switches must not silently read a stale probe cache',
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

// Agent 侧运行时提示：适配器 stderr 与回合看门狗的兜底动作都必须能浮到界面，
// 并在后续回合成功完成后自动过期。
const envelope = (seq, type, data) => ({ seq, timestamp: `t${seq}`, event: { type, data } });
assert.equal(latestAgentRuntimeNotice([]), null, 'no events means no notice');
assert.equal(
  latestAgentRuntimeNotice([envelope(1, 'turn_started', {})]),
  null,
  'a plain turn start is not a notice',
);
const stderrNotice = latestAgentRuntimeNotice([
  envelope(1, 'turn_started', {}),
  envelope(2, 'runtime_notice', {
    kind: 'agent_stderr',
    agent: 'claude',
    detail: 'cancel floor elapsed without the SDK yielding',
  }),
]);
assert.equal(stderrNotice?.kind, 'agent_stderr');
assert.equal(stderrNotice?.detail, 'cancel floor elapsed without the SDK yielding');
assert.equal(
  latestAgentRuntimeNotice([
    envelope(1, 'runtime_notice', { kind: 'agent_stall', quietSeconds: 200 }),
    envelope(2, 'runtime_notice', { kind: 'not_a_known_kind' }),
  ])?.kind,
  'agent_stall',
  'unknown notice kinds must not surface',
);
assert.equal(
  latestAgentRuntimeNotice([
    envelope(1, 'runtime_notice', { kind: 'agent_stall_settled' }),
    envelope(2, 'turn_started', {}),
  ])?.kind,
  'agent_stall_settled',
  'a newer user turn must not hide the notice that was produced for it (the restart notice is emitted on the next message)',
);
assert.equal(
  latestAgentRuntimeNotice([
    envelope(1, 'runtime_notice', { kind: 'agent_stall_settled' }),
    envelope(2, 'turn_completed', { status: 'Interrupted' }),
  ])?.kind,
  'agent_stall_settled',
  'the notice survives its own interrupted completion so the ending stays explained',
);
assert.equal(
  latestAgentRuntimeNotice([
    envelope(1, 'runtime_notice', { kind: 'cancel_timeout' }),
    envelope(2, 'turn_started', {}),
    envelope(3, 'turn_completed', { status: 'Completed' }),
  ]),
  null,
  'a completed turn afterwards means the session recovered, so the notice expires',
);
assert.equal(
  latestAgentRuntimeNotice([
    envelope(1, 'turn_started', {}),
    envelope(2, 'runtime_notice', { kind: 'cancel_timeout' }),
    envelope(3, 'turn_completed', { status: 'Interrupted' }),
  ])?.kind,
  'cancel_timeout',
  'the notice survives its own turn completion so the ending stays explained',
);
assert.match(
  notices,
  /data-testid="acp-agent-runtime-notice"/,
  'the ACP notices module must render the agent runtime notice',
);
assert.match(
  view,
  /latestAgentRuntimeNotice\(events\)/,
  'the ACP view must derive the notice from the event stream',
);
for (const kind of ['agent_stall_restart', 'agent_session_restarted', 'agent_session_restarted_fresh']) {
  assert.equal(
    latestAgentRuntimeNotice([envelope(1, 'runtime_notice', { kind })])?.kind,
    kind,
    `${kind} must surface (repeated-stall escalation and the restart itself)`,
  );
}
assert.match(
  view,
  /redactDisplayError\(notice\.detail/,
  'the notice detail is adapter stderr text and must be redacted like turn errors',
);
assert.match(
  notices,
  /copy\.agentRestartHint/,
  'the restarted-session notice must use its own hint copy',
);
assert.match(
  notices,
  /notice\.detail \? copy\.agentStderrHint : copy\.agentStderrNoDetailHint/,
  'Web notices without projected stderr detail must not promise raw adapter text',
);
assert.match(
  notices,
  /notice\.kind === 'agent_stall_cancel'[\s\S]*copy\.agentStallCancelHint/,
  'an automatic cancel must not tell the user to press Stop again',
);

console.log('✓ ACP runtime notice state matrix passed');
