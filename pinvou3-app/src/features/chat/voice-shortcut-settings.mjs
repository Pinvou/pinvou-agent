const VOICE_SHORTCUT_ENABLED_KEY = 'pinvou_voice_shortcut_enabled_v1';
const VOICE_SHORTCUT_INTRO_SEEN_KEY = 'pinvou_voice_shortcut_intro_seen_v8';
const VOICE_POSTPROCESS_ENABLED_KEY = 'pinvou_voice_postprocess_enabled_v1';
const VOICE_SHORTCUT_SETTINGS_EVENT = 'pinvou:voice-shortcut-settings';

function readBooleanSetting(key, fallback = false, storage = globalThis.localStorage) {
  try {
    const value = storage && storage.getItem(key);
    if (value === 'true') return true;
    if (value === 'false') return false;
  } catch (_) {}
  return fallback;
}

function writeBooleanSetting(key, value, storage = globalThis.localStorage) {
  try {
    if (storage) storage.setItem(key, value ? 'true' : 'false');
  } catch (_) {}
}

function voiceShortcutEnabled(storage) {
  return readBooleanSetting(VOICE_SHORTCUT_ENABLED_KEY, false, storage);
}

function voiceShortcutIntroSeen(storage) {
  return readBooleanSetting(VOICE_SHORTCUT_INTRO_SEEN_KEY, false, storage);
}

function setVoiceShortcutEnabled(value, storage) {
  writeBooleanSetting(VOICE_SHORTCUT_ENABLED_KEY, !!value, storage);
  try {
    globalThis.dispatchEvent(new CustomEvent(VOICE_SHORTCUT_SETTINGS_EVENT, {
      detail: { enabled: !!value },
    }));
  } catch (_) {}
}

function setVoiceShortcutIntroSeen(value, storage) {
  writeBooleanSetting(VOICE_SHORTCUT_INTRO_SEEN_KEY, !!value, storage);
}

// 智能整理(纠错+结构化)开关,默认开启。注意:tauri 桥(经典脚本)无法 import
// 本模块,它按同一 key 直读 localStorage(见 platform/tauri/bridge/voice.js),
// 改 key 时必须两侧同步。
function voicePostprocessEnabled(storage) {
  return readBooleanSetting(VOICE_POSTPROCESS_ENABLED_KEY, true, storage);
}

function setVoicePostprocessEnabled(value, storage) {
  writeBooleanSetting(VOICE_POSTPROCESS_ENABLED_KEY, !!value, storage);
  try {
    globalThis.dispatchEvent(new CustomEvent(VOICE_SHORTCUT_SETTINGS_EVENT, {
      detail: { postprocessEnabled: !!value },
    }));
  } catch (_) {}
}

export {
  VOICE_POSTPROCESS_ENABLED_KEY,
  VOICE_SHORTCUT_ENABLED_KEY,
  VOICE_SHORTCUT_INTRO_SEEN_KEY,
  VOICE_SHORTCUT_SETTINGS_EVENT,
  readBooleanSetting,
  setVoicePostprocessEnabled,
  setVoiceShortcutEnabled,
  setVoiceShortcutIntroSeen,
  voicePostprocessEnabled,
  voiceShortcutEnabled,
  voiceShortcutIntroSeen,
};
