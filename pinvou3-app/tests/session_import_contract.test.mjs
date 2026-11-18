import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// 会话备份导入 + 从此轮分叉：source-contract 断言（session_export_contract
// 风格——无 DOM harness 的接线钉子）。钉住：导入面住在懒加载 SettingsView
// （主入口不背这些字节）、取消静默、toast 组成、行门控与 in-flight 禁用；
// 分叉入口的桥/只读/busy 门控与错误透出；序号口径的纯函数归位。

const main = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');
const settings = readFileSync(new URL('../src/features/settings/SettingsView.jsx', import.meta.url), 'utf8');
const chatView = readFileSync(new URL('../src/features/chat/ChatView.jsx', import.meta.url), 'utf8');
const timeline = readFileSync(new URL('../src/features/conversation/ConversationTimeline.jsx', import.meta.url), 'utf8');
const forkEntries = readFileSync(new URL('../src/features/chat/forkTurnEntries.js', import.meta.url), 'utf8');
const checkpoints = readFileSync(new URL('../src/features/codex/checkpoints.js', import.meta.url), 'utf8');

// 1. 导入处理器住在懒加载的 SettingsView：bridge 守卫、in-flight 早退、
// 取消（null）静默返回、成功/失败 toast 组成，主入口不再携带导入面。
{
  const at = settings.indexOf("await invokeTauri('import_session_archive')");
  assert.notStrictEqual(at, -1, 'SettingsView must invoke import_session_archive');
  const body = settings.slice(Math.max(0, at - 400), at + 2200);
  // import_session_archive 不经引擎（纯本地 store + 原生对话框）：只挡并发
  // 重入，bridge 断连不再拦截（r7 minor——此前点击静默无响应）。
  assert.ok(body.includes('if (importingSessions) return;'), 'handler must guard re-entry (the command needs no engine, bridge availability must not silently no-op the click)');
  assert.ok(body.includes('if (!result) return;'), 'user cancel (null) must return silently');
  // 长文案（四计数 + 缺产物补救指引 / 失败详情）自带 8 秒时长，默认 pill
  // 的 3 秒会把恢复路径截掉（r7 minor）。
  assert.ok(body.includes('duration: 8000'), 'long import toasts must carry their own duration');
  // multiline（r8）：80vw 单行 truncate 在窄窗口上仍会裁掉补救指引的尾部，
  // 长文案（有缺产/失败）必须放开为可换行形态。
  assert.ok(
    body.includes('multiline: artifactsFailedCount > 0 || (result.failed || []).length > 0')
      && body.includes('multiline: true'),
    'long import toasts must opt into the multiline pill on BOTH the conditional success toast and the failure toast (r8 mutation 核对：只删条件腿本钉转红)',
  );
  assert.ok(body.includes('t.uiSettings.importSessionsDone(') && body.includes('result.skipped_existing') && body.includes('result.artifacts_failed'), 'success toast must compose imported/skipped/failed counts plus the partial (missing-artifacts) count');
  assert.ok(body.includes('failure.reason'), 'per-record failure reasons must at least reach the console (count-only left users with "1 failed" and no cause)');
  assert.ok(body.includes('t.uiSettings.importSessionsFailed') && body.includes('${detail}'), 'failure must surface the localized failed toast with the backend detail');
  assert.ok(!main.includes('import_session_archive'), 'main entry must not carry the import surface (entry-budget ledger policy)');
}

// 2. Settings 导入行：桌面独享门控、合同 testid、in-flight 禁用态。
assert.match(
  settings,
  /\{!isWeb && setSettingsToast && \(\s*<IOSSection title=\{t\.uiSettings\.sessionsSection\}>/,
  'import row must be desktop-only and gated on the toast channel',
);
assert.match(
  settings,
  /data-testid="settings-import-sessions" disabled=\{importingSessions\}/,
  'import button must keep its testid and disable while importing',
);

// 3. 分叉入口：ChatView 经纯函数建表；busy 整表隐藏；桥不可用/多代理只读
// 车道不传处理器；错误带详情透出到会话通知。
assert.match(
  chatView,
  /forkEntriesByTurnId\(conversationProjection\.turns\)/,
  'fork entries must come from the pinned pure helper',
);
assert.match(
  chatView,
  /forkEntries=\{\(busy \|\| ctxCompactInFlight\) \? undefined : forkEntries\}/,
  'fork entries must hide entirely while a turn or a compaction is in flight',
);
// 压缩进行中（ctxCompactInFlight）整表隐藏：空闲会话的手动压缩全程 busy
// 都是 false（compaction 不占 turn 预约），单独的 busy 门挡不住「点击必报
// 错」入口（r7 M1——锚定 forkSupported 块防 ChatView 他处同名表达式撑绿）。
assert.match(
  chatView,
  /const forkSupported = !!\([\s\S]*?&& !ctxCompactInFlight/,
  'fork support must gate on compaction-in-flight (the Rust probe rejects and the raw diagnostic would toast on click)',
);
assert.match(
  chatView,
  /canInvoke\('fork_session'\)/,
  'fork support must be gated through canInvoke (web access-policy refusal must hide the entry, not error on click)',
);
// 锚定到 forkSupported 块（chatView 里有五处同款 sched- startsWith；不锚定
// 的话删掉分叉门的检查仍会被 aux 车道的同款表达式撑绿）。大小写不敏感
// （toLowerCase）对齐 Rust is_sched_session_id 的 eq_ignore_ascii_case——
// 大写变体 id 在大小写不敏感文件系统上就是定时会话本体。
assert.match(
  chatView,
  /const forkSupported = !!\([\s\S]*?!activeSessionId\.toLowerCase\(\)\.startsWith\('sched-'\)/,
  'scheduled-run sessions must not render fork entries (Rust refuses them unconditionally)',
);
// fork 失败详情的三语兜底（'unknown error' 字面量不得回流 toast）。
assert.doesNotMatch(
  chatView,
  /\|\s*'unknown error'/,
  'fork failure detail must fall back to the i18n unknownError key, not an English literal',
);
// 自动切换仅 ChatView 仍挂载时执行（卸载后 activeSessionIdRef 冻结，
// 会替用户在别的视图背后切会话）。
assert.match(
  chatView,
  /chatViewMountedRef\.current && activeSessionIdRef\.current === sourceSessionId/,
  'fork auto-switch must be gated on ChatView still being mounted',
);
// fork 失败的 REBIND_IN_PROGRESS 走既有三语分类映射（rebindErrors.js），
// 不把后端英文诊断尾巴原样亮给用户（review r2：持锁方是 fork 自身时，
// 原样尾巴还会误导指认「目录 rebind」）。
assert.match(
  chatView,
  /classifyRebindError\(err, t\)/,
  'fork failure must classify rebind rejections through the shared mapping',
);
assert.match(
  chatView,
  /classified\.kind === 'copy'/,
  'only marker-classified rejections use the mapped copy; raw detail stays for everything else',
);
// in-flight 门按会话记（Set）：单槽位会被并发分叉互相清掉（A 落盘清掉 B
// 的标记，B 上的再点击绕过前端门撞 Rust 预约拒绝）。
assert.match(
  chatView,
  /forkBusySessionIds\.has\(sourceSessionId\)/,
  'the fork in-flight guard must be keyed per session (a Set, not a single slot)',
);
assert.match(
  chatView,
  /onForkTurn=\{!forkSupported \|\| isMultiAgentReadOnly \? undefined : handleForkTurn\}/,
  'fork handler must be withheld when unsupported and on multi-agent read-only lanes',
);
assert.match(
  chatView,
  /invokeTauri\('fork_session', \{ sessionId: sourceSessionId, keepTurns \}\)/,
  'fork handler must pass the 1-based keepTurns ordinal for the captured source session',
);
assert.match(
  chatView,
  /activeSessionIdRef\.current === sourceSessionId/,
  'auto-switch must fire only while the user is still on the source session',
);
assert.match(
  chatView,
  /t\.uiConversation\.forkFailed/,
  'fork failure must surface the localized notice with detail',
);
// 自动切换失败路径 resolve(false) 而非 reject：布尔为假必须补专用提示，
// 否则 forkSwitchFailed 的 catch 臂是死代码、失败只落桥接层通用条目
// （r7 minor 2）。
assert.match(
  chatView,
  /switched = await bridge\.sessions\.switchToSession\(forked\.id\);/,
  'the auto-switch result must be consumed (the bridge resolves false on failure, it never rejects)',
);
assert.match(
  chatView,
  /if \(!switched && chatViewMountedRef\.current && activeSessionIdRef\.current === sourceSessionId\)/,
  'a failed auto-switch must flash the dedicated forkSwitchFailed notice while still on the source session',
);

// 4. 时间线按钮：只有处理器与入口表同时在场才渲染，testid 稳定；渲染判定
// 独立于 assistant footer 可见性（footer 还要求生命周期已知/有回复——序号表
// 对纯用户轮照样编号，按钮不得随之消失），footer 隐藏时以独立分叉行渲染。
assert.match(
  timeline,
  /const forkButton = forkTurnVisible && \(\s*<button type="button" data-testid="conversation-fork-turn"/,
  'fork button must be gated on handler + per-turn ordinal and keep its testid',
);
assert.match(
  timeline,
  /\{!assistantFooterVisible && forkTurnVisible && \(\s*<div[^>]*data-testid="conversation-fork-row"/,
  'turns without an assistant footer must still render their fork entry as a standalone row',
);
assert.match(
  timeline,
  /const \{ forkEntries: forkEntriesProp, \.\.\.turnProps \} = props;/,
  'the fork map must not ride into per-turn props (memo comparator churn)',
);

// 5. 序号口径：分叉表与回退表共享 turnHasUserText（纯图片 turn 不占序号，
// 与 Rust 底座 era 口径对齐）。
assert.match(
  forkEntries,
  /import \{ turnHasUserText \} from '\.\.\/conversation\/deepseek-conversation\.js';/,
  'fork entries helper must reuse the projection-owned turn classifier',
);
assert.match(
  checkpoints,
  /!turnHasUserText\(turn\)/,
  'rewind entries must skip image-only turns on the same basis as fork',
);
