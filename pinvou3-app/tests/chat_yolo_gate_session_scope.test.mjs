#!/usr/bin/env node
// 评审 #445 round-7 的回归钉住：普通聊天绑定会话的 YOLO 门控路径
// （resolveBindingForGate / handleModeChipSwitch / confirmChatYoloSwitch）
// 必须在每个 await 之后用 activeSessionIdRef.current（最新渲染值）复核
// 会话身份——闭包里的 activeSessionId 是渲染期常量，自比较永远为真
// （round-5 的修复因此是死代码），未绑定 A 点击 YOLO、查询在飞时切到
// 已绑定未确认的 B，会让 B 无确认卡被 exitPlanToYolo 翻转。
// 先例：chat_cancel_session_scope.test.mjs（post-await 写入按会话作用域的
// 源码模式回归）与 right_dock_occlusion_gate.test.mjs（activeSessionIdRef
// 比对模式）。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const chatPath = path.join(here, '..', 'src', 'features', 'chat', 'ChatView.jsx');
const src = fs.readFileSync(chatPath, 'utf8');

// 1. resolveBindingForGate 的 post-await state 写入必须与 ref 最新值比对
//    （渲染期常量 activeSessionId 的比对永远为真，是死守卫）。
assert.match(
  src,
  /if \(sid === activeSessionIdRef\.current\) setSessionWorkspaceBinding\(normalized\);/,
  'binding state write after await must compare against activeSessionIdRef.current',
);
assert.doesNotMatch(
  src,
  /if \(sid === activeSessionId\) setSessionWorkspaceBinding/,
  'render-const comparison is a dead guard (always true in the closure)',
);

// 2. handleModeChipSwitch 在绑定查询 await 之后复核 ref 最新值。
assert.match(
  src,
  /const sessionBinding = await resolveBindingForGate\(gateSid\);\s*\n[\s\S]*?if \(gateSid !== activeSessionIdRef\.current\) return;/,
  'gate must re-check the active session after the binding query await',
);

// 3. prefs 查询 await（第二个 await）之后同样复核。
const rechecks = src.match(/if \(gateSid !== activeSessionIdRef\.current\) return;/g) || [];
assert.ok(
  rechecks.length >= 2,
  `both awaits (binding query + prefs read) must be followed by the ref re-check, found ${rechecks.length}`,
);

// 4. 确认卡在 exitPlanToYolo 前复核打开时捕获的 sid。
assert.match(
  src,
  /pendingChatYoloSwitchSidRef\.current !== activeSessionIdRef\.current/,
  'confirm must re-verify the captured sid before exitPlanToYolo',
);
assert.match(
  src,
  /pendingChatYoloSwitchSidRef\.current = gateSid;\s*\n\s*setPendingChatYoloSwitch\(true\);/,
  'opening the confirm card must capture the gate sid',
);

// 5. 切换会话即作废旧会话的确认卡与错误文案。
assert.match(
  src,
  /setPendingChatYoloSwitch\(false\);\s*\n\s*\/\/ eslint-disable-next-line react-hooks\/set-state-in-effect[^\n]*\n\s*setChatYoloConfirmError\(''\);\s*\n\s*\}, \[activeSessionId\]\);/,
  'session switch must reset the pending confirm card and its error',
);

// 6. 门控路径不得残留渲染期常量自比较（死守卫形态）。
assert.doesNotMatch(
  src,
  /if \(gateSid !== activeSessionId\)/,
  'self-referential closure comparison must not come back',
);

console.log('chat_yolo_gate_session_scope: ok');
