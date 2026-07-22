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
assert.match(petWindow, /listen\(['"]scheduled_task:run_updated['"]/);
assert.match(petWindow, /className="pet-activity pet-activity-scheduled"/);
assert.match(petWindow, /scheduledRun:\s*scheduledNotice/);
assert.match(petWindow, /定时任务已完成/);
assert.match(petWindow, /formatScheduledNoticeBody\(scheduledNotice\)/);
assert.match(petCss, /\.pet-activity-scheduled\s*\{/);

assert.match(rust, /pub struct PetScheduledRunNavigation/);
assert.match(rust, /pub scheduled_run:\s*Option<PetScheduledRunNavigation>/);
assert.match(rust, /scheduled_run:\s*Option<PetScheduledRunNavigation>/);

assert.match(main, /request\.scheduled_run\s*\|\|\s*request\.scheduledRun/);
assert.match(main, /bridge\.openScheduledRunChat/);
assert.match(main, /setCurrentView\(['"]scheduled['"]\)/);
assert.match(main, /pet:scheduled_notice_opened/);
assert.match(main, /pet:scheduled_notice_open_failed/);

console.log('pet scheduled notice contract tests passed');
