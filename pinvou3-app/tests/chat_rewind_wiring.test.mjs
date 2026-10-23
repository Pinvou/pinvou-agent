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
const tauriInteraction = source('platform/tauri/bridge/interaction.js');
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
// 评审 r9 M5 / r10 M3 / r11 M4：重载投影的三个用户气泡过滤（tool_result 载体 /
// 步数预算通知 / 压缩载体）。main FE14-wave5 把 rerender 主体去重进
// shared/bridge-shared-helpers.js 的 rerenderFromMessagesCore（两 lane 经 hooks
// 注入差异），过滤随主体单份落在共享核心——断言对象从两份 lane 内联循环改为
// 共享核心 + 「两 lane 必须真的调核心」（lane 静默回退内联循环会让过滤全部失效）。
const sharedHelpers = source('shared/bridge-shared-helpers.js');
assert.match(tauriBridge, /rerenderFromMessagesCore\(/, 'tauri 重载必须走共享核心 rerenderFromMessagesCore（lane 回退内联循环会让全部用户气泡过滤失效）');
assert.match(webBridge, /rerenderFromMessagesCore\(/, 'web 重载必须走共享核心 rerenderFromMessagesCore（lane 回退内联循环会让全部用户气泡过滤失效）');
// 形状锚（非标识符共现）——r22 集成修复（main #749 重放口径）：载体取用户
// 文本必须先剥运行时图片标记（`<image path="…">`/`</image>`），纯载体剥完为空
// 仍无气泡，混合形态保留 user 边界。变异实证：删掉 isImageMarkerTextBlock
// 过滤（载体直取全文）或删掉 toolResultCarrier 分支（混合形态出幻影气泡）
// 必须双双变红。
assert.match(sharedHelpers, /toolResultCarrier\s*\?\s*blocks\.filter\(b => !isImageMarkerTextBlock\(b\)\)\s*:\s*blocks/, '重载核心：tool_result 载体必须先剥运行时图片标记再取用户文本（纯载体无气泡、混合形态保留边界）');
assert.match(sharedHelpers, /function isImageMarkerTextBlock\(b\)/, '重载核心：图片标记谓词必须随核心定义（与 CodeWhale image_attach/turn_loop 的 <image path="…"> 标记同形）');
// 步数预算通知过滤（r10 M3）形状锚。
assert.match(sharedHelpers, /stepBudgetNotice[\s\S]{0,240}if \(utext && !stepBudgetNotice\)/, '重载核心：步数预算通知必须被过滤出用户气泡');
// 压缩载体过滤的调用形状锚（变异实证：禁用过滤行全绿）。过滤器必须在 rerender
// 的 user 分支里被调用（carrier check 在 bubble 创建前），谓词定义在共享核心内。
assert.match(sharedHelpers, /if \(isCompactionCarrierBlocks\(blocks\)\) continue;/, '重载核心：压缩载体必须在用户气泡创建前被过滤——禁用该行必须变红');
assert.match(sharedHelpers, /function isCompactionCarrierBlocks\(blocks\)/, '重载核心：压缩载体谓词必须随核心定义（classic script 无法 import ESM，与 internal-message.mjs 逐字镜像）');
// r13 M4：三个安全关键机制的接线 pin——变异实证均全绿。
// 1) forceDurableLoad：删掉第四参 → 同 id 快路径假成功（r7-M3 修掉的形状）。
assert.match(chatView, /switchToSessionInternal\(sid, true, 'chat-rewind', \{ forceDurableLoad: true \}\)/,
  'reload 必须传 forceDurableLoad: true——删掉第四参回退重载走快路径假成功');
// 2) filesWillRewind：改成恒 false → 确认键在预览未决时可用（破坏性文件还原绕过成本可见）。
assert.match(rewindDialogs, /const filesWillRewind = !entry\.conversationOnly && \(!scopeChoiceable \|\| includeWorkspace\)/,
  'filesWillRewind 的推导链必须锚定——恒 false 会让预览门形同虚设');
// 3) scope payload：硬编码 true → 复选框变死 UI、仅对话回退不可达。
assert.match(rewindDialogs, /onConfirm\(\{ includeWorkspace \}\)/,
  '确认回调必须传 { includeWorkspace }——硬编码 true 复选框变死 UI');
assert.match(controller, /includeWorkspace === false/,
  '控制器侧必须消费 includeWorkspace === false（仅对话回退腿）');
// r14 M-D：code 车道有意不注入 invalidateSkippedBuffer（loadSession 每次从磁盘
// 重读，无快路径缓存可陈旧）——此形状必须被钉住，防止被当作遗漏"修复"回去。
assert.doesNotMatch(codexView, /invalidateSkippedBuffer/, 'code 车道不得注入 invalidateSkippedBuffer（loadSession 无缓存快路径，注入是 work 模式专用修复）');
// 4) ChatView 注入接线：删掉 invalidateSkippedBuffer 注入 → 静默 no-op。
assert.match(chatView, /invalidateSkippedBuffer:/,
  'ChatView 必须注入 invalidateSkippedBuffer——删掉注入回退失效静默 no-op');
// r12 M3：invalidateSkippedCache 的调用形状锚——变异实证：改回读执行时刻的
// depsRef.current.sessionId（r11-M1 修掉的回归形状）当前全绿。六个调用点
// 必须传闭包局部 sessionId（回退发起时的目标会话 id，入口现取非陈旧捕获）。
assert.doesNotMatch(controller, /invalidateSkippedBuffer\?\.\(depsRef\.current\.sessionId\)/,
  '不得回读执行时刻的活动会话 id——那是 r11-M1 修掉的回归形状（目标会话漏失效 + 活动会话被误伤）');
assert.match(controller, /invalidateSkippedCache\(sessionId\)/,
  '调用点必须传闭包局部 sessionId（回退目标）——至少一处形状锚定');
const invalidateCallCount = (controller.match(/invalidateSkippedCache\(sessionId\)/g) || []).length;
assert.equal(invalidateCallCount, 6, `全部六处调用点必须传闭包局部 sessionId（单点回归此前可全绿），实际 ${invalidateCallCount}`);
// r11 M4：frontendEcho 标记行的形状锚（变异实证：禁用标记行全绿）。标记必须
// 在 pushUserEcho 的 !persist 分支——回退为恒不标记即回滚 r5-M1。
assert.match(tauriInteraction, /if \(!persist\) item\.frontendEcho = true;/, 'tauri 交互桥：persist=false 的回显必须标记 frontendEcho——禁用标记行必须变红');
assert.match(webBridge, /if \(!persist\) item\.frontendEcho = true;/, 'web bridge：persist=false 的回显必须标记 frontendEcho——禁用标记行必须变红');
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
  /resetRewindForSessionSwitch\(\);\s*\n\s*\}, \[activeSessionId, resetRewindForSessionSwitch\]\);/,
  'ChatView 必须在会话切换时调用共享 hook 的 resetForSessionSwitch，且 effect 依赖数组必须含 activeSessionId（r20 M2：漏掉它则跨会话陈旧回退截断复活而本钉不红）',
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
// r20 minor 3：三个回退提示键的删除守卫——ui_language_coverage 只校验 en/ja
// 覆盖 zh，键从所有字典删光时全套件仍绿（UI 渲染 undefined 文案）。
for (const language of ['zh', 'en', 'ja']) {
  for (const key of ['rewindReloadFailed', 'rewindFilesDeleteNote', 'rewindUndoExpiryNote']) {
    const text = dict[language].uiCodex[key];
    assert.equal(typeof text, 'string', `${key} 必须有 ${language} 文案（删除守卫）`);
    assert.ok(text.length > 0, `${key} 的 ${language} 文案不得为空`);
  }
}

// r21 M1：计数源钉——list_checkpoints 的 userTurns 必须派生自账本索引末条 Turn
// 快照（O(1)，无 transcript 解析）；回退成 store.load 全量计数或删掉派生都要红。
// 压缩收缩窗口的正确性由后端收缩钩子的作废保证（compaction_shrink_keeps_ledger_
// count_transcript_honest 锚定），两侧合起来才是完整的调和门契约。
{
  const rs = readFileSync(new URL('../src-tauri/src/app/commands/checkpoints.rs', import.meta.url), 'utf8');
  assert.match(
    rs,
    /let user_turns = entries[\s\S]{0,40}?\.rev\(\)[\s\S]{0,120}?CheckpointKind::Turn/,
    'list_checkpoints 的 userTurns 必须从索引末条 Turn 快照派生（r21 M1 计数源钉）',
  );
  const listBody = rs.split('pub async fn list_checkpoints')[1] || '';
  assert.doesNotMatch(
    listBody.split('pub async fn')[0],
    /store_count|.load(&session_id_count)/,
    'list_checkpoints 不得回退到全量 transcript 解析计数（r19-M8 成本回归）',
  );
}
console.log('chat_rewind_wiring: all assertions passed');
