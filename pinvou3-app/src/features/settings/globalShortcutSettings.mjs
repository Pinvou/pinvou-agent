// 全局唤醒快捷键（set_global_shortcut_config）的纯前端预校验。
// Rust 端仍是权威校验：这里只挡住明显不可能的组合，避免必然失败的 IPC 往返。
// 有意收紧（非遗漏）：Rust 解析器也接受无修饰键的单键组合（如 F9），
// 但系统级抢占单个按键过于激进，产品策略上要求至少一个修饰键，故一并拦下。
// 本模块不读写 localStorage、不发事件——bs.settings.settings 与命令响应是唯一
// 状态源（与 chat/voice-shortcut-settings.mjs 的本地镜像方案刻意不同）。
const GLOBAL_SHORTCUT_DEFAULT_ACCELERATOR = 'CmdOrCtrl+Alt+P';

// Tauri accelerator 的修饰键词表（小写比较）；CmdOrCtrl / CommandOrControl
// 是合法的跨平台修饰键别名。meta/opt 虽非 Rust 修饰键别名，仍放行预校验：
// 放行只是走一次 Rust 权威校验并得到本地化 invalid 提示，拦下反而会让
// Save 永久灰死且无解释。
const GLOBAL_SHORTCUT_MODIFIER_KEYS = new Set([
  'ctrl', 'control', 'cmd', 'command', 'cmdorctrl', 'commandorcontrol', 'super', 'meta',
  'alt', 'option', 'opt', 'shift',
]);

function normalizeGlobalShortcutDraft(value) {
  return String(value == null ? '' : value).trim();
}

// 展示形态 → Rust 可解析串：'Ctrl/Cmd' 是展示专用的平台中立写法（与 README
// 措辞一致），Rust 解析不了 "CTRL/CMD"——上送与预校验前折回 CmdOrCtrl 别名，
// 让"只改尾键"的编辑（Ctrl/Cmd+Alt+O）也能通过 Rust 校验而不是必然报错。
// 其余 token 原样保留（大小写/空白本就由 Rust 归一化）。
function globalShortcutDraftToRust(value) {
  return normalizeGlobalShortcutDraft(value)
    .split('+')
    .map((part) => (part.trim().toLowerCase() === 'ctrl/cmd' ? 'CmdOrCtrl' : part.trim()))
    .join('+');
}

// 折叠形态：去空白 + 小写。仅用于"同一组合的别名拼写"判定（Rust 归一化会把
//  control+alt+KeyP / Ctrl + Alt + P 折到同一规范串），不用于上送值。
function foldGlobalShortcutDraft(value) {
  return normalizeGlobalShortcutDraft(value).replace(/\s+/g, '').toLowerCase();
}

// 形如 modifier+...+key：至少两段、每段非空、除末段外至少含一个修饰键，
// 且末段不能是纯修饰键（只有修饰键的组合无法注册）。判定前先把展示专用
// 写法折回 Rust 可解析串（Ctrl/Cmd → CmdOrCtrl），与上送路径同构。
function isPlausibleGlobalShortcutAccelerator(value) {
  const parts = globalShortcutDraftToRust(value).split('+').map(part => part.trim().toLowerCase());
  if (parts.length < 2) return false;
  if (!parts.every(part => part.length > 0)) return false;
  const mainKey = parts[parts.length - 1];
  if (GLOBAL_SHORTCUT_MODIFIER_KEYS.has(mainKey)) return false;
  return parts.slice(0, -1).some(part => GLOBAL_SHORTCUT_MODIFIER_KEYS.has(part));
}

// 归一化修饰键 → 展示用修饰键名（小写比较；ctrl 走下方兜底规则同样得到 Ctrl）。
// cmdorctrl 显示为 Ctrl/Cmd：别名在首次保存前不会被 Rust 归一化，平台中立写法
// 避免 Win/Linux 上把默认组合误显示成 Cmd（与 README 的 Ctrl/Cmd 措辞一致）。
const GLOBAL_SHORTCUT_DISPLAY_TOKENS = new Map([
  ['control', 'Ctrl'],
  ['cmd', 'Cmd'], ['command', 'Cmd'], ['cmdorctrl', 'Ctrl/Cmd'], ['meta', 'Cmd'], ['super', 'Cmd'],
  ['alt', 'Alt'], ['option', 'Alt'], ['opt', 'Alt'],
  ['shift', 'Shift'],
]);

function capitalizeFirstLetter(token) {
  return token.charAt(0).toUpperCase() + token.slice(1);
}

// 机器规范串 → 人类可读展示形态：control+alt+KeyP → Ctrl+Alt+P、KeyX→X、
// DigitX→X；Numpad*/Keypad* 不做前缀剥离（先于 Key 规则判断，避免 Key+pad
// 误剥）；其余 token（F 键、方向键、Media*、Minus/Equal 等命名键）原样
// 保留、仅首字母大写。仅用于展示与脏检查：输入框持有展示形态，保存时仍把
// 草稿原样上送（Rust 端可解析友好输入并归一化）。空输入返回 ''，未知 token
// 原样透传。
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
