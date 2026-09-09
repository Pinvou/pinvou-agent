import assert from 'node:assert/strict';
import {
  computerUseConsentView,
  extractComputerUseScreenshotPath,
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
  { enabled: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'a missing slice hides every surface',
);
assert.deepEqual(
  computerUseConsentView({ enabled: false, granted: true, grantRequest: { sessionId: 's1' }, confirmRequest: { confirmId: 'c1' } }),
  { enabled: false, showBanner: false, grantRequest: null, confirmRequest: null },
  'feature toggle off: no banner, no dialogs even with stale requests',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: false, stopped: false }),
  { enabled: true, showBanner: false, grantRequest: null, confirmRequest: null },
  'enabled but ungranted stays quiet until a request arrives',
);
const grantRequest = { sessionId: 's1' };
assert.deepEqual(
  computerUseConsentView({ enabled: true, grantRequest }),
  { enabled: true, showBanner: false, grantRequest, confirmRequest: null },
  'grant_required surfaces the grant dialog',
);
const confirmRequest = { sessionId: 's1', action: 'click', element: 'Save', confirmId: 'c1' };
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: false, confirmRequest }),
  { enabled: true, showBanner: true, grantRequest: null, confirmRequest },
  'a per-action confirmation stacks on top of the persistent banner',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: true, confirmRequest }),
  { enabled: true, showBanner: false, grantRequest: null, confirmRequest },
  'stop() hides the banner immediately while a pending confirm still shows',
);
assert.deepEqual(
  computerUseConsentView({ enabled: true, granted: true, stopped: false }),
  { enabled: true, showBanner: true, grantRequest: null, confirmRequest: null },
  'granted and not stopped keeps the non-dismissible banner visible',
);

console.log('computer use logic tests passed');
