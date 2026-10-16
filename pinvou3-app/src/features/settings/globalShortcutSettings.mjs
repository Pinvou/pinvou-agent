// Pure client-side pre-validation for the global shortcut
// (set_global_shortcut_config). Rust stays the authoritative parser: this
// only filters combinations that can never register, so an inevitably
// failing IPC round-trip is skipped. Intentionally stricter (not a gap):
// the Rust parser also accepts single-key combos without modifiers (e.g.
// F9), but grabbing a lone system-wide key is too aggressive — product
// policy requires at least one modifier, so those are rejected here too.
// This module never touches localStorage and emits no events —
// bs.settings.settings and the command response are the only state sources
// (deliberately unlike the local-mirror approach of
// chat/voice-shortcut-settings.mjs).
const GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR = 'CmdOrCtrl+Alt+P';

// Modifier vocabulary of Tauri accelerators (compared lowercase);
// CmdOrCtrl / CommandOrControl are legal cross-platform modifier aliases.
// meta/opt are not Rust modifier aliases but still pass pre-validation:
// passing just costs one authoritative Rust check with a localized invalid
// notice, while rejecting would leave Save permanently disabled with no
// explanation.
const GLOBAL_SHORTCUT_MODIFIER_KEYS = new Set([
  'ctrl', 'control', 'cmd', 'command', 'cmdorctrl', 'commandorcontrol', 'super', 'meta',
  'alt', 'option', 'opt', 'shift',
]);

function normalizeGlobalShortcutDraft(value) {
  return String(value == null ? '' : value).trim();
}

// Display form → Rust-parseable string: 'Ctrl/Cmd' is a display-only
// platform-neutral spelling (matches the README wording) that the Rust
// parser cannot read ("CTRL/CMD") — fold it back to the CmdOrCtrl alias
// before pre-validation and submit, so an edit of just the trailing key
// (Ctrl/Cmd+Alt+O) passes Rust validation instead of guaranteeing an error.
// Every other token passes through untouched (case/whitespace are already
// normalized by Rust).
function globalShortcutDraftToRust(value) {
  return normalizeGlobalShortcutDraft(value)
    .split('+')
    .map((part) => (part.trim().toLowerCase() === 'ctrl/cmd' ? 'CmdOrCtrl' : part.trim()))
    .join('+');
}

// Folded form: whitespace stripped + lowercased. Only for "same combo,
// alias spelling" decisions (Rust normalization folds control+alt+KeyP /
// Ctrl + Alt + P onto one canonical string); never for the submitted value.
function foldGlobalShortcutDraft(value) {
  return normalizeGlobalShortcutDraft(value).replace(/\s+/g, '').toLowerCase();
}

// Shape modifier+...+key: at least two parts, every part non-empty, at
// least one modifier outside the last part, and the last part must not be
// a bare modifier (a modifiers-only combo cannot register). The display-only
// spelling folds back to the Rust-parseable alias first (Ctrl/Cmd →
// CmdOrCtrl), mirroring the submit path.
function isPlausibleGlobalShortcutAccelerator(value) {
  const parts = globalShortcutDraftToRust(value).split('+').map(part => part.trim().toLowerCase());
  if (parts.length < 2) return false;
  if (!parts.every(part => part.length > 0)) return false;
  const mainKey = parts[parts.length - 1];
  if (GLOBAL_SHORTCUT_MODIFIER_KEYS.has(mainKey)) return false;
  return parts.slice(0, -1).some(part => GLOBAL_SHORTCUT_MODIFIER_KEYS.has(part));
}

// Normalized modifier → display modifier name (lowercase comparison; ctrl
// reaches the fallback rule below and yields Ctrl all the same).
// cmdorctrl displays as Ctrl/Cmd: the alias is not normalized by Rust until
// the first save, and the platform-neutral spelling avoids mislabeling the
// default combo as Cmd on Windows/Linux (matches the README's Ctrl/Cmd
// wording).
const GLOBAL_SHORTCUT_DISPLAY_TOKENS = new Map([
  ['control', 'Ctrl'],
  ['cmd', 'Cmd'], ['command', 'Cmd'], ['cmdorctrl', 'Ctrl/Cmd'], ['meta', 'Cmd'], ['super', 'Cmd'],
  ['alt', 'Alt'], ['option', 'Alt'], ['opt', 'Alt'],
  ['shift', 'Shift'],
]);

function capitalizeFirstLetter(token) {
  return token.charAt(0).toUpperCase() + token.slice(1);
}

// Machine-canonical string → human-readable display form:
// control+alt+KeyP → Ctrl+Alt+P, KeyX→X, DigitX→X; Numpad*/Keypad* never
// get a prefix stripped (checked before the Key rule so Key+pad is not
// mangled); every other token (F-keys, arrows, Media*, Minus/Equal and
// other named keys) passes through with only its first letter capitalized.
// Display and dirty-check only: the input holds the display form, and the
// draft is still submitted as-is on save (Rust parses friendly input and
// normalizes). Empty input returns ''; unknown tokens pass through.
function formatGlobalShortcutForDisplay(value) {
  const raw = normalizeGlobalShortcutDraft(value);
  if (!raw) return '';
  return raw.split('+').map((rawToken) => {
    const token = rawToken.trim();
    if (!token) return token;
    const lower = token.toLowerCase();
    if (GLOBAL_SHORTCUT_DISPLAY_TOKENS.has(lower)) return GLOBAL_SHORTCUT_DISPLAY_TOKENS.get(lower);
    if (lower.startsWith('numpad') || lower.startsWith('keypad')) return capitalizeFirstLetter(token);
    if (lower.startsWith('key') && lower.length > 3) return capitalizeFirstLetter(token.slice(3));
    if (lower.startsWith('digit') && lower.length > 5) return capitalizeFirstLetter(token.slice(5));
    return capitalizeFirstLetter(token);
  }).join('+');
}

export {
  GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR,
  foldGlobalShortcutDraft,
  formatGlobalShortcutForDisplay,
  globalShortcutDraftToRust,
  isPlausibleGlobalShortcutAccelerator,
  normalizeGlobalShortcutDraft,
};
