#!/usr/bin/env node
// 评论 9/12（PR #207 reviewer 点 9 + 协作者评论 12）的回归：ChatView 的取消中
// 状态必须按 session 记录（cancellingSessionIds 集合），而不是全局 single-flight
// 布尔或单个 sid。ChatView 在切换 active session 时不 remount，全局布尔会让
// 「取消会话 A 期间切到仍在运行的会话 B」时 B 的停止按钮保持禁用、handleCancel
// 早退丢弃 B 的取消，直到 A 的 invoke 返回（后端通道背压时可能较长）。单个 sid
// 则无法表示多个会话同时处于取消中：A 发起取消 → 切到 B 发起取消 → 切回 A 时
// state 已被 B 覆盖，A 的按钮重新启用、可重复触发取消。必须用按 sid 的 Set
// 记录，并在各自 Promise 完成时只删除对应 sid。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const chatPath = path.join(here, '..', 'src', 'features', 'chat', 'ChatView.jsx');
const chatSource = fs.readFileSync(chatPath, 'utf8');

// 1. 取消中状态按 session 集合记录：state 是 cancellingSessionIds（Set），
//    而非全局布尔或单个 sid。
assert.match(
  chatSource,
  /const \[cancellingSessionIds, setCancellingSessionIds\] = useState\(\(\) => new Set\(\)\);/,
  'cancelling state must be a per-session id set, not a global boolean or single id',
);

// 2. handleCancel 的 single-flight 早退按 session 判断：只有当前 session 已在
//    取消中才忽略点击；切到别的会话后不再被旧会话的取消挡住。
assert.match(
  chatSource,
  /if \(!bridge\.available \|\| cancellingSessionIds\.has\(activeSessionId\)\) return;/,
  'handleCancel must early-return only when the same session is already cancelling',
);

// 3. 取消标记在 invoke 期间只禁用当前 session 的按钮（按集合成员判断）。
assert.match(
  chatSource,
  /disabled=\{cancellingSessionIds\.has\(activeSessionId\)\}/,
  'stop button must be disabled only for the session being cancelled',
);

// 4. 发起取消时把 sid 加入集合（不可变更新，保留其他会话的标记）。
assert.match(
  chatSource,
  /setCancellingSessionIds\(prev => new Set\(prev\)\.add\(cancellingSid\)\);/,
  'starting a cancel must add the sid to the set without dropping other sids',
);

// 5. finally 清理按发起时快照只删自己的 sid：切换 session 后旧 invoke 返回
//    不会误清其他会话正在进行的取消标记（函数式更新 + 快照比较）。
assert.match(
  chatSource,
  /setCancellingSessionIds\(prev => \{[\s\S]{0,200}next\.delete\(cancellingSid\);[\s\S]{0,120}return next;\s*\}\);/,
  'finally must clear only the cancelling flag armed for its own session',
);

// 6. 旧全局布尔 / 单个 sid 不应残留（防回归到旧实现）。
assert.ok(
  !/useState\(false\);[\s\S]{0,400}handleCancel/.test(chatSource) ||
    !/const \[cancelling, setCancelling\] = useState\(false\)/.test(chatSource),
  'no global cancelling boolean state may remain in ChatView',
);
assert.ok(
  !/cancellingSessionId === activeSessionId/.test(chatSource),
  'no single-session cancelling comparison may remain in ChatView',
);

// 7. 提取 handleCancel 函数体做行为验证：同一 session 双击被忽略，切 session
//    后允许新取消（用文本级模拟，避免渲染 JSX）。
const start = chatSource.indexOf('async function handleCancel()');
assert.notStrictEqual(start, -1, 'handleCancel must exist');
const end = chatSource.indexOf('\n      }', start);
const handleCancelBody = chatSource.slice(start, end);
assert.match(handleCancelBody, /cancellingSessionIds\.has\(activeSessionId\)/);
assert.match(handleCancelBody, /setCancellingSessionIds\(prev => new Set\(prev\)\.add\(cancellingSid\)\)/);
assert.match(handleCancelBody, /await bridge\.chat\.cancelGeneration\(\)/);
assert.match(handleCancelBody, /next\.delete\(cancellingSid\)/);

// 8. A→B→A 序列行为验证：单个 sid 会被 B 覆盖导致 A 的按钮误启用；Set 语义
//    下 A 的标记在 B 取消期间保留，只有 A 自己的 Promise 完成才清除。
//    用与组件相同的不可变更新语义模拟。
{
  let cancelling = new Set();
  const setCancelling = (updater) => { cancelling = updater(cancelling); };
  const activeSessionId = 'A';

  // A 发起取消：加入集合，按钮应禁用。
  setCancelling(prev => new Set(prev).add('A'));
  assert.ok(cancelling.has('A'), 'A cancelling after its invoke starts');
  assert.ok(cancelling.has(activeSessionId), 'A button must be disabled while A is cancelling');

  // 切到 B 并发起取消：B 加入集合，A 的标记必须保留。
  setCancelling(prev => new Set(prev).add('B'));
  assert.ok(cancelling.has('A') && cancelling.has('B'), 'A and B may cancel concurrently');

  // 切回 A：A 仍在集合中，按钮必须保持禁用（不能被 B 的标记覆盖而误启用）。
  assert.ok(cancelling.has(activeSessionId), 'switching back to A must keep A disabled');

  // A 的 invoke 返回：只删 A，B 的标记不受影响。
  setCancelling(prev => {
    if (!prev.has('A')) return prev;
    const next = new Set(prev);
    next.delete('A');
    return next;
  });
  assert.ok(!cancelling.has('A'), 'A cleared when its own invoke resolves');
  assert.ok(cancelling.has('B'), 'B cancelling flag must survive A completion');
}

console.log('ok: cancel state is scoped per session and survives concurrent cancels (reviewer points 9/12)');
