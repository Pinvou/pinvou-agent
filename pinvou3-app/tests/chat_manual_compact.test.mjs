#!/usr/bin/env node
// 主车道手动压缩入口（ctx 用量行按钮）的契约测试：
//   1. 行为层（vm 驱动 tauri chat-events 监听器）：chat:compaction start→终态
//      维护 state.compactingSessions 在途标记；done 的 post_tokens 是压缩后
//      占用的保守估计（forwarder 为刷新用量条而加），先于 dirty guard 消费。
//   2. 源码契约层：ChatView 按钮守卫（busy/compacting/按会话入队）、
//      session_engine_not_running 本地化映射、web 双端同改、i18n 每语言恰一份。
// 复用 chat_usage_context_window.test.mjs 的 vm 装置。
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => readFileSync(path.join(root, ...parts), 'utf8');

// ── tauri 端行为测试：compactingSessions 标记 + post_tokens 消费 ──
const source = read('src', 'shared', 'bridge-shared-helpers.js') + '\n' + read('src', 'platform', 'tauri', 'bridge', 'chat-events.js');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInContext(source, vm.createContext({
  window: windowObject,
  console,
  Date,
  String,
}), { filename: 'chat-events.js' });

const listeners = new Map();
let notifyCount = 0;
const pruneMerges = [];
const state = {
  activeSessionId: 'session-1',
  chatItems: [],
  messages: [],
  tokens: { input: 40000, max: 131072 },
  thinking: { active: false },
  compactingSessions: {},
};
const context = {
  state,
  listen(name, handler) { listeners.set(name, handler); },
  notify() { notifyCount += 1; },
  invoke: async () => null,
  turnUsageDirty: {},
  sessionStates: {},
  renderMarkdown(text) { return `<p>${text}</p>`; },
  bt() { return ''; },
  onSessionEvent(_event, callback) { callback(); },
  runSyncOnSession(_sessionId, callback) { callback(); },
  addChatItem(item) {
    item.id = ++context.itemIdSeq;
    state.chatItems.push(item);
  },
  addSystemItem(text, meta = {}) {
    context.addChatItem({ type: 'system', text, ...meta });
  },
  addOrMergePruneCompaction(compactId) { pruneMerges.push(compactId); },
  timeStr() { return '12:00'; },
  flushPendingTextBlock() {
    if (!context.pendingAssistantText) return;
    context.pendingAssistantBlocks.push({ type: 'text', text: context.pendingAssistantText });
    context.pendingAssistantText = '';
  },
  currentStreamId: 1,
  currentStreamText: '',
  pendingAssistantText: '',
  pendingAssistantBlocks: [],
  itemIdSeq: 1,
};
windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['chat-events'](context);

const emitCompaction = payload => listeners.get('chat:compaction')({
  event: 'chat:compaction',
  payload: { session_id: 'session-1', ...payload },
});

// 场景 1：start 置位带时间戳的在途标记（手动与自动压缩共用同一事件流；
// 读取侧按时间戳做 15 分钟兜底自愈，见 ChatView 与驱逐保护谓词）。
emitCompaction({ phase: 'start', id: 'compact-1', auto: false, message: 'Manual context compaction started' });
const compactStartedAt = state.compactingSessions['session-1'];
assert.ok(typeof compactStartedAt === 'number' && compactStartedAt > 0, 'start 必须置位带时间戳的压缩在途标记');
assert.equal(state.chatItems.length, 1, 'start 系统项照常入列');

// 场景 2：done 清除标记，且 post_tokens 是压缩后的占用估计——直接刷新
// 用量条（保留真实窗口分母），不等下一轮 usage。
const notifyAfterStart = notifyCount;
emitCompaction({ phase: 'done', id: 'compact-1', messages_before: 12, messages_after: 8, post_tokens: 9500, message: '12 → 8 messages' });
assert.equal(state.compactingSessions['session-1'], undefined, 'done 终态必须清除在途标记');
assert.equal(state.tokens.input, 9500, 'post_tokens 必须刷新用量条 input');
assert.equal(state.tokens.max, 131072, '真实窗口分母保持不变');
assert.ok(notifyCount > notifyAfterStart, '标记清除/用量刷新必须通知 UI');

// 场景 3：post_tokens 守卫——缺失/非正数/超窗不得污染用量条（保留上次准确值）。
for (const bad of [undefined, 0, -5, 999999]) {
  emitCompaction({ phase: 'start' });
  const inputBefore = state.tokens.input;
  emitCompaction({ phase: 'done', messages_before: 8, messages_after: 8, post_tokens: bad, message: 'x' });
  assert.equal(state.tokens.input, inputBefore, `post_tokens=${bad} 不得更新用量条`);
}

// 场景 3b：边界——post_tokens 恰好等于窗口上限必须接受（守卫是 <= 而非
// <；压缩后占用恰在窗口边缘是合法状态，改成 < 会让该合法值被拒）。
emitCompaction({ phase: 'start' });
emitCompaction({ phase: 'done', messages_before: 8, messages_after: 8, post_tokens: 131072, message: 'x' });
assert.equal(state.tokens.input, 131072, 'post_tokens 恰等于窗口上限必须刷新用量条');

// 场景 4：fail/cancel 终态同样清除标记——不能留下永久禁用的入口。
for (const phase of ['fail', 'cancel']) {
  emitCompaction({ phase: 'start' });
  assert.ok(state.compactingSessions['session-1'] > 0);
  emitCompaction({ phase, message: 'x' });
  assert.equal(state.compactingSessions['session-1'], undefined, `${phase} 终态必须清除在途标记`);
}

// 场景 5：prune-only 自动压缩（done+pruneOnlyAuto 合并路径）也要清标记。
emitCompaction({ phase: 'start' });
emitCompaction({ phase: 'done', auto: true, messages_before: 8, messages_after: 8, message: '2 tool results pruned' });
assert.equal(state.compactingSessions['session-1'], undefined, 'prune-only 合并路径同样必须清标记');
assert.equal(pruneMerges.length, 1, 'prune-only 合并路径不受影响');

// 场景 6：无 session_id 的畸形事件——不得写标记，也不得炸监听器。
assert.doesNotThrow(() => emitCompaction({ phase: 'start', session_id: undefined }));
assert.equal(Object.keys(state.compactingSessions).length, 0, '无 session_id 不得写标记');

// 场景 7：压缩轮的 usage 事件仍被 dirty guard 冻结（post_tokens 已给出
// 占用估计，后续累计 input 不可信——语义与 chat_usage_context_window 的
// dirty 场景一致）。dirty 必须由压缩事件自身置位（监听器自标记），
// 测试不得预置——否则删掉自标记本场景仍绿，post_tokens 绕过 dirty 的
// 前提就失钉了。
const emitUsage = payload => listeners.get('chat:usage')({
  event: 'chat:usage',
  payload: { session_id: 'session-1', ...payload },
});
emitCompaction({ phase: 'start', session_id: 'session-2' });
assert.ok(context.turnUsageDirty['session-2'], '压缩事件必须自置 turnUsageDirty（post_tokens 绕过 dirty 的前提）');
emitUsage({ session_id: 'session-2', input_tokens: 40000 });
assert.equal(state.tokens.input, 131072, '压缩轮后续累计 input 不可信，保留最近接受的 post_tokens 值');

// ── web 双端源码契约：压缩 handler 必须同步维护标记并消费 post_tokens ──
const tauriCompaction = source.slice(source.indexOf('listen("chat:compaction"'));
const webSource = read('src', 'platform', 'web', 'bridge.js');
const webCompaction = webSource.slice(webSource.indexOf('listen("chat:compaction"'));
for (const [label, section] of [['tauri', tauriCompaction], ['web', webCompaction]]) {
  assert.ok(section.includes('if (phase === "start") state.compactingSessions[sid] = Date.now();'), `${label}: start 必须置位带时间戳标记（读取侧据此兜底自愈）`);
  assert.ok(section.includes('else delete state.compactingSessions[sid];'), `${label}: 终态必须经 else 分支清标记（done/fail/cancel 全覆盖）`);
  assert.ok(section.includes('state.tokens = { input: postTokens, max: state.tokens.max };'), `${label}: 必须把 post_tokens 写入用量条`);
  assert.ok(section.includes('postTokens <= state.tokens.max'), `${label}: post_tokens 超窗守卫保留`);
  assert.ok(section.includes('兜底按墙钟（Date.now）而非 Rust 侧 Instant 单调钟'), `${label}: 墙钟兜底的取舍必须留档（提前解禁窗口是接受的代价，防止被误"修复"）`);
}
// 双端同改的强约束：两条注册体从 listen 标记到 post_tokens 块尾必须逐字节
// 一致——web 侧漂移不再只依赖上面 4 条字符串 pin 的间接覆盖。
const compactionSpanTail = '        notify();\n      }\n    }';
const spanToPostTokensTail = section => section.slice(0, section.indexOf(compactionSpanTail) + compactionSpanTail.length);
assert.equal(spanToPostTokensTail(tauriCompaction), spanToPostTokensTail(webCompaction),
  'tauri/web 压缩注册体公共前缀必须逐字节一致（标记维护 + post_tokens 消费双端同改）');

// ── 桥接状态接线：字段进切片、两端 state 声明、删除/驱逐同清、驱逐保护 ──
const sharedSource = read('src', 'shared', 'bridge-shared-helpers.js');
assert.ok(sharedSource.includes('"compactingSessions"'), 'STATE_SLICE_FIELDS.chat 必须声明 compactingSessions（两 lane 快照共用同一常量）');
assert.ok(sharedSource.includes('Date.now() - state.compactingSessions[id] < 15 * 60 * 1000'), '容量驱逐保护必须按兜底时限读在途标记（过期不保护）');
const tauriBridgeSource = read('src', 'platform', 'tauri', 'bridge.js');
assert.equal(tauriBridgeSource.split('compactingSessions: {}').length - 1, 1, 'tauri state 必须声明 compactingSessions');
assert.equal(webSource.split('compactingSessions: {}').length - 1, 1, 'web state 必须声明 compactingSessions');
const tauriSessionsSource = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
assert.equal(tauriSessionsSource.split('delete state.compactingSessions[id]').length - 1, 3, 'tauri 三处删除/驱逐点必须同清标记');
assert.equal(webSource.split('delete state.compactingSessions[id]').length - 1, 3, 'web 三处删除/驱逐点必须同清标记');

// ── ChatView 源码契约：守卫、按会话入队、错误映射、可测性挂钩 ──
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx').replace(/\r\n/g, '\n');
// 守卫必须同时挡 busy（轮中入队→轮末前无反馈）与压缩在途（终态前重复入队）。
assert.match(chatViewSource, /if \(!sid \|\| manualCompactActive \|\| compactingActive \|\| busy\) return;/,
  '入口守卫必须同时挡 busy 与压缩在途，否则轮中点击是静默空操作且按钮复位后重复入队');
// 命令只入队即返回：sid 必须在调用点捕获（切会话后错误仍落到原会话）；
// 手动标记必须在 invoke 前置位且紧邻 try——入队到桥接 start 接管之间
// 按钮不得复位，否则双击会重复入队压缩。
assert.match(chatViewSource, /const sid = activeSessionId;\s*if \(!sid[\s\S]*?setManualCompact\(\{ sid, at: Date\.now\(\) \}\);\s*try \{/, 'sid 必须在 invoke 前捕获，手动标记置位紧邻 invoke');
assert.match(chatViewSource, /invokeTauri\('compact_now', \{ sessionId: sid \}\)/, '压缩命令与参数形状');
// 按会话 keying（ChatView 切会话不 remount）：布尔会在 A 入队中误禁 B。
assert.match(chatViewSource, /const \[manualCompact, setManualCompact\] = useState\(null\);/, '手动入队状态必须按会话记录（{sid, at}）');
assert.match(chatViewSource, /compactingStartedAt && Date\.now\(\) - compactingStartedAt < COMPACTING_FLAG_MAX_AGE_MS/, '桥接在途标记必须按 15 分钟兜底自愈（镜像 MAX_COMPACTION_LATCH_AGE）');
assert.match(chatViewSource, /manualCompact\.sid === activeSessionId\s*&& Date\.now\(\) - manualCompact\.at < COMPACTING_FLAG_MAX_AGE_MS/, '手动标记同样受兜底时限约束');
// 兜底时限常量值必须钉死（上面的 pattern pin 对值不敏感）：改成长期值会让
// 丢失终态的入口多禁一轮，改成 0 会让双击防护失效，且与驱逐保护的 15 分钟失同步。
assert.ok(chatViewSource.includes('const COMPACTING_FLAG_MAX_AGE_MS = 15 * 60 * 1000'),
  'ChatView 兜底时限常量值必须与 Rust MAX_COMPACTION_LATCH_AGE（15 分钟）钉死一致');
assert.match(chatViewSource, /setManualCompact\(current => \(current && current\.sid === sid \? null : current\)\);/, '失败复位必须校验会话身份');
// 桥接层 start 写入该会话标记后必须立即释放手动标记（身份校验，只清本会话）；
// 不得以"该会话处于激活"为前提——压缩在后台完成时 effect 看不到
// compactingActive，残留的手动标记会把按钮多禁一轮 TTL 并显示假"压缩中"。
assert.match(chatViewSource, /if \(manualCompact && compactingMap && compactingMap\[manualCompact\.sid\]\) \{[\s\S]{0,300}setManualCompact\(current => \(current && current\.sid === manualCompact\.sid \? null : current\)\);/, '桥接 start 写入标记时必须立即释放手动标记（与会话激活无关）');
// 释放 effect 的依赖数组必须钉住：缺 compactingMap 会漏掉桥接标记变化，
// 缺 manualCompact 会闭包旧标记、清错会话。
assert.match(chatViewSource, /\}, \[compactingMap, manualCompact\]\);/, 'start 接管释放 effect 必须声明 [compactingMap, manualCompact] 依赖');
// 兜底自愈必须主动触发：入口禁用期间排到期 tick，完全空闲的应用（无任何
// 无关 notify、被禁按钮点击不产生渲染）也要能在 15 分钟边界恢复按钮。
assert.match(chatViewSource, /if \(!ctxCompactInFlight\) return;[\s\S]{0,500}setCompactHealTick\(current => current \+ 1\)/, '入口禁用期间必须排兜底到期 tick（自愈不能只靠被动重渲染）');
assert.match(chatViewSource, /healBoundaryAt = Math\.min\(/, '兜底 tick 必须取桥接标记与手动标记过期边界的较早者');
// 在途文案与提示 tip 必须实际挂到按钮上（i18n key 不得沦为死文案）。
assert.match(chatViewSource, /title=\{ctxCompactInFlight \? t\.compactingNow : t\.compactNowTip\}/, 'compactingNow/compactNowTip 必须实际用于按钮');
// 引擎懒启动错误映射：裸错误链不得直接露出（与代码车道 nativeCompactInactive 同款）。
assert.match(chatViewSource, /raw\.includes\('session_engine_not_running'\) \? t\.uiCodex\.nativeCompactInactive : raw/, '引擎未运行必须映射本地化文案');
assert.ok(!chatViewSource.includes('compactFailNotice'), '失败文案必须复用 uiCodex.compactFail，不得新开重复 key');
// UI 挂钩与禁用形状。
assert.match(chatViewSource, /onClick=\{handleManualCompact\}/, '按钮必须实际接线点击处理器（卸掉 onClick 其余 pin 仍绿）');
assert.match(chatViewSource, /data-testid="chat-ctx-compact"/, '按钮必须带 testid（smoke 可达）');
assert.match(chatViewSource, /disabled=\{busy \|\| ctxCompactInFlight\}/, '禁用必须含 busy 与压缩在途');

// ── i18n：每语言恰一份（count-1 per locale），重复 key 不得回流 ──
for (const locale of ['en', 'zh', 'ja']) {
  const dictSource = read('src', 'shared', 'i18n', `${locale}.js`);
  for (const key of ['compactNowTip', 'compactingNow']) {
    assert.equal(dictSource.split(`${key}:`).length - 1, 1, `${locale}: ${key} 必须恰一份`);
  }
  assert.ok(!dictSource.includes('compactFailNotice'), `${locale}: 不得与 uiCodex.compactFail 重复新开 key`);
}
