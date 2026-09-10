import assert from 'node:assert/strict';
import {
  DENY_SUPPRESSION_MS,
  computerUseConsentView,
  extractComputerUseScreenshotPath,
  isDeniedRequestSuppressed,
} from '../src/features/computer-use/computer-use-logic.js';

// ── 截图路径提取 ──────────────────────────────────────────────────
assert.equal(
  extractComputerUseScreenshotPath('已截图，保存至 /home/u/.pinvou3/sessions/s1/attachments/computer_use/shot_1.png'),
  '/home/u/.pinvou3/sessions/s1/attachments/computer_use/shot_1.png',
  'plain text with an absolute unix path must be extracted',
);
assert.equal(
  extractComputerUseScreenshotPath('screenshot saved: C:\\Users\\asto\\.pinvou3\\sessions\\s1\\attachments\\computer_use\\shot 2.png done'),
  'C:\\Users\\asto\\.pinvou3\\sessions\\s1\\attachments\\computer_use\\shot 2.png',
  'windows drive paths with backslash separators must be extracted',
);
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({
    content: [{ type: 'text', text: '截图已保存到 /ws/attachments/computer_use/envelope.png' }],
  })),
  '/ws/attachments/computer_use/envelope.png',
  'MCP-style JSON envelopes must be unwrapped before searching',
);
// Envelope + Windows path: the raw JSON's escaped backslashes must NOT win
// the last-match rule and produce a doubled-separator path (review finding).
assert.equal(
  extractComputerUseScreenshotPath(JSON.stringify({
    content: [{ type: 'text', text: 'saved C:\\Users\\u\\attachments\\computer_use\\win.png' }],
  })),
  'C:\\Users\\u\\attachments\\computer_use\\win.png'.replaceAll('\\\\', '\\'),
  'envelope Windows paths must resolve to the clean single-separator form',
);
assert.equal(
  extractComputerUseScreenshotPath('first /ws/attachments/computer_use/a.png then /ws/attachments/computer_use/b.png'),
  '/ws/attachments/computer_use/b.png',
  'a multi-step result resolves to the newest (last) screenshot',
);
assert.equal(
  extractComputerUseScreenshotPath('SEE /WS/ATTACHMENTS/COMPUTER_USE/UP.PNG'),
  '/WS/ATTACHMENTS/COMPUTER_USE/UP.PNG',
  'matching is case-insensitive for the directory marker and extension',
);
assert.equal(
  extractComputerUseScreenshotPath('no screenshot in this step'),
  null,
  'output without a screenshot falls back to the default tool card',
);
assert.equal(
  extractComputerUseScreenshotPath('wrote /ws/attachments/notes/summary.png'),
  null,
  'PNGs outside attachments/computer_use must not be picked up',
);
assert.equal(
  extractComputerUseScreenshotPath('see attachments/computer_use/relative.png'),
  null,
  'relative paths without an absolute anchor must be rejected',
);
assert.equal(extractComputerUseScreenshotPath(null), null);
assert.equal(extractComputerUseScreenshotPath(''), null);
assert.equal(
  extractComputerUseScreenshotPath({ content: [{ type: 'text', text: 'saved /ws/attachments/computer_use/obj.png' }] }),
  '/ws/attachments/computer_use/obj.png',
  'object outputs are searched via their JSON serialization',
);

// ── 授权界面可见性状态机 ──────────────────────────────────────────
assert.deepEqual(
  computerUseConsentView(null),
  { enabled: false, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'a missing slice hides every surface',
);
assert.deepEqual(
  computerUseConsentView({ enabled: false, granted: true, grantRequest: { sessionId: 's1' }, confirmRequest: { confirmId: 'c1' } }),
  { enabled: false, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'feature toggle off: no banner, no dialogs even with stale requests',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: false, stopped: false }),
  { enabled: true, stopped: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'enabled but ungranted stays quiet until a request arrives',
);
const grantRequest = { sessionId: 's1' };
assert.deepEqual(
  computerUseConsentView({ enabled: true, grantRequest }),
  { enabled: true, stopped: false, showBanner: false, grantRequest, confirmRequest: null },
  'grant_required surfaces the grant dialog',
);
const confirmRequest = { sessionId: 's1', action: 'click', element: 'Save', confirmId: 'c1' };
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: false, confirmRequest }),
  { enabled: true, stopped: false, showBanner: true, grantRequest: null, confirmRequest },
  'a per-action confirmation stacks on top of the persistent banner',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: true, confirmRequest }),
  { enabled: true, stopped: true, showBanner: false, grantRequest: null, confirmRequest: null },
  'stop() collapses the banner and the dialogs: stop_all already killed the pending confirm, so its approve button must not stay live (review finding)',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: false, stopped: true, grantRequest }),
  { enabled: true, stopped: true, showBanner: false, grantRequest: null, confirmRequest: null },
  'stop() also collapses a pending grant dialog',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: false }),
  { enabled: true, stopped: false, showBanner: true, grantRequest: null, confirmRequest: null },
  'granted and not stopped keeps the non-dismissible banner visible',
);

// ── 明确拒绝后的弹窗频控（评审修复）──────────────────────────────
assert.equal(
  isDeniedRequestSuppressed(Date.now() - 1000, Date.now()),
  true,
  'a request within the cooldown of an explicit deny must be suppressed',
);
assert.equal(
  isDeniedRequestSuppressed(Date.now() - DENY_SUPPRESSION_MS - 1, Date.now()),
  false,
  'after the cooldown a fresh dialog may be shown again',
);
assert.equal(isDeniedRequestSuppressed(undefined, Date.now()), false, 'no deny recorded → never suppressed');
assert.equal(isDeniedRequestSuppressed(Date.now() + 5_000, Date.now()), false, 'clock skew must not suppress forever');

console.log('computer use logic tests passed');
