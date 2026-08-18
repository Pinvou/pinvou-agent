import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(path.join(root, 'src/shared/authority-sync-diagnostics.js'), 'utf8');

function harness({ web = false, available = true, fail = false } = {}) {
  const storage = new Map();
  const calls = [];
  let connectionListener = null;
  const window = {
    crypto: { randomUUID: () => 'fixed-run-id' },
    navigator: { onLine: true },
    document: { visibilityState: 'visible' },
    localStorage: {
      getItem: key => storage.get(key) || null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: key => storage.delete(key),
    },
    PinvouPlatform: {
      kind: web ? 'web' : 'desktop',
      isWeb: web,
      canInvoke: () => available,
      getConnectionState: () => ({ status: available ? 'ready' : 'connecting', desktop_online: available }),
      onConnectionChange: listener => { connectionListener = listener; },
    },
    __TAURI__: {
      core: {
        invoke: async (command, args) => {
          calls.push({ command, args });
          if (fail) throw new Error('offline');
        },
      },
    },
    addEventListener() {},
    setTimeout() { return 1; },
    clearTimeout() {},
  };
  window.window = window;
  vm.runInNewContext(source, { window, Date, JSON, Math, Object, Promise }, { filename: 'authority-sync-diagnostics.js' });
  return { window, storage, calls, connectionListener: () => connectionListener };
}

test('diagnostics redact content and flush one centralized batch', async () => {
  const { window, calls } = harness();
  window.PinvouAuthoritySyncDiagnostics.record('reconcile_attempt', {
    session_id: 'chat-safe',
    message_count: 3,
    message: 'private user text',
    access_token: 'private-token',
    error: 'relay rejected Bearer inline-secret https://x.test/?token=query-secret',
  });
  await window.PinvouAuthoritySyncDiagnostics.flush();
  assert.ok(calls.length >= 1);
  const entries = calls.flatMap(call => call.args.entries);
  const attempt = entries.find(entry => entry.event === 'reconcile_attempt');
  assert.equal(attempt.details.session_id, 'chat-safe');
  assert.equal(attempt.details.message_count, 3);
  assert.equal(attempt.details.message, '[REDACTED]');
  assert.equal(attempt.details.access_token, '[REDACTED]');
  assert.equal(attempt.details.error.includes('inline-secret'), false);
  assert.equal(attempt.details.error.includes('query-secret'), false);
  assert.ok(calls.every(call => call.command === 'record_authority_sync_diagnostics'));
});

test('web diagnostics remain queued while unavailable', async () => {
  const { window, calls, storage } = harness({ web: true, available: false });
  window.PinvouAuthoritySyncDiagnostics.record('reconcile_failed', { reason: 'load_error' });
  await window.PinvouAuthoritySyncDiagnostics.flush();
  assert.equal(calls.length, 0);
  assert.ok(window.PinvouAuthoritySyncDiagnostics.pendingCount() >= 2);
  assert.ok(storage.get('pinvou.authority_sync.diagnostics.v1'));
});

test('authority reconciliation lifecycle is covered by the centralized diagnostics', () => {
  const webBridge = fs.readFileSync(path.join(root, 'src/platform/web/bridge.js'), 'utf8');
  const desktopBridge = [
    'src/platform/tauri/bridge.js',
    'src/platform/tauri/bridge/chat-events.js',
    'src/platform/tauri/bridge/chat.js',
    'src/platform/tauri/bridge/interaction.js',
  ].map(file => fs.readFileSync(path.join(root, file), 'utf8')).join('\n');
  const backend = [
    'src-tauri/src/features/assistant/engine.rs',
    'src-tauri/src/features/remote_control/manager/mod.rs',
    'src-tauri/src/app/commands/sessions.rs',
    'src-tauri/src/app/commands/remote_control.rs',
  ].map(file => fs.readFileSync(path.join(root, file), 'utf8')).join('\n');
  for (const event of [
    'local_turn_claimed',
    'remote_turn_marked',
    'transcript_committed_event_received',
    'chat_done_classified',
    'reconcile_started',
    'reconcile_attempt_rejected',
    'reconcile_attempt_failed',
    'reconcile_succeeded',
    'reconcile_exhausted',
    'authority_sync_notice_shown',
    'remote_sync_blocked_action',
  ]) {
    assert.ok(webBridge.includes(`"${event}"`), `Web bridge is missing ${event}`);
    assert.ok(desktopBridge.includes(`"${event}"`), `desktop bridge is missing ${event}`);
  }
  for (const event of [
    'transcript_committed_emitting',
    'chat_done_emitting',
    'desktop_load_session_succeeded',
    'desktop_load_session_failed',
    'web_session_chunk_served',
    'web_session_download_cancelled',
  ]) {
    assert.ok(backend.includes(`"${event}"`), `backend is missing ${event}`);
  }
});
