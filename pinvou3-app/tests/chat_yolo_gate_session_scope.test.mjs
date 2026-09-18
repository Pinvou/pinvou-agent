#!/usr/bin/env node
// Regression pins for the YOLO gating path of regular-chat bound sessions:
// session identity must be re-checked after every await against
// activeSessionIdRef.current (latest rendered value) — the closure's
// activeSessionId is a render-time constant whose self-comparison is always
// true. Clicking YOLO on unbound A and switching to bound-but-unconfirmed B
// while the query is in flight would let B be flipped by exitPlanToYolo with
// no card.
// Precedents: chat_cancel_session_scope.test.mjs (post-await writes scoped per
// session source pattern) and right_dock_occlusion_gate.test.mjs (the
// activeSessionIdRef comparison pattern).
// （resolveBindingForGate / handleModeChipSwitch / confirmChatYoloSwitch）
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const chatPath = path.join(here, '..', 'src', 'features', 'chat', 'ChatView.jsx');
// Normalize CRLF so the multi-line anchors below behave the same on a Windows
// working tree (core.autocrlf=true) as in CI.
const src = fs.readFileSync(chatPath, 'utf8').replace(/\r\n/g, '\n');

// 1. resolveBindingForGate's post-await state write must be compared against the
//    latest ref value (comparison against the render-time constant is always
//    true, a dead guard). #464 round-6 finding 8a introduced a generation-guarded
//    cache write in the same block, so the state write is allowed to carry the
//    binding-sid bookkeeping; what stays pinned is the ref comparison itself.
assert.match(
  src,
  /if \(sid === activeSessionIdRef\.current\) \{\n\s*(?:bindingSidRef\.current = sid;\n\s*)?setSessionWorkspaceBinding\(normalized\);/,
  'binding state write after await must compare against activeSessionIdRef.current',
);
assert.doesNotMatch(
  src,
  /if \(sid === activeSessionId\) setSessionWorkspaceBinding/,
  'render-const comparison is a dead guard (always true in the closure)',
);

// 2. handleModeChipSwitch re-checks the latest ref value after the binding query await.
assert.match(
  src,
  /const sessionBinding = await resolveBindingForGate\(gateSid\);\s*\n[\s\S]*?if \(gateSid !== activeSessionIdRef\.current\) return;/,
  'gate must re-check the active session after the binding query await',
);

// 3. Re-check again after the prefs query await (the second await).
const rechecks = src.match(/if \(gateSid !== activeSessionIdRef\.current\) return;/g) || [];
assert.ok(
  rechecks.length >= 2,
  `both awaits (binding query + prefs read) must be followed by the ref re-check, found ${rechecks.length}`,
);

// 4. The confirm card re-checks the captured sid before exitPlanToYolo.
assert.match(
  src,
  /pendingChatYoloSwitchSidRef\.current !== activeSessionIdRef\.current/,
  'confirm must re-verify the captured sid before exitPlanToYolo',
);
assert.match(
  src,
  /pendingChatYoloSwitchSidRef\.current = gateSid;[\s\S]{0,200}?setPendingChatYoloSwitch\(true\);/,
  'opening the confirm card must capture the gate sid',
);

// 5. Switching sessions invalidates the old session's confirm card and error message.
assert.match(
  src,
  /setPendingChatYoloSwitch\(false\);\s*\n\s*setChatYoloConfirmError\(''\);\s*\n\s*\}, \[activeSessionId\]\);/,
  'session switch must reset the pending confirm card and its error',
);

// 6. The gating path must not retain render-time constant self-comparison (dead guard shape).
assert.doesNotMatch(
  src,
  /if \(gateSid !== activeSessionId\)/,
  'self-referential closure comparison must not come back',
);

// 7. Decision glue: the confirm card must open driven by the
//    needsYoloConfirmation(prefs) verdict, capturing the verdict sid when it
//    opens.
assert.match(
  src,
  /if \(needsYoloConfirmation\(prefs\)\) \{\s*\n\s*pendingChatYoloSwitchSidRef\.current = gateSid;/,
  'the confirm card must be driven by needsYoloConfirmation(prefs) and capture the gate sid',
);

// 8. Decision glue: the catch on binding query failure must fail closed and
//    return the unknown-binding sentinel (treat as bound, ask once too often),
//    never silently pass as unbound.
assert.match(
  src,
  /\} catch \{\s*\n(?:[^\n]*\n){1,6}?\s*return CHAT_YOLO_GATE_UNKNOWN_BINDING;/,
  'the binding-query catch must fail closed by returning CHAT_YOLO_GATE_UNKNOWN_BINDING',
);

// 9. The final action must target the verdict sid: activeSessionIdRef is a
//    render-time mirror that can lag the bridge store by a frame, which the
//    ref check cannot cover; the bridge's argument-less exitPlanToYolo reads
//    the live active at call time and would swap the target for the
//    post-switch session. The verdict sid must reach the command as a
//    parameter (bridge-side behavior pinned by
//    interaction_write_routing.test.mjs).
assert.match(
  src,
  /await bridge\.interaction\.exitPlanToYolo\(gateSid\);/,
  'handleModeChipSwitch must pass the adjudicated sid to exitPlanToYolo',
);
assert.match(
  src,
  /await bridge\.interaction\.exitPlanToYolo\(pendingChatYoloSwitchSidRef\.current\);/,
  'confirmChatYoloSwitch must pass the captured sid to exitPlanToYolo',
);
assert.doesNotMatch(
  src,
  /bridge\.interaction\.exitPlanToYolo\(\s*\)/,
  'the gate paths must not fall back to the live-active exitPlanToYolo() form (any call shape)',
);

console.log('chat_yolo_gate_session_scope: ok');

// ── R9 MAJOR 1: the plan-stuck card's Go must pass the same one-time gate ─────────
// The bridge's planStuckGo used to finish with an argument-less exitPlanToYolo():
// a bound regular session went from the plan_stuck card straight to Yolo,
// bypassing the confirm card entirely, and reading the live active at call time
// targeted the post-switch session. The ChatView source scan cannot cover the
// bridge layer, so both bridges' sources are scanned here as well.

const tauriInteractionPath = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge', 'interaction.js');
const webBridgePath = path.join(here, '..', 'src', 'platform', 'web', 'bridge.js');
const tauriInteraction = fs.readFileSync(tauriInteractionPath, 'utf8');
const webBridge = fs.readFileSync(webBridgePath, 'utf8');

// The bridge layer must not retain any argument-less exitPlanToYolo call (void or await).
for (const [label, bridgeSrc] of [['tauri interaction.js', tauriInteraction], ['web bridge.js', webBridge]]) {
  assert.doesNotMatch(
    bridgeSrc,
    /(?<!\w)exitPlanToYolo\(\s*\)/,
    `${label} must not call exitPlanToYolo without an explicit target session (#445 R9 MAJOR 1)`,
  );
}

// The bridge's planStuckGo must accept an explicit target session and pass the same sid to exitPlanToYolo.
for (const [label, bridgeSrc] of [['tauri interaction.js', tauriInteraction], ['web bridge.js', webBridge]]) {
  assert.match(
    bridgeSrc,
    /async function planStuckGo\(itemId, targetSessionId\)/,
    `${label} planStuckGo must accept the adjudicated session id`,
  );
  const body = bridgeSrc.slice(bridgeSrc.indexOf('async function planStuckGo'), bridgeSrc.indexOf('async function planStuckGo') + 1200);
  assert.match(
    body,
    /await exitPlanToYolo\(sid\)/,
    `${label} planStuckGo must target exitPlanToYolo at the adjudicated sid`,
  );
}

// ChatView side: the plan card's Go must go through the gated handler and record
// the planStuckGo action; after confirmation it dispatches the recorded action
// with the captured sid.
assert.match(
  src,
  /<PlanStuckCard item=\{item\} t=\{t\} onGo=\{onPlanStuckGo\} \/>/,
  'the plan-stuck card must render through the gate-handler prop',
);
assert.match(
  src,
  /onPlanStuckGo=\{handlePlanStuckGo\}/,
  'ChatBubble must receive the gate handler for the plan-stuck card',
);
assert.match(
  src,
  /pendingChatYoloActionRef\.current = \{ kind: 'planStuckGo', itemId \};/,
  'the Go path must record its pending action before adjudicating the gate',
);
assert.match(
  src,
  /bridge\.interaction\.planStuckGo\(action\.itemId, pendingChatYoloSwitchSidRef\.current\)/,
  'confirmation must dispatch planStuckGo with the captured sid',
);
assert.match(
  src,
  /useEffect\(\(\) => \{\s*\n\s*pendingChatYoloSwitchSidRef\.current = null;\s*\n\s*pendingChatYoloActionRef\.current = null;/,
  'session switch must clear the pending action',
);

console.log('chat_yolo_gate_session_scope: ok (incl. bridge-layer no-arg ban)');
