import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => fs.readFileSync(path.join(ROOT, ...parts), 'utf8');

// The computer-use consent commands are first-party desktop surfaces: the
// remote web client must never learn them (the same exclusion pattern the
// multiagent commands pin in multiagent_plan_normalize.test.mjs). The
// consent events are likewise first-party — the remote transport rejects
// them by the same policy file.
const DESKTOP_COMMANDS = [
  'computer_use_get_status',
  'computer_use_grant',
  'computer_use_revoke',
  'computer_use_stop',
  'computer_use_confirm',
  'computer_use_deny',
  'computer_use_set_enabled',
  'computer_use_request_permissions',
];

const DESKTOP_EVENTS = [
  'computer_use:grant_required',
  'computer_use:confirm_required',
  'computer_use:state_changed',
];

test('computer_use stays desktop-exclusive: web policy and web bridge must not learn it', () => {
  const policy = JSON.parse(read('src', 'platform', 'web', 'access-policy.json'));
  const webAdapter = read('src', 'platform', 'web', 'bridge', 'domain-adapter.js');
  const desktopBridgeFeature = read('src', 'platform', 'tauri', 'bridge', 'computer_use.js');

  for (const command of DESKTOP_COMMANDS) {
    assert.equal(
      policy.allowed_commands.includes(command),
      false,
      `${command} must stay off the web access-policy allowlist`,
    );
    assert.equal(
      webAdapter.includes(command),
      false,
      `the web domain adapter must not proxy ${command}`,
    );
    assert.ok(
      desktopBridgeFeature.includes(command),
      `the desktop bridge must expose ${command} (the exclusion only makes sense while the desktop side exists)`,
    );
  }

  for (const event of DESKTOP_EVENTS) {
    assert.equal(
      policy.allowed_events.includes(event),
      false,
      `${event} must never reach the web client`,
    );
  }
});
