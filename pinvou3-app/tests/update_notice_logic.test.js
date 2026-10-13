#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'updater', 'update-notice-logic.js');
const code = fs.readFileSync(logicPath, 'utf8');
const ctx = {
  window: {},
  URLSearchParams,
};
ctx.globalThis = ctx;
vm.createContext(ctx);
vm.runInContext(code, ctx, { filename: logicPath });

const logic = ctx.window.UpdateNoticeLogic;

assert.strictEqual(logic.previewEnabled({ search: '?mockUpdate=1' }), true);
assert.strictEqual(logic.previewEnabled({ search: '?mockUpdate=0' }), false);
assert.strictEqual(logic.previewEnabled({ search: '' }), false);

assert.strictEqual(logic.updateInfoFor(null), null);
assert.deepStrictEqual(
  logic.updateInfoFor({ updateInfo: { available: true, latest_version: '2.0.0' } }),
  { available: true, latest_version: '2.0.0' }
);
assert.strictEqual(
  logic.updateInfoFor({ updateInfo: { available: false, latest_version: '2.0.0' } }),
  null
);
assert.strictEqual(logic.updateInfoFor(null, { preview: true }).latest_version, '1.2.0');

assert.strictEqual(logic.versionKey(null), '');
assert.strictEqual(logic.versionKey({ latest_version: '2.0.0', current_version: '1.0.0' }), '2.0.0');
assert.strictEqual(logic.versionKey({ current_version: '1.0.0' }), '1.0.0');

// vm-realm objects fail deepStrictEqual against test-realm literals; compare keys instead.
assert.deepStrictEqual(Object.keys(logic.viewModel(null, null)), []);

const vmIdle = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'linux' }
);
assert.strictEqual(vmIdle.version, '1.2.0');
assert.strictEqual(vmIdle.label, '升级并重启');
assert.strictEqual(vmIdle.action, 'download');
assert.strictEqual(vmIdle.disabled, false);

const vmWindowsIdle = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'windows' }
);
assert.strictEqual(vmWindowsIdle.label, '下载并安装');

const vmCustomLabels = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'linux' },
  null,
  { downloadInstallRestart: 'Update & Restart' }
);
assert.strictEqual(vmCustomLabels.label, 'Update & Restart');

const vmDownloading = logic.viewModel(
  { updateDownloading: true, updateProgress: 42 },
  { available: true, latest_version: '1.2.0' }
);
assert.strictEqual(vmDownloading.label, '下载中 42%');
assert.strictEqual(vmDownloading.action, 'none');
assert.strictEqual(vmDownloading.disabled, true);

const vmInstalling = logic.viewModel(
  { updateDownloading: true, updateProgress: 100 },
  { available: true, latest_version: '1.2.0' }
);
assert.strictEqual(vmInstalling.label, '安装中...');
assert.strictEqual(vmInstalling.action, 'none');

const vmReady = logic.viewModel(
  { updateReady: true },
  { available: true, latest_version: '1.2.0', platform: 'linux' }
);
assert.strictEqual(vmReady.label, '立即重启');
assert.strictEqual(vmReady.action, 'restart');
assert.strictEqual(vmReady.disabled, false);

const vmWindowsReady = logic.viewModel(
  { updateReady: true },
  { available: true, latest_version: '1.2.0', platform: 'windows' }
);
assert.strictEqual(vmWindowsReady.label, '安装器已启动');
assert.strictEqual(vmWindowsReady.action, 'none');
assert.strictEqual(vmWindowsReady.disabled, true);

// macOS 与 Linux 同型:后端 Ok(false) → 前端自动 restartApp(app.restart() 按路径 exec 新
// bundle,inode 语义与 Linux 一致)。与 Windows 启动外部 MSI 安装器不同。因此 macOS ready
// resolves through the restart branch (label=立即重启, action=restart, disabled=false).
const vmMacReady = logic.viewModel(
  { updateReady: true },
  { available: true, latest_version: '1.2.0', platform: 'macos' }
);
assert.strictEqual(vmMacReady.label, '立即重启');
assert.strictEqual(vmMacReady.action, 'restart');
assert.strictEqual(vmMacReady.disabled, false);

// macOS idle 态:restartAfterInstall=true(mac 与 linux 同型),idle 应走"升级并重启"
// 而非"下载并安装"(后者是 Windows/无 restartAfterInstall 的默认)。
const vmMacIdle = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'macos' }
);
assert.strictEqual(vmMacIdle.label, '升级并重启');
assert.strictEqual(vmMacIdle.action, 'download');
assert.strictEqual(vmMacIdle.disabled, false);

// The raw backend error (may carry request details, often untranslated) must
// never reach the view model: the localized hint does, the raw string stays on
// bridge state for diagnostics.
const vmError = logic.viewModel(
  { updateError: 'sha256 failed' },
  { available: true, latest_version: '1.2.0' },
  '1.1.0',
  { updateInstallFailed: 'Update failed' }
);
assert.strictEqual(vmError.error, 'Update failed');
assert.strictEqual(
  logic.viewModel(
    { updateError: 'sha256 failed' },
    { available: true, latest_version: '1.2.0' }
  ).error,
  '更新失败',
  'labels without the key fall back to the default hint, never the raw error'
);

// unknown platform falls back to download-and-install (no auto-restart):
// only linux/macos support auto-restart (see update-notice-logic.js), so an
// unknown platform (e.g. a future freebsd) must not assume it can restart:
// the idle state resolves to download-and-install rather than
// upgrade-and-restart (action='download', not 'restart').
const vmUnknownIdle = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'freebsd' }
);
assert.strictEqual(vmUnknownIdle.label, '下载并安装');
assert.strictEqual(vmUnknownIdle.action, 'download');
assert.strictEqual(vmUnknownIdle.disabled, false);

console.log('update_notice_logic: ok');
