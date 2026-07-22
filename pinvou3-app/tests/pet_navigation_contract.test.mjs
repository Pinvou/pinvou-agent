import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

function source(relativePath) {
  return readFileSync(new URL(`../${relativePath}`, import.meta.url), 'utf8');
}

const petWindow = source('src/features/pet/PetWindow.jsx');
const petCss = source('src/features/pet/pet.css');
const petInteraction = source('src/features/pet/pet-interaction.js');
const main = source('src/app/main.jsx');
const chatView = source('src/features/chat/ChatView.jsx');
const rustPetWindow = source('src-tauri/src/features/pet/pet_window.rs');
const rustLib = source('src-tauri/src/lib.rs');

assert.match(petWindow, /buildAnimationSequence/);
assert.doesNotMatch(petWindow, /['"]poke['"]/, 'a click must not trigger the jumping row');
assert.match(petWindow, /['"]jumping['"]/, 'hover should use the jumping row');
assert.match(petInteraction, /['"]running-right['"]/);
assert.match(petInteraction, /['"]running-left['"]/);
assert.match(petWindow, /invoke\(['"]open_main_from_pet['"]/);
assert.match(petWindow, /deriveActivities/);
assert.match(petWindow, /className="pet-activity-shell"/);
assert.match(petWindow, /className="pet-activity-open"[\s\S]{0,260}?openMain\(activity\.sessionId\)/);
assert.doesNotMatch(
  petWindow,
  /<button[^>]+className=\{`pet-activity/,
  'the composite card must not nest its close, expand and reply buttons',
);
assert.match(petWindow, /className="pet-character"[\s\S]{0,500}?aria-label=/);
assert.doesNotMatch(
  petWindow,
  /className="pet-character"[\s\S]{0,500}?title=/,
  'the tiny native window clips title tooltips and makes them look like broken bubbles',
);
assert.match(petCss, /\.pet-activity/);
assert.match(petCss, /\.pet-activities\s*\{[\s\S]{0,220}?width:\s*300px/);
assert.match(petCss, /\.pet-activity-title\s*\{[\s\S]{0,220}?font-size:\s*14px/);
assert.match(petCss, /\.pet-activity-body\s*\{[\s\S]{0,220}?font-size:\s*13px/);
assert.match(petWindow, /pet-align-\$\{edgeAlign\}/);
assert.match(petCss, /\.pet-root\.pet-align-left\s*\{/);
assert.match(petCss, /\.pet-root\.pet-align-right\s*\{/);
assert.match(petWindow, /className="pet-character-slot"/);
assert.match(petWindow, /invoke\(['"]set_pet_activity_visible['"]/);
// 右键菜单为窗口内 DOM 浮层(不再 invoke 原生菜单窗口:GB10 malloc 闪退)。
assert.match(petWindow, /const onCharacterContextMenu = \(event\) => \{/);
assert.match(petWindow, /setCtxMenu\(\{ x, y \}\)/);
assert.match(petWindow, /className="pet-context-menu"/);
assert.match(petWindow, /invoke\(['"]set_pet_enabled['"],\s*\{\s*enabled:\s*false\s*\}\)/);
assert.doesNotMatch(petWindow, /invoke\(['"]show_pet_context_menu['"]/);
assert.match(petWindow, /listen\(['"]pet:session_unavailable['"]/);

assert.match(rustPetWindow, /pub async fn open_main_from_pet/);
assert.match(rustPetWindow, /get_webview_window\(['"]main['"]\)/);
assert.match(rustPetWindow, /pet:navigation_pending/);
assert.match(rustPetWindow, /pub async fn take_pet_navigation/);
assert.match(rustPetWindow, /emit_to\(\s*['"]main['"]/);
assert.match(rustPetWindow, /pub async fn set_pet_activity_visible/);
assert.doesNotMatch(rustPetWindow, /PET_MENU_LABEL|show_pet_context_menu|hide_pet_context_menu/);
assert.match(rustLib, /pet_window::open_main_from_pet/);
assert.match(rustLib, /pet_window::take_pet_navigation/);
assert.match(rustLib, /manage\(pet_window::PetNavigationState::default\(\)\)/);
assert.match(rustLib, /pet_window::set_pet_activity_visible/);
assert.doesNotMatch(rustLib, /pet_window::(?:show|hide)_pet_context_menu/);

assert.match(main, /listen\(['"]pet:navigation_pending['"]/);
assert.match(main, /invoke\(['"]take_pet_navigation['"]\)/);
assert.match(main, /addEventListener\(['"]focus['"]/);
assert.match(main, /pet:session_unavailable/);
assert.match(main, /emitTo\(['"]pet['"],\s*name/);
assert.match(main, /pet:activity_snapshot/);
assert.match(main, /focusComposerTick=/);
assert.match(chatView, /focusComposerTick/);
assert.match(chatView, /composerRef\.current\.focus\(\)/);
assert.match(
  chatView,
  /if \(!focusComposerTick\)[\s\S]{0,240}?composerRef\.current\.focus\(\)[\s\S]{0,80}?}, 80\)/,
  'composer focus should wait for the native main-window activation to settle',
);

console.log('pet navigation contract tests passed');
