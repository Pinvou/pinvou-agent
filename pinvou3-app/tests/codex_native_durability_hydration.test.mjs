#!/usr/bin/env node
// Focused regression for round-32 m3: hydration preserve covers durability
// notices. Test name: codex native lane hydration preserves durability notices
// (round-32). Harness mirrors code_native_lane.test.mjs (copy the module into a
// temporary type:module directory, then import).
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-codex-native-durability-'));
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
mkdirSync(path.join(temp, 'features', 'conversation'), { recursive: true });
mkdirSync(path.join(temp, 'features', 'codex'), { recursive: true });
mkdirSync(path.join(temp, 'features', 'multiagent'), { recursive: true });
mkdirSync(path.join(temp, 'shared'), { recursive: true });
for (const file of ['conversation-model.js', 'deepseek-conversation.js']) {
  copyFileSync(path.join(root, 'src', 'features', 'conversation', file), path.join(temp, 'features', 'conversation', file));
}
copyFileSync(path.join(root, 'src', 'features', 'codex', 'code-native-lane.js'), path.join(temp, 'features', 'codex', 'code-native-lane.js'));
copyFileSync(path.join(root, 'src', 'features', 'multiagent', 'spawn-aggregation.mjs'), path.join(temp, 'features', 'multiagent', 'spawn-aggregation.mjs'));
copyFileSync(path.join(root, 'src', 'shared', 'internal-message.mjs'), path.join(temp, 'shared', 'internal-message.mjs'));
copyFileSync(path.join(root, 'src', 'shared', 'shell-tools.mjs'), path.join(temp, 'shared', 'shell-tools.mjs'));
copyFileSync(path.join(root, 'src', 'shared', 'path-utils.js'), path.join(temp, 'shared', 'path-utils.js'));

try {
  const {
    applyNativeChatEvent,
    appendNativeSystemItem,
    createNativeLane,
    hydrateNativeLane,
  } = await import(`${pathToFileURL(path.join(temp, 'features', 'codex', 'code-native-lane.js')).href}?t=${Date.now()}`);

  // codex native lane hydration preserves durability notices (round-32):
  // finalizePreparedSessionCreation appends the mode persist-error note into the
  // native lane and then immediately activates the session, whose loadSession
  // hydrateNativeLane wipes live-only system items. The preserve filter keeps
  // host-visible audit records (toolGateDecision / guardrailDenied, round-9) and
  // now also durabilityNotice facts; a plain system item stays live-only.
  const lane = createNativeLane();
  appendNativeSystemItem(lane, 'plain transient notice');
  applyNativeChatEvent(lane, 'chat:guardrail_denied', {
    session_id: 's-dur',
    tool_name: 'write_file',
    reason: 'guardrail_ask_off',
  });
  appendNativeSystemItem(lane, 'mode persist failed: disk error', { durabilityNotice: true });

  assert.equal(lane.items.length, 3);
  assert.equal(lane.items.filter(item => item.type === 'system').length, 3);

  hydrateNativeLane(lane, { messages: [] }, []);

  assert.equal(lane.items.length, 2,
    'hydration keeps exactly the two preserved audit items and drops the plain system item');
  assert.equal(lane.items.some(item => item.type === 'system' && !item.guardrailDenied && !item.durabilityNotice), false,
    'the plain live-only system item must not survive hydration');
  const guardrailSurvivor = lane.items.find(item => item.guardrailDenied);
  assert.ok(guardrailSurvivor, 'guardrail hard-deny audit survives hydration (round-9 preserve)');
  assert.equal(guardrailSurvivor.toolName, 'write_file');
  assert.equal(guardrailSurvivor.reason, 'guardrail_ask_off');
  const durabilitySurvivor = lane.items.find(item => item.durabilityNotice);
  assert.ok(durabilitySurvivor, 'durability notice survives hydration (round-32 preserve)');
  assert.equal(durabilitySurvivor.text, 'mode persist failed: disk error');
  assert.equal(durabilitySurvivor.type, 'system');

  // Same-kind dedup stays defensive and non-collapsing: re-appending an identical
  // durability notice then re-hydrating must not double-insert the survivor
  // (dedup by text), and an unrelated durability notice must survive alongside it.
  appendNativeSystemItem(lane, 'mode persist failed: disk error', { durabilityNotice: true });
  appendNativeSystemItem(lane, 'ask persist failed: other error', { durabilityNotice: true });
  hydrateNativeLane(lane, { messages: [] }, []);
  const durabilitySurvivors = lane.items.filter(item => item.durabilityNotice);
  assert.equal(durabilitySurvivors.length, 2,
    'same-text durability notice dedups to one; a distinct notice survives alongside');
  assert.ok(lane.items.some(item => item.guardrailDenied), 'guardrail audit survives the second rehydrate too');

  console.log('codex native lane hydration preserves durability notices (round-32): ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
