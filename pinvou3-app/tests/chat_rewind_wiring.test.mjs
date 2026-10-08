// 工作模式（原生 chat 车道）回退接线契约：静态断言 ChatView 挂上了共享回退编排
// （renderBeforeTurn 入口 + 确认/撤销弹窗 + scopeSelection 变体），code 车道
// （CodexAcpView）改用共享模块且弹窗维持无双层选择项，共享层自 codex 迁入后
// 旧位置不再存在。纯静态源断言（同 codex_checkpoints_logic 的 web 策略锚定法，
// 不挂 DOM）。
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js';

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');
const chatView = source('features/chat/ChatView.jsx');
const codexView = source('features/codex/CodexAcpView.jsx');
const controller = source('features/conversation/useRewindController.js');
const rewindChip = source('features/conversation/RewindChip.jsx');

// ── ChatView 接线：共享模块 + 时间线入口 + 弹窗 ──────────────────────
// 共享模块导入（搬家后唯一来源是 features/conversation）。
assert.match(chatView, /from '\.\.\/conversation\/useRewindController\.js'/, 'ChatView 必须使用共享编排 hook');
assert.match(chatView, /from '\.\.\/conversation\/RewindChip\.jsx'/, 'ChatView 必须导入共享回退组件');
assert.match(chatView, /from '\.\.\/conversation\/checkpoints\.js'/, 'ChatView 必须导入共享纯逻辑（rewindUndoAvailable）');

// 时间线 turn 边界入口：ConversationTimeline 传 renderBeforeTurn 且渲染 RewindChip。
assert.match(chatView, /renderBeforeTurn=\{turn =>/, 'ChatView 必须向 ConversationTimeline 传 renderBeforeTurn');
assert.match(chatView, /<RewindChip\s/, 'renderBeforeTurn 内必须渲染 RewindChip');
// 「撤销回退」入口与两个弹窗挂载。
assert.match(chatView, /<RewindUndoChip\s/, 'ChatView 必须渲染「撤销回退」入口');
assert.match(chatView, /<RewindConfirmDialog\s/, 'ChatView 必须挂载回退确认弹窗');
assert.match(chatView, /<RewindUndoConfirmDialog\s/, 'ChatView 必须挂载撤销确认弹窗');

// 工作模式特有：确认弹窗启用「回退范围」选择变体（默认勾选双层回退，见
// RewindChip 的 useState(true)）；code 车道不传（见下）。
assert.match(chatView, /^\s+scopeSelection$/m, 'ChatView 的确认弹窗必须启用 scopeSelection 变体');

// 车道门：定时相关会话（sched- 运行上下文/定时任务创建向导）不渲染入口——与
// 后端「定时会话不产生快照、命令拒绝」同口径（B2 的 UI 侧）。
assert.match(
  chatView,
  /Boolean\(activeSessionId\) && !isScheduledTaskCreationChat && !scheduledRunContext/,
  '工作模式回退车道门必须排除定时相关会话',
);

// ChatView 不得直接调回退命令：编排（含忙碌单 flight/重载/归属检查）在共享
// hook 内，视图重复编排就是回归。
assert.doesNotMatch(chatView, /invokeTauri\('rewind_to_turn'/, 'ChatView 不得绕过共享 hook 直接调 rewind_to_turn');
assert.doesNotMatch(chatView, /invokeTauri\('undo_last_rewind'/, 'ChatView 不得绕过共享 hook 直接调 undo_last_rewind');

// ── 共享编排 hook：范围选择 → conversation_only 映射 ─────────────────
// 取消勾选「同时回退工作区文件」→ conversationOnly=true（后端既有语义，零改动）；
// 无快照边界（entry.conversationOnly）恒为仅对话。
assert.match(
  controller,
  /choice && choice\.includeWorkspace === false/,
  '共享 hook 必须把 includeWorkspace:false 识别为显式仅对话回退',
);
assert.match(
  controller,
  /conversationOnly: target\.conversationOnly \|\| conversationOnlyRequested/,
  '确认请求的 conversationOnly 必须合并入口变体与用户选择',
);

// 工作范围弹窗变体：默认勾选（推荐值）+ 无快照边界不渲染选择项（不承诺后端
// 给不了的文件恢复）。
assert.match(rewindChip, /useState\(true\)/, '回退范围选择默认勾选（双层回退为推荐值）');
assert.match(
  rewindChip,
  /scopeChoiceable = scopeSelection && !entry\.conversationOnly/,
  '无快照边界不得呈现回退范围选择项',
);

// ── code 车道：共享模块复用 + 弹窗维持原样 ────────────────────────────
assert.match(codexView, /from '\.\.\/conversation\/useRewindController\.js'/, 'CodexAcpView 必须复用共享编排 hook');
assert.match(codexView, /from '\.\.\/conversation\/RewindChip\.jsx'/, 'CodexAcpView 必须改用共享回退组件');
assert.match(codexView, /from '\.\.\/conversation\/checkpoints\.js'/, 'CodexAcpView 必须改用共享纯逻辑');
// code 车道确认弹窗无「回退范围」选择项（其契约为「改动随对话回退」，双层是
// 默认）——scopeSelection 只出现在 ChatView。
{
  const codexDialogs = codexView.match(/<RewindConfirmDialog[\s\S]{0,600}?\/>/);
  assert.ok(codexDialogs, 'CodexAcpView 必须挂载回退确认弹窗');
  assert.doesNotMatch(codexDialogs[0], /scopeSelection/, 'code 车道确认弹窗不得有回退范围选择项');
}

// 搬家完成：codex 下不再有旧副本（防止双源漂移）。
assert.equal(
  existsSync(new URL('../src/features/codex/checkpoints.js', import.meta.url)),
  false,
  'features/codex/checkpoints.js 必须迁往 features/conversation（不得留副本）',
);
assert.equal(
  existsSync(new URL('../src/features/codex/RewindChip.jsx', import.meta.url)),
  false,
  'features/codex/RewindChip.jsx 必须迁往 features/conversation（不得留副本）',
);

// ── i18n：回退范围选择项三语齐备（键集一致性由 ui_language_coverage 钉住，
// 这里锚定新键的存在与中性文案方向）──────────────────────────────────
for (const language of ['zh', 'en', 'ja']) {
  const group = dict[language].uiCodex;
  assert.ok(group, `uiCodex 字典必须存在于 ${language}`);
  assert.equal(typeof group.rewindScopeWorkspace, 'string', `rewindScopeWorkspace 必须有 ${language} 文案`);
  assert.equal(typeof group.rewindScopeWorkspaceOffNote, 'string', `rewindScopeWorkspaceOffNote 必须有 ${language} 文案`);
  assert.ok(group.rewindScopeWorkspace.length > 0 && group.rewindScopeWorkspaceOffNote.length > 0);
}
// 中性化方向锚定：两车道共用同文案，不得再以「代码变更」限定回退对象。
assert.doesNotMatch(dict.zh.uiCodex.rewindChangesToUndo, /代码/, '将撤销的变更文案必须中性（工作模式共用）');
assert.doesNotMatch(dict.en.uiCodex.rewindChangesToUndo, /code/i, 'File changes copy must stay lane-neutral');

console.log('chat_rewind_wiring: all assertions passed');
