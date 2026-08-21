import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const skillRoot = resolve(
  appRoot,
  'src-tauri/resources/common/bundle/pinvouos-agent-skills/pinvou-browser-auth',
);
const brokerPath = resolve(skillRoot, 'scripts/qqmusic_wechat_auth.py');

test(
  'browser auth broker passes its isolated stdlib protocol and state self-test',
  { skip: process.platform !== 'linux' },
  () => {
    const run = spawnSync('/usr/bin/python3', ['-I', brokerPath, '_self-test'], {
      encoding: 'utf8',
      env: {
        ...process.env,
        PYTHONHOME: '/definitely/not/a/python-home',
        PYTHONPATH: '/definitely/not/a/python-path',
        PYTHONDONTWRITEBYTECODE: '1',
      },
      timeout: 10_000,
    });

    assert.equal(run.signal, null, run.stderr);
    assert.equal(run.status, 0, run.stderr);
    assert.deepEqual(JSON.parse(run.stdout), { ok: true });
  },
);

test('browser auth bundle keeps the warm/card and no-secret transport contract', () => {
  const broker = readFileSync(brokerPath, 'utf8');
  const skill = readFileSync(resolve(skillRoot, 'SKILL.md'), 'utf8');

  assert.match(broker, /class StdlibWebSocket/);
  assert.match(broker, /fcntl\.flock/);
  assert.match(broker, /ProxyHandler\(\{\}\)/);
  assert.match(broker, /prior_verified/);
  assert.match(
    broker,
    /PROCESS_ACTIVE_LIFETIME_SECONDS = MAX_TTL_SECONDS - PROCESS_CLEANUP_GUARD_SECONDS/,
  );
  assert.match(broker, /stale_active_job/);
  assert.match(broker, /authorized_handoff_expired/);
  assert.match(
    broker,
    /state\.get\("status"\) == "authorized" and not active_state_is_reusable/,
  );
  assert.match(broker, /signal\.setitimer\(signal\.ITIMER_REAL/);
  assert.match(broker, /ISOLATED_PYTHON,\s*"-I"/);
  assert.doesNotMatch(broker, /import websocket/);
  assert.doesNotMatch(broker, /\bpkill\b/);
  assert.doesNotMatch(broker, /qr_url/i);
  assert.equal((broker.match(/\bprint\(/g) ?? []).length, 1);

  assert.match(skill, /status: authorized.*evidence\.prior_verified: true/);
  assert.match(skill, /Only for `waiting`/);
  assert.match(skill, /qqmusic_wechat_auth_action/);
  assert.match(skill, /capability_unavailable/);
  assert.doesNotMatch(skill, /Front's clickable authorization-choice surface/);
  assert.match(skill, /\/usr\/bin\/python3 -I/);
});
