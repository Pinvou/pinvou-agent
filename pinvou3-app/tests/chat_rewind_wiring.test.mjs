// 工作模式（原生 chat 车道）回退接线契约：静态断言 ChatView 挂上了共享回退编排
// （renderBeforeTurn 入口 + 确认/撤销弹窗 + scopeSelection 变体），code 车道
// （CodexAcpView）改用共享模块且弹窗维持无双层选择项，共享层自 codex 迁入后
// 旧位置不再存在。纯静态源断言（同 codex_checkpoints_logic 的 web 策略锚定法，
// 不挂 DOM）。
import assert from 'node:assert/strict';
import { readFileSync, existsSync } from 'node:fs';
import { dict } from './helpers/i18n-all.js';

const source = relative => readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');
const tauriBridge = source('platform/tauri/bridge.js');
const webBridge = source('platform/web/bridge.js');
const chatView = source('features/chat/ChatView.jsx');
const codexView = source('features/codex/CodexAcpView.jsx');
const controller = source('features/conversation/useRewindController.js');
const rewindChip = source('features/conversation/RewindChip.jsx');
const rewindDialogs = source('features/conversation/RewindDialogs.jsx');
// r11 拆分形状锚：chips 文件只导出入口组件（弹窗已移 RewindDialogs.jsx 懒加载，
// 不占启动主入口预算）；弹窗文件不导出 chips（防止拆分漂移回去）。
assert.match(rewindChip, /export function RewindChip\(/, 'RewindChip.jsx 必须导出 RewindChip');
assert.match(rewindChip, /export function RewindUndoChip\(/, 'RewindChip.jsx 必须导出 RewindUndoChip');
assert.doesNotMatch(rewindChip, /export function RewindConfirmDialog\(/, '确认弹窗必须已拆出（不得留在启动路径的 chips 文件内）');
assert.match(rewindDialogs, /export function RewindConfirmDialog\(/, 'RewindDialogs.jsx 必须导出确认弹窗');
assert.match(rewindDialogs, /export function RewindUndoConfirmDialog\(/, 'RewindDialogs.jsx 必须导出撤销确认弹窗');

// ── 运行时桥暴露锚定：ChatView 依赖的两个 bridge 方法必须真的在 TauriBridge
// curated 对象上（静态源断言曾漏过此层——方法只存在于 feature registry 而
// 未重导出时，typeof 守卫静默走 fallback，回退重载/提示全失效）────────────
{
  const bridgeSource = source('platform/tauri/bridge.js');
  const sessionsSurface = bridgeSource.match(/^\s{4}sessions: \{[\s\S]*?\n\s{4}\},/m);
  assert.ok(sessionsSurface, 'curated sessions 对象必须存在');
  assert.match(
    sessionsSurface[0],
    /switchToSessionInternal/,
    'bridge.sessions 必须暴露 switchToSessionInternal（回退后的强制盘载重注水；公开 switchToSession 同 id 快路径不会重载）',
  );
  const chatSurface = bridgeSource.match(/^\s{4}chat: \{[\s\S]*?\n\s{4}\},/m);
  assert.ok(chatSurface, 'curated chat 对象必须存在');
  assert.match(
    chatSurface[0],
    /addSystemItem/,
    'bridge.chat 必须暴露 addSystemItem（回退/撤销结果的内联提示落点）',
  );
}

// ── ChatView 接线：共享模块 + 时间线入口 + 弹窗 ──────────────────────
// 共享模块导入（搬家后唯一来源是 features/conversation）。
assert.match(chatView, /from '\.\.\/conversation\/useRewindController\.js'/, 'ChatView 必须使用共享编排 hook');
// 评审 r9 M5：两车道重载投影必须跳过含 tool_result 的 User 消息的用户气泡
// （后端按 ToolResult-first 判 NotPrompt；工具卡由 toolMeta 回填还原）——
// 否则截图类工具回传的 [ToolResult, Text, ImageUrl, Text] 渲染幻影气泡并顶
// 偏序号，调和门随之隐藏该会话全部入口。
// r10 M5：形状锚（非标识符共现）——删掉三元守卫（回退为直取
// userMessageDisplayText）必须变红；只留未用变量声明的变异同样变红。
assert.match(tauriBridge, /toolResultCarrier\s*\?\s*""\s*:\s*userMessageDisplayText/, 'tauri 重载：tool_result 载体必须以三元守卫跳过用户气泡');
assert.match(webBridge, /toolResultCarrier\s*\?\s*""\s*:\s*userMessageDisplayText/, 'web 重载：tool_result 载体必须以三元守卫跳过用户气泡');
// 步数预算通知过滤（r10 M3）双车道同款形状锚。
assert.match(tauriBridge, /stepBudgetNotice[\s\S]{0,240}if \(utext && !stepBudgetNotice\)/, 'tauri 重载：步数预算通知必须被过滤出用户气泡');
assert.match(webBridge, /stepBudgetNotice[\s\S]{0,240}if \(utext && !stepBudgetNotice\)/, 'web 重载：步数预算通知必须被过滤出用户气泡');
// r10 M5：busy 容差必须在调用点以 { busy: Boolean(busy) } 传入——删掉该参数
// （生产失去 busy 容差、每轮发送期间入口消失）必须变红；函数级单测对此静默。
assert.match(
  controller,
  /rewindEntriesByTurnId\(turns, checkpoints\.checkpoints, checkpoints\.userTurns, \{ busy: Boolean\(busy\) \}\)/,
  'entries 构建必须传 { busy: Boolean(busy) }（r9 minor 9 的调用点锚）',
);
// 评审 r7 M3(d)：回退后重载必须走 force-durable-load 的 switchToSessionInternal
// （换 plain switchToSession 的同 id 快路径会把截断前的陈旧时间线当成功上屏，
// 全部套件对此静默）。
assert.match(
  chatView,
  /switchToSessionInternal/,
  '回退重载必须调用 switchToSessionInternal（force-durable-load），不得退回 plain switchToSession',
);
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

// 会话切换复位：ChatView 不按会话 remount，必须挂 [activeSessionId] 复位
// （否则切换前打开的确认弹窗会对新会话执行旧目标的回退——跨会话截断）。
assert.match(
  chatView,
  /resetRewindForSessionSwitch\(\);/,
  'ChatView 必须在会话切换时调用共享 hook 的 resetForSessionSwitch',
);
// reload 布尔合同：switchToSessionInternal 失败返回 false 而非 reject，视图
// 侧必须转换成异常交给共享编排的 reload 失败路径。
assert.match(
  chatView,
  /ok !== true/,
  'ChatView 的 reload 必须把 switchToSessionInternal 的 false 返回当作失败抛出',
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
assert.match(rewindDialogs, /useState\(true\)/, '回退范围选择默认勾选（双层回退为推荐值）');
assert.match(
  rewindDialogs,
  /scopeChoiceable = scopeSelection && !entry\.conversationOnly/,
  '无快照边界不得呈现回退范围选择项',
);

// 确认键预览门控（评审 r4 M2 / r6 minor 1 pin）：默认勾选的破坏性恢复必须
// 在代价可见后才可执行——预览未决（未加载/加载中/失败）时禁用确认键。
// 回滚该门控（okDisabled 恒 false）必须让本 pin 变红。
assert.match(
  rewindDialogs,
  /const previewUndecided = filesWillRewind[\s\S]{0,300}?!previewState \|\| previewState\.loading \|\| previewState\.error/,
  '预览未决（未加载/加载中/失败）必须计入 previewUndecided',
);
assert.match(
  rewindDialogs,
  /okDisabled=\{previewUndecided\}/,
  '确认键必须接入 okDisabled={previewUndecided}（预览未决时禁用）',
);
assert.match(
  rewindDialogs,
  /disabled=\{busy \|\| okDisabled\}/,
  '弹窗壳的确认键禁用条件必须包含 okDisabled',
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
