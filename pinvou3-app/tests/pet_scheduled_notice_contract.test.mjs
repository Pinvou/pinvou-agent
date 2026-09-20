import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const petWindow = readFileSync(new URL('../src/features/pet/PetWindow.jsx', import.meta.url), 'utf8');
const petCss = readFileSync(new URL('../src/features/pet/pet.css', import.meta.url), 'utf8');
const main = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');
const rust = readFileSync(new URL('../src-tauri/src/features/pet/pet_window.rs', import.meta.url), 'utf8');

assert.match(petWindow, /isScheduledSessionPayload/);
assert.match(
  petWindow,
  /\.filter\(\(session\) => !isScheduledSessionPayload\(session\)\)[\s\S]{0,120}?applyActivitySnapshot\(/,
  'scheduled snapshot sessions must not become ordinary activity cards',
);
// scheduled_task:run_updated 不再由 pet 窗口订阅：file_watcher 的 payload 恒为空，
// 任何读取 run.status 的分支都是死路；完成通知刷新由 chat:done 路径（PET_EVENTS 内
// isScheduledSessionPayload 分支）承担，故此处的旧订阅契约一并移除。
assert.match(petWindow, /className="pet-activity pet-activity-scheduled"/);
assert.match(petWindow, /scheduledRun:\s*scheduledNotice/);
assert.match(petWindow, /petCopy\.scheduledDone/);
assert.match(petWindow, /formatScheduledNoticeBody\(scheduledNotice, t\.langTag, petCopy\.done\)/);
assert.match(petCss, /\.pet-activity-scheduled\s*\{/);

assert.match(rust, /pub struct PetScheduledRunNavigation/);
assert.match(rust, /pub scheduled_run:\s*Option<PetScheduledRunNavigation>/);

assert.match(main, /request\.scheduled_run\s*\|\|\s*request\.scheduledRun/);
assert.match(main, /bridge\.scheduled\.openScheduledRunChat/);
assert.match(main, /setCurrentView\(['"]scheduled['"]\)/);
assert.match(main, /pet:scheduled_notice_opened/);
assert.match(main, /pet:scheduled_notice_open_failed/);

console.log('pet scheduled notice contract tests passed');
