// Round-34 R2: production-wiring pins for the codex picker-request
// lifecycle — the round-11 m8 / round-32 MAJOR 1 stale-replay family is
// guarded by hand-built models only; the production application effect and
// the three host clear paths had no pin of any class (grep-verified in the
// round-34 review). Source-shape pins on the real files, the technique the
// sibling wiring tests use.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...segments) => fs.readFileSync(path.join(root, ...segments), 'utf8');

test('the view applies the consumed request through beginDraft + setDraftProjectBinding', () => {
  const view = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const effect = view.indexOf('consumePickerRequest(workspacePickerRequest');
  assert.ok(effect > 0, 'the consumption effect must exist');
  const window = view.slice(effect, effect + 900);
  const draft = window.indexOf('beginDraft(');
  const binding = window.indexOf('setDraftProjectBinding(');
  assert.ok(draft > 0, 'the request path is applied via beginDraft');
  assert.ok(binding > draft, 'the project binding is staged after the draft');
  assert.match(
    window.slice(binding, binding + 300),
    /projectId \? \{ projectId, roots/,
    'the binding carries {projectId, roots} from the request payload',
  );
});

test('the host clears the request on all three invalidation paths', () => {
  const main = read('src', 'app', 'main.jsx');
  // 1) leaving the codex view (round-32 MAJOR 1's fix)
  const leave = main.indexOf("if (currentView !== 'codex') setPickerCodexRequest(null);");
  assert.ok(leave > 0, 'the leave-view clear must exist');
  // 2) picker dismissal
  const dismiss = main.indexOf('closeWorkspacePicker(); setPickerCodexRequest(null);');
  assert.ok(dismiss > 0, 'the dismissal clear must exist');
  // 3) a chat-lane stage invalidating a lingering codex request
  const count = main.split('setPickerCodexRequest(null);').length - 1;
  assert.ok(count >= 3, `all three clear sites must exist (found ${count})`);
});

test('the view acknowledges consumption so a remount cannot replay', () => {
  const view = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const effect = view.indexOf('consumePickerRequest(workspacePickerRequest');
  const window = view.slice(effect, effect + 900);
  assert.match(
    window,
    /onWorkspacePickerRequestConsumed\(\)/,
    'the consumed-ack must fire in the same effect as the application',
  );
});
