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
  const body = settings.slice(Math.max(0, at - 400), at + 500);
  assert.ok(body.includes('if (!bridge.available || importingSessions) return;'), 'handler must guard bridge availability and re-entry');
  assert.ok(body.includes('if (!result) return;'), 'user cancel (null) must return silently');
  assert.ok(body.includes('t.uiSettings.importSessionsDone(') && body.includes('result.skipped_existing'), 'success toast must compose imported/skipped/failed counts');
  assert.ok(body.includes('t.uiSettings.importSessionsFailed'), 'failure must surface the localized failed toast');
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
  /forkEntries=\{busy \? undefined : forkEntries\}/,
  'fork entries must hide entirely while a turn is in flight',
);
assert.match(
  chatView,
  /onForkTurn=\{!bridge\.available \|\| isMultiAgentReadOnly \? undefined : handleForkTurn\}/,
  'fork handler must be withheld on web and multi-agent read-only lanes',
);
assert.match(
  chatView,
  /invokeTauri\('fork_session', \{ sessionId: activeSessionId, keepTurns \}\)/,
  'fork handler must pass the 1-based keepTurns ordinal',
);
assert.match(
  chatView,
  /t\.uiConversation\.forkFailed/,
  'fork failure must surface the localized notice with detail',
);

// 4. 时间线按钮：只有处理器与入口表同时在场才渲染，testid 稳定。
assert.match(
  timeline,
  /\{onForkTurn && forkEntries && forkEntries\.has\(turn\.id\) && \(\s*<button type="button" data-testid="conversation-fork-turn"/,
  'fork button must be gated on handler + entry and keep its testid',
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
