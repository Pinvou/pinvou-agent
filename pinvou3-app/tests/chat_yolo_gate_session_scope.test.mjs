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
  /await resolveBindingForGate\(gateSid\);[\s\S]{0,200}?if \(gateSid !== activeSessionIdRef\.current\) return false;/,
  'gate must re-check the active session after the binding query await (round-7 P2: the query result no longer feeds the adjudication, but the await and its staleness re-check stay)',
);

// 3. Re-check again after the prefs query await (the second await).
const rechecks = src.match(/if \(gateSid !== activeSessionIdRef\.current\) return;/g) || [];
assert.ok(
  rechecks.length >= 2,
  `both awaits (binding query + prefs read) must be followed by the ref re-check, found ${rechecks.length}`,
);

// 4. The confirm card owns its dialog identity: the post-await generation
//    check supersedes the old live-active sid re-read (a switch-away always
//    bumps the generation via the session-switch effect, so a superseded
//    confirm discards instead of consuming whichever refs are current —
//    round-8 R8-1).
assert.match(
  src,
  /await bridge\.interaction\.confirmCodeYolo\(\);[\s\S]{0,240}if \(pendingChatYoloDialogGenRef\.current !== genAtConfirm\) \{/,
  'confirm must re-verify its dialog generation after the save round-trip before any dispatch (round-8 R8-1; supersedes the old live-active sid re-read)',
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
  /if \(needsYoloConfirmation\(prefs\)\) \{\s*\n[^\n]*\n[^\n]*\n\s*pendingChatYoloDialogGenRef\.current \+= 1;\s*\n\s*pendingChatYoloSwitchSidRef\.current = gateSid;/,
  'the confirm card must be driven by needsYoloConfirmation(prefs), bump the dialog generation, and capture the gate sid (round-8 R8-1)',
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
  /await bridge\.interaction\.exitPlanToYolo\(sidAtConfirm\);/,
  'confirmChatYoloSwitch must pass the captured sid to exitPlanToYolo (captured before the await, round-8 R8-1)',
);
assert.doesNotMatch(
  src,
  /await bridge\.interaction\.exitPlanToYolo\(pendingChatYoloSwitchSidRef\.current\);/,
  'confirmChatYoloSwitch must not re-read the shared sid ref after the await (round-8 R8-1)',
);
assert.doesNotMatch(
  src,
  /bridge\.interaction\.exitPlanToYolo\(\s*\)/,
  'the gate paths must not fall back to the live-active exitPlanToYolo() form (any call shape)',
);

// 10. 评审 P2：Auto→Yolo 逃生在 Web 必须可达，而进入 Auto 必须仍被挡住。
//     Web 桥带 setSessionMode 后，存在性守卫不再区分表面：进入 Auto 若只
//     靠守卫，浏览器用户会把会话切进一个自己无法发送（webAutoSendBlocked）
//     也无法决策审批卡（decideApproval 不在 Web 白名单）的档位。
//     Auto→Yolo 走同一命令、在 Web 可达；进入 Auto 显式挡 Web。
const autoEntry = src.slice(
  src.indexOf('if (target === \'auto\') {'),
  src.indexOf("if (target !== 'yolo') return;"),
);
assert.match(
  autoEntry,
  /if \(isWeb \|\| typeof bridge\.interaction\.setSessionMode !== 'function'\) return;/,
  'entering Auto must stay blocked on the web surface (approval cards have no web decision face)',
);
assert.match(
  src,
  /await bridge\.interaction\.setSessionMode\(gateSid, 'yolo'\)/,
  'Auto→Yolo must route through the unified set_session_mode with the adjudicated sid',
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
  /chatYoloGateAllows\(gateSid, \{ kind: 'planStuckGo', itemId \}\)/,
  'the Go path must pass its pending action explicitly to the gate (no preset/default-fill race)',
);
assert.match(
  src,
  /bridge\.interaction\.planStuckGo\(actionAtConfirm\.itemId, sidAtConfirm\)/,
  'confirmation must dispatch planStuckGo with the pre-await captured sid (round-8 R8-1)',
);
assert.match(
  src,
  /useEffect\(\(\) => \{\s*\n(?:[^\n]*\n){0,3}?\s*pendingChatYoloDialogGenRef\.current \+= 1;\s*\n\s*pendingChatYoloSwitchSidRef\.current = null;\s*\n\s*pendingChatYoloActionRef\.current = null;/,
  'session switch must clear the pending action',
);

// ── Review P2: canceling the confirm must dispose the pending action ─────────
// The cancel callback used to only close the dialog, leaving the captured
// pendingChatYoloActionRef (e.g. { kind: 'planStuckGo', itemId }) in place.
// Within the same session the gate later fills the action only when it is
// empty (`if (!pendingChatYoloActionRef.current)`), so a subsequent
// mode-only Yolo entry re-opened the card on top of the stale action and
// confirming executed the CANCELED continuation: the plan card got marked
// handled and its continue command sent even though the user had canceled.
// Same-session scope: the session-switch effect cannot be relied on (it only
// fires on activeSessionId changes). Negative-first: on the unfixed source
// this match fails (onCancel closes the dialog only); the fresh-confirm pins
// above double as the negative control that a non-canceled Yolo confirm still
// dispatches its own action.
assert.match(
  src,
  /onCancel=\{\(\) => \{\n(?:[^\n]*\n){0,12}?\s*pendingChatYoloSwitchSidRef\.current = null;\s*\n\s*pendingChatYoloActionRef\.current = null;\s*\n\s*setPendingChatYoloSwitch\(false\);\s*\n\s*\}\}/,
  'canceling the YOLO confirm must dispose the pending action (not just close the dialog)',
);
assert.doesNotMatch(
  src,
  /onCancel=\{\(\) => setPendingChatYoloSwitch\(false\)\}/,
  'the cancel-only dialog close must not come back (stale pending action rider)',
);

// ── 评审 P2：Web 面的 Auto→Yolo 逃生 ──────────────────────────────────────
// Web 桥此前缺 setSessionMode，ChatView 的存在性守卫让 Auto 会话的 Yolo
// 动作在 Web 静默无操作（发送门把浏览器用户锁死在 Auto 档）。运行时行为
// 由 web_bridge_domain_contract.test.mjs 钉；这里钉两侧源码契约：
// 12. Web 桥的 setSessionMode 必须镜像桌面契约：entry 捕获 sid（显式裁决
//     sid 优先）、统一 set_session_mode invoke、权威 modeState 写回、无
//     sid 纯 no-op（composer-controls 的兜底 setSessionMode(null,'auto')
//     在 Web 不得变成可达的进入-Auto 写入）。
const webSetSessionMode = webBridge.slice(
  webBridge.indexOf('async function setSessionMode'),
  webBridge.indexOf('async function setSessionMode') + 1200,
);
assert.match(
  webSetSessionMode,
  /await invoke\("set_session_mode", args\)/,
  'web setSessionMode must invoke the unified set_session_mode command',
);
assert.match(
  webSetSessionMode,
  /applyAuthoritativeModeState\(sid, st\)/,
  'web setSessionMode must converge through the authoritative modeState write-back',
);
assert.match(
  webSetSessionMode.slice(0, webSetSessionMode.indexOf('try {')),
  /if \(!sid\) return;/,
  'web setSessionMode must no-op without a session id before any invoke',
);
// 13. ChatView：进入 Auto 仍显式挡 Web（Web 门面不导出 decideApproval——
//     审批卡无浏览器决策面；注意决定命令本身在 access-policy 白名单是前向
//     兼容登记，挡的是门面导出），Auto→Yolo 走同一命令、在 Web 可达。
const webAutoEntry = src.slice(
  src.indexOf("if (target === 'auto') {"),
  src.indexOf("if (target !== 'yolo') return;"),
);
assert.match(
  webAutoEntry,
  /if \(isWeb \|\| typeof bridge\.interaction\.setSessionMode !== 'function'\) return;/,
  'entering Auto must stay blocked on the web surface (approval cards have no web decision face)',
);
assert.match(
  src,
  /await bridge\.interaction\.setSessionMode\(gateSid, 'yolo'\)/,
  'Auto→Yolo must route through the unified set_session_mode with the adjudicated sid',
);

// ── Round-8 R8-1: the async confirm must own its dialog identity ────────────
// The save round-trip (`await confirmCodeYolo()`) is the window where the
// user can switch sessions, cancel, or open another confirm. The unfixed body
// re-read the shared mutable refs AFTER the await; a superseded confirmation
// then passed the live-active guard on the NEW session's state and dispatched
// the newer dialog's not-yet-confirmed planStuckGo. The fix captures
// (generation, sid, action) before the await, bumps the generation at every
// invalidation point (gate open / cancel / session switch), and discards a
// superseded outcome without touching the shared refs, flag, or error state.
// Negative-first: on the unfixed source the capture pin and the no-reread pin
// both fail.
assert.match(
  src,
  /const genAtConfirm = pendingChatYoloDialogGenRef\.current;\s*\n\s*const sidAtConfirm = pendingChatYoloSwitchSidRef\.current;\s*\n\s*const actionAtConfirm = pendingChatYoloActionRef\.current;\s*\n\s*try \{\s*\n\s*await bridge\.interaction\.confirmCodeYolo\(\);/,
  'the confirm must capture (gen, sid, action) before its save round-trip',
);
assert.doesNotMatch(
  src,
  /const action = pendingChatYoloActionRef\.current;/,
  'the confirm must not re-read the shared action ref after the await (capture before the await instead)',
);
assert.match(
  src,
  /await bridge\.interaction\.confirmCodeYolo\(\);[\s\S]{0,240}if \(pendingChatYoloDialogGenRef\.current !== genAtConfirm\) \{[\s\S]{0,420}?return;/,
  'a superseded confirm outcome must be discarded on the post-await generation check',
);
assert.match(
  src,
  /if \(needsYoloConfirmation\(prefs\)\) \{[\s\S]{0,240}pendingChatYoloDialogGenRef\.current \+= 1;/,
  'gate open must bump the dialog generation (a new dialog owns the shared refs)',
);
assert.match(
  src,
  /onCancel=\{\(\) => \{[\s\S]{0,900}?pendingChatYoloDialogGenRef\.current \+= 1;[\s\S]{0,200}?pendingChatYoloSwitchSidRef\.current = null;[\s\S]{0,200}?pendingChatYoloActionRef\.current = null;/,
  'cancel must bump the generation and dispose the captured action',
);
assert.match(
  src,
  /useEffect\(\(\) => \{\s*\n(?:[^\n]*\n){0,4}?[^\n]*R8-1[^\n]*\n\s*pendingChatYoloDialogGenRef\.current \+= 1;\s*\n\s*pendingChatYoloSwitchSidRef\.current = null;/,
  'the session-switch effect must bump the generation before nulling the refs',
);
// Dispatch-origin discipline: the confirm dispatches the SAME command family
// the click came from (Plan→Yolo exit / Auto→Yolo set_session_mode / draft
// lane default) using the CAPTURED values, never the post-await refs.
// Round-9 review fix: the action is an explicit gate parameter assigned
// unconditionally — the old caller-side preset + `if (!ref.current)`
// default-fill raced a gen-matched in-flight confirm whose success path
// nulled the ref between preset and fill, arming the NEW dialog with the
// bare `{ kind: 'modeChip' }` default (no `via`) whose confirm then fell
// into the exitPlanToYolo branch.
assert.match(
  src,
  /async function chatYoloGateAllows\(gateSid, action\)/,
  'the gate must take the continue-action as an explicit parameter',
);
assert.match(
  src,
  /pendingChatYoloSwitchSidRef\.current = gateSid;[\s\S]{0,200}?pendingChatYoloActionRef\.current = action;\s*\n\s*setPendingChatYoloSwitch\(true\);/,
  'gate open must assign the caller-passed action unconditionally before opening the card',
);
assert.doesNotMatch(
  src,
  /if \(!pendingChatYoloActionRef\.current\)/,
  'the default-fill must not come back (a nulled ref mid-race armed the new dialog with the bare default)',
);
assert.match(
  src,
  /chatYoloGateAllows\(gateSid, \{ kind: 'modeChip', via: isPlan \? 'exitPlanToYolo' : 'sessionMode' \}\)/,
  'the chip path must pass its dispatch origin explicitly to the gate',
);
assert.match(
  src,
  /chatYoloGateAllows\(null, \{ kind: 'modeChip', via: 'draftMode' \}\)/,
  'the unbound draft path must pass the draft dispatch origin explicitly to the gate',
);
assert.match(
  src,
  /else if \(actionAtConfirm && actionAtConfirm\.via === 'sessionMode'\) \{\s*\n\s*await bridge\.interaction\.setSessionMode\(sidAtConfirm, 'yolo'\);/,
  'the confirm must honor the captured origin (session-mode entries via set_session_mode)',
);
assert.match(
  src,
  /else if \(actionAtConfirm && actionAtConfirm\.via === 'draftMode'\) \{\s*\n\s*await bridge\.interaction\.setDraftMode\('yolo'\);/,
  'the confirm must honor the captured origin (draft entries via setDraftMode)',
);
assert.match(
  src,
  /if \(pendingChatYoloDialogGenRef\.current === genAtConfirm\) \{\s*\n\s*setChatYoloConfirmError\(/,
  'the failure path must gate its error write on the generation (no painting a newer dialog)',
);

// ── round-10: malformed/missing-mode display fallback must stay "auto" ─────────
// The widest mode is the worst-direction lie: a missing or malformed `mode`
// on an authoritative payload must display the conservative lane default,
// never Yolo (round-8 L7-4 adopted this on the tauri lane; the round-10
// review caught the web lane still carrying "yolo" fallbacks and three test
// mocks encoding the pre-fix shape). These pins make a silent revert red in
// whichever lane it lands. (Scheduled-task run-as-Yolo wire inputs and the
// explicit draft `setDraftMode('yolo')` route are intentional Yolo literals
// and are deliberately NOT banned.)
const bridgeDir = path.join(here, '..', 'src', 'platform');
const fpTauriBridge = fs.readFileSync(path.join(bridgeDir, 'tauri', 'bridge.js'), 'utf8').replace(/\r\n/g, '\n');
const fpTauriInteraction = fs.readFileSync(path.join(bridgeDir, 'tauri', 'bridge', 'interaction.js'), 'utf8').replace(/\r\n/g, '\n');
const fpWebBridge = fs.readFileSync(path.join(bridgeDir, 'web', 'bridge.js'), 'utf8').replace(/\r\n/g, '\n');
const fpSharedHelpers = fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8').replace(/\r\n/g, '\n');

// tauri: authoritative convergence + read fallbacks.
assert.match(fpTauriBridge, /mode: st\.mode \|\| "auto"/,
  'tauri authoritative fallback must display auto for a missing mode');
assert.doesNotMatch(fpTauriBridge, /st\.mode \|\| "yolo"/,
  'tauri authoritative fallback must never fall back to yolo');
assert.match(fpTauriBridge, /modeState: \{ mode: "auto" \}/,
  'tauri freshBuffer must initialize the generic default as auto');
assert.match(fpTauriInteraction, /mode: ms\.mode \|\| "auto"/,
  'tauri interaction read fallback must display auto');
assert.doesNotMatch(fpTauriInteraction, /ms\.mode \|\| "yolo"/,
  'tauri interaction read fallback must never fall back to yolo');
assert.match(fpTauriInteraction, /\{ mode: "auto", multiAgent: false, askOnGuardrail: true \}/,
  'tauri interaction absence fallback must be the auto shape');

// web: bootstrap restore, syncModeState catch, mapModeState, authoritative convergence.
assert.match(fpWebBridge, /: \{ mode: "auto", multiAgent: false, askOnGuardrail: true \}/,
  'web bootstrap absence fallback must be the auto shape');
assert.match(fpWebBridge, /mode: \(ms && ms\.mode\) \|\| "auto"/,
  'web mapModeState fallback must display auto');
assert.match(fpWebBridge, /mode: st\.mode \|\| "auto"/,
  'web authoritative convergence fallback must display auto');
assert.doesNotMatch(fpWebBridge, /\(ms && ms\.mode\) \|\| "yolo"/,
  'web mapModeState fallback must never fall back to yolo');
assert.doesNotMatch(fpWebBridge, /st\.mode \|\| "yolo"/,
  'web authoritative convergence fallback must never fall back to yolo');

// shared: freshBuffer generic default is auto (the draft override remains
// load-bearing — lane defaults may legitimately be plan/yolo).
assert.match(fpSharedHelpers, /modeState: \{ mode: "auto" \}/,
  'shared freshBuffer generic default must be auto');

console.log('chat_yolo_gate_session_scope: ok (incl. bridge-layer no-arg ban + fallback-posture pins)');

