#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridge = readFileSync(path.join(here, '..', 'src', 'tauri-bridge.js'), 'utf8');
const main = readFileSync(path.join(here, '..', 'src', 'main.jsx'), 'utf8');
const petWindow = readFileSync(
  path.join(here, '..', 'src', 'features', 'pet', 'PetWindow.jsx'),
  'utf8',
);
const css = readFileSync(
  path.join(here, '..', 'src', 'features', 'pet', 'pet.css'),
  'utf8',
);

assert.match(bridge, /async function sendMessageToSession\(sessionId, text, meta\)/);
assert.match(bridge, /await ensureSessionBufferLoaded\(sid\)/);
assert.match(bridge, /if \(isBusyFor\(sid\)\)/);
assert.match(bridge, /runSyncOnSession\(sid/);
assert.match(bridge, /doSendFor\(sid, content, content, \[\]/);
assert.match(bridge, /sendMessageToSession:\s*sendMessageToSession/);
assert.match(main, /listen\(['"]pet:reply_pending['"]/);
assert.match(main, /invoke\(['"]take_pet_reply['"]\)/);
assert.match(main, /bridge\.sendMessageToSession\(sid, text\)/);
assert.match(main, /result\.completion\.then/);
assert.match(main, /pet:reply_accepted/);
assert.match(main, /pet:reply_failed/);
assert.match(petWindow, /useReducer\(\s*petCardUiReducer/);
assert.match(petWindow, /invoke\(['"]queue_pet_reply['"]/);
assert.match(petWindow, /listen\(['"]pet:reply_accepted['"]/);
assert.match(petWindow, /listen\(['"]pet:reply_failed['"]/);
assert.match(petWindow, /className="pet-activity-close"/);
assert.match(petWindow, /className="pet-activity-expand"/);
assert.match(petWindow, /className="pet-activity-expand-icon"/);
assert.match(petWindow, /className="pet-activity-reply"/);
assert.match(petWindow, /className="pet-activity-open"/);
assert.match(petWindow, /className="pet-activity-title-row"/);
assert.match(petWindow, /className="pet-activity-body-row"/);
assert.match(petWindow, /className="pet-reply-composer"/);
assert.match(petWindow, /import \{ renderPetMarkdown \} from ['"]\.\/pet-markdown\.js['"]/);
assert.match(petWindow, /dangerouslySetInnerHTML=\{\{ __html: renderPetMarkdown\(source\) \}\}/);
assert.match(petWindow, /event\.key === ['"]Enter['"] && !event\.shiftKey/);
assert.match(petWindow, /event\.key === ['"]Escape['"]/);
assert.match(petWindow, /Math\.min\(element\.scrollHeight, 52\)/);
assert.match(petWindow, /className="pet-reply-send-icon"/);
assert.doesNotMatch(
  petWindow,
  /ResizeObserver/,
  'card-local changes must not resize the native pet window',
);
assert.match(petWindow, /const PET_ACTIVITY_WINDOW_HEIGHT = 260;/);
assert.match(petWindow, /activityHeight:/);
assert.match(css, /-webkit-line-clamp:\s*2/);
assert.match(
  css,
  /\.pet-activities\s*\{[^}]*overflow-y:\s*auto[^}]*background:\s*transparent[^}]*border:\s*0[^}]*box-shadow:\s*none/,
  'the shared outer panel scrolls while remaining visually transparent',
);
assert.match(css, /\.pet-activities-tray\s*\{[^}]*gap:\s*8px/);
assert.doesNotMatch(css, /\.pet-activity-shell\s*\+\s*\.pet-activity-shell\s*\{[^}]*border-top:/);
assert.match(
  css,
  /\.pet-activity\s*\{[^}]*border:\s*1px solid rgba\(33, 38, 45, 0\.09\)[^}]*border-radius:\s*16px[^}]*background:\s*rgba\(255, 255, 255, 0\.98\)[^}]*box-shadow:/,
  'each conversation keeps the original independent bubble appearance',
);
assert.match(
  css,
  /\.pet-activity\.is-expanded\s+\.pet-activity-body\s*\{[^}]*-webkit-line-clamp:\s*unset[^}]*max-height:\s*none[^}]*overflow:\s*visible/,
);
assert.match(css, /\.pet-activity-body-expanded\s*\{[^}]*overflow:\s*visible/);
assert.doesNotMatch(
  css,
  /\.pet-activity-body-expanded\s*\{[^}]*overflow-y:\s*auto/,
  'expanded rows must rely on outer-panel scrolling',
);
assert.match(css, /\.pet-activity-body\s*>\s*:first-child\s*\{[^}]*margin-top:\s*0/);
assert.match(css, /\.pet-activity-body\s*>\s*:last-child\s*\{[^}]*margin-bottom:\s*0/);
assert.match(css, /\.pet-activity:hover[\s\S]*\.pet-activity-close/);
assert.match(css, /\.pet-activity:hover[\s\S]*\.pet-activity-reply/);
assert.match(
  css,
  /\.pet-activity-main\s*\{[^}]*padding:\s*7px 14px/,
  'activity text keeps the full card width instead of reserving a right rail',
);
assert.doesNotMatch(css, /\.pet-activity-main\s*\{[^}]*padding:[^;}]*48px/);
assert.match(
  css,
  /\.pet-activity-title-row\s*\{[^}]*display:\s*grid[^}]*grid-template-columns:\s*minmax\(0, 1fr\) 22px/,
);
assert.match(
  css,
  /\.pet-activity-expand\s*\{[^}]*position:\s*relative[^}]*top:\s*-2px[^}]*background:\s*transparent[^}]*color:\s*#858b94[^}]*opacity:\s*0/,
  'the expand control sits two pixels above the title-row center',
);
assert.doesNotMatch(
  css,
  /\.pet-activity\.is-expanded\s+\.pet-activity-expand\s*\{[^}]*transform:/,
  'expanding must not rotate the button tooltip together with the arrow',
);
assert.match(
  css,
  /\.pet-activity\.is-expanded\s+\.pet-activity-expand-icon\s*\{[^}]*transform:\s*rotate\(90deg\)/,
);
assert.match(
  css,
  /\.pet-activity:hover\s+\.pet-activity-status\s*\{[^}]*opacity:\s*0/,
);
assert.match(
  css,
  /\.pet-activity:hover\s+\.pet-activity-expand\s*\{[^}]*opacity:\s*1[^}]*pointer-events:\s*auto/,
);
assert.match(
  css,
  /\.pet-activity-expand:hover,[\s\S]*?\.pet-activity-expand:focus-visible\s*\{[^}]*background:\s*#4c5158[^}]*color:\s*#fff/,
);
assert.match(
  css,
  /\.pet-activity-reply\s*\{[^}]*position:\s*absolute[^}]*right:\s*8px[^}]*bottom:\s*5px[^}]*top:\s*auto[^}]*height:\s*20px/,
  'reply stays pinned to the bubble bottom-right corner',
);
assert.match(
  css,
  /\.pet-activity-reply:hover,[\s\S]*?\.pet-activity-reply:focus-visible\s*\{[^}]*background:\s*#4c5158[^}]*color:\s*#fff/,
  'reply should turn dark on hover and keyboard focus',
);
assert.doesNotMatch(css, /\.pet-activity-reply\s*\{[^}]*top:\s*(?:0|26px)/);
assert.doesNotMatch(
  css,
  /\.pet-activity:focus-within\s+\.pet-activity-(?:close|reply)/,
  'card actions should follow pointer hover instead of sticking on the previously focused card',
);
assert.match(
  css,
  /\.pet-reply-composer\s*\{[^}]*min-height:\s*28px[^}]*padding:\s*2px 4px 2px 9px/,
  'the inline reply composer should stay visually slim',
);
assert.match(css, /\.pet-reply-composer textarea\s*\{[^}]*min-height:\s*18px[^}]*max-height:\s*52px/);
assert.match(
  css,
  /\.pet-reply-composer\s*>\s*button\s*\{[^}]*width:\s*22px[^}]*height:\s*22px[^}]*border-radius:\s*7px[^}]*background:\s*transparent/,
);
assert.match(
  css,
  /\.pet-reply-composer\s*>\s*button:not\(:disabled\):hover\s*\{[^}]*background:\s*#343941[^}]*color:\s*#fff/,
);
assert.match(css, /\.pet-reply-send-icon\s*\{[^}]*width:\s*14px[^}]*height:\s*14px/);

console.log('pet reply contract tests passed');
