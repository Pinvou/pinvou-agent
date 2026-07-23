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

assert.strictEqual(logic.viewModel(null, null).visible, false);

let vmIdle = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'linux' }
);
assert.strictEqual(vmIdle.visible, true);
assert.strictEqual(vmIdle.version, '1.2.0');
assert.strictEqual(vmIdle.label, '升级并重启');
assert.strictEqual(vmIdle.action, 'download');
assert.strictEqual(vmIdle.disabled, false);
assert.strictEqual(vmIdle.restartAfterInstall, true);

let vmWindowsIdle = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'windows' }
);
assert.strictEqual(vmWindowsIdle.label, '下载并安装');
assert.strictEqual(vmWindowsIdle.restartAfterInstall, false);

let vmCustomLabels = logic.viewModel(
  { appVersion: '1.1.0' },
  { available: true, latest_version: '1.2.0', platform: 'linux' },
  null,
  { downloadInstallRestart: 'Update & Restart' }
);
assert.strictEqual(vmCustomLabels.label, 'Update & Restart');

let vmDownloading = logic.viewModel(
  { updateDownloading: true, updateProgress: 42 },
  { available: true, latest_version: '1.2.0' }
);
assert.strictEqual(vmDownloading.label, '下载中 42%');
assert.strictEqual(vmDownloading.action, 'none');
assert.strictEqual(vmDownloading.disabled, true);

let vmInstalling = logic.viewModel(
  { updateDownloading: true, updateProgress: 100 },
  { available: true, latest_version: '1.2.0' }
);
assert.strictEqual(vmInstalling.label, '安装中...');
assert.strictEqual(vmInstalling.action, 'none');

let vmReady = logic.viewModel(
  { updateReady: true },
  { available: true, latest_version: '1.2.0' }
);
assert.strictEqual(vmReady.label, '立即重启');
assert.strictEqual(vmReady.action, 'restart');
assert.strictEqual(vmReady.disabled, false);

let vmWindowsReady = logic.viewModel(
  { updateReady: true },
  { available: true, latest_version: '1.2.0', platform: 'windows' }
);
assert.strictEqual(vmWindowsReady.label, '安装器已启动');
assert.strictEqual(vmWindowsReady.action, 'none');
assert.strictEqual(vmWindowsReady.disabled, true);

let vmError = logic.viewModel(
  { updateError: 'sha256 failed' },
  { available: true, latest_version: '1.2.0' }
);
assert.strictEqual(vmError.error, 'sha256 failed');

console.log('update_notice_logic: ok');
