#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { dictZh } from '../src/shared/i18n/zh.js';
import { dictEn } from '../src/shared/i18n/en.js';
import { dictJa } from '../src/shared/i18n/ja.js';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(testDir, '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

const pillSource = read('src', 'features', 'voice-composer', 'VoiceRecordingPill.jsx');
const controlsSource = read('src', 'features', 'voice-composer', 'VoiceComposerControls.jsx');
const popoverSource = read('src', 'features', 'voice-composer', 'VoiceAsrPopover.jsx');
const hookSource = read('src', 'features', 'voice-composer', 'useVoicePillPresence.js');
const baseCss = read('src', 'styles', 'base.css');

// The stop hint names both modifier spellings (macOS Option, elsewhere Alt)
// because React must not sniff the OS and consumes no OS-identity capability
// today (get_platform_capabilities has an `os` field, but no React reader).
for (const [lang, dict, prefix] of [
  ['zh', dictZh, '再按'],
  ['en', dictEn, 'Press'],
  ['ja', dictJa, 'もう一度'],
]) {
  const hint = dict.voiceStopHint;
  assert.equal(typeof hint, 'string', `${lang} must define voiceStopHint`);
  assert.match(hint, new RegExp(prefix), `${lang} voiceStopHint must be its own copy`);
  assert.match(hint, /Alt/, `${lang} voiceStopHint must name Alt`);
  assert.match(hint, /Option/, `${lang} voiceStopHint must name Option`);
}

// Enter/exit/popover animation classes exist and the exit rule stays after the
// enter rule so an exit interrupting an enter still wins the cascade.
assert.match(baseCss, /@keyframes voicePopIn/);
assert.match(baseCss, /@keyframes voicePopOut/);
assert.match(baseCss, /\.voice-pop-in/);
assert.match(baseCss, /\.voice-pop-out/);
assert.ok(baseCss.indexOf('.voice-pop-out {') > baseCss.indexOf('.voice-pop-in {'), 'exit rule must come after enter rule');
assert.match(baseCss, /@keyframes voicePillLive/);
assert.match(baseCss, /@media \(prefers-reduced-motion: reduce\)[\s\S]*voice-pop-in[\s\S]*voice-pop-out[\s\S]*voice-pill-live/, 'reduced motion must disable the voice animations');

// Exit timing contract: the unmount delay must cover the exit animation
// (0.14s) so the fade completes before the pill leaves the tree.
const exitMs = Number(hookSource.match(/VOICE_PILL_EXIT_MS = (\d+)/)?.[1]);
const exitAnimMs = Number(baseCss.match(/\.voice-pop-out \{ animation: voicePopOut 0\.(\d\d)s/)?.[1]) * 10;
assert.ok(exitAnimMs > 0, 'exit animation duration must be parseable');
assert.ok(exitMs >= exitAnimMs, `unmount delay ${exitMs}ms must cover the ${exitAnimMs}ms exit animation`);

// The pill only animates out while mounted via the presence hook: closing
// freezes pointer events, and recording keeps the live halo plus the hint.
assert.match(pillSource, /closing = false/, 'closing prop must default to false so direct renders stay enter-only');
assert.match(pillSource, /closing \? 'voice-pop-out pointer-events-none' : 'pointer-events-auto'/, 'closing must apply the exit class and drop pointer events');
assert.match(pillSource, /recording \? 'voice-pill-live' : ''/, 'recording must enable the live halo');
assert.match(pillSource, /copy\.voiceStopHint/, 'recording bubble must show the stop hint');
assert.doesNotMatch(pillSource, /voiceModeLabel/, 'the dead mode-label branch must stay removed');

// The pill layer keeps the pill mounted through the exit window and renders a
// frozen last-active snapshot, not the terminal status.
assert.match(controlsSource, /useVoicePillPresence\(shouldShow, voiceInput\)/);
assert.match(controlsSource, /presence\.snapshot \|\| voiceInput/, 'exit must render the frozen snapshot');
assert.match(controlsSource, /closing=\{presence\.closing\}/);

// The ASR download popover shares the same enter animation.
assert.match(popoverSource, /voice-pop-in/);

console.log('voice_pill_animation: ok');
