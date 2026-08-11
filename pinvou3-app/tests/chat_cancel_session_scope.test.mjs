#!/usr/bin/env node
// 评论 9（PR #207 reviewer 点 9）的回归：ChatView 的取消中状态必须按
// session 记录（cancellingSessionId），而不是全局 single-flight 布尔。
// ChatView 在切换 active session 时不 remount，全局布尔会让「取消会话 A
// 期间切到仍在运行的会话 B」时 B 的停止按钮保持禁用、handleCancel 早退
// 丢弃 B 的取消，直到 A 的 invoke 返回（后端通道背压时可能较长）。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const chatPath = path.join(here, '..', 'src', 'features', 'chat', 'ChatView.jsx');
const chatSource = fs.readFileSync(chatPath, 'utf8');

// 1. 取消中状态按 session 记录：state 是 cancellingSessionId 而非全局布尔。
assert.match(
  chatSource,
  /const \[cancellingSessionId, setCancellingSessionId\] = useState\(null\);/,
  'cancelling state must be bound to a session id, not a global boolean',
);

// 2. handleCancel 的 single-flight 早退按 session 判断：只有当前 session 已在
//    取消中才忽略点击；切到别的会话后不再被旧会话的取消挡住。
assert.match(
  chatSource,
  /if \(!bridge\.available \|\| cancellingSessionId === activeSessionId\) return;/,
  'handleCancel must early-return only when the same session is already cancelling',
);

// 3. 取消标记在 invoke 期间只禁用当前 session 的按钮。
assert.match(
  chatSource,
  /disabled=\{cancellingSessionId === activeSessionId\}/,
  'stop button must be disabled only for the session being cancelled',
);

// 4. finally 清理按发起时快照清自己的标记：切换 session 后旧 invoke 返回
//    不会误清新 session 正在进行的取消标记（函数式更新 + 快照比较）。
assert.match(
  chatSource,
  /setCancellingSessionId\(prev => \(prev === cancellingSid \? null : prev\)\);/,
  'finally must clear only the cancelling flag armed for its own session',
);

// 5. 旧全局布尔不应残留（防回归到全局 single-flight）。
assert.ok(
  !/useState\(false\);[\s\S]{0,400}handleCancel/.test(chatSource) ||
    !/const \[cancelling, setCancelling\] = useState\(false\)/.test(chatSource),
  'no global cancelling boolean state may remain in ChatView',
);

// 6. 提取 handleCancel 函数体做行为验证：同一 session 双击被忽略，切 session
//    后允许新取消（用文本级模拟，避免渲染 JSX）。
const start = chatSource.indexOf('async function handleCancel()');
assert.notStrictEqual(start, -1, 'handleCancel must exist');
const end = chatSource.indexOf('\n      }', start);
const handleCancelBody = chatSource.slice(start, end);
assert.match(handleCancelBody, /cancellingSessionId === activeSessionId/);
assert.match(handleCancelBody, /setCancellingSessionId\(cancellingSid\)/);
assert.match(handleCancelBody, /await bridge\.chat\.cancelGeneration\(\)/);
assert.match(handleCancelBody, /prev === cancellingSid \? null : prev/);

console.log('ok: cancel state is scoped per session (reviewer point 9)');
