const VOICE_SHORTCUT_ENABLED_KEY = 'pinvou_voice_shortcut_enabled_v1';
const VOICE_SHORTCUT_INTRO_SEEN_KEY = 'pinvou_voice_shortcut_intro_seen_v8';
const VOICE_POSTPROCESS_ENABLED_KEY = 'pinvou_voice_postprocess_enabled_v1';
const VOICE_SHORTCUT_SETTINGS_EVENT = 'pinvou:voice-shortcut-settings';

function readBooleanSetting(key, fallback = false, storage = localStorage) {
  try {
    const value = storage && storage.getItem(key);
    if (value === 'true') return true;
    if (value === 'false') return false;
  } catch {
    // storage 访问失败（隐私模式 / 注入 storage 抛错）时按 fallback 处理。
  }
  return fallback;
}

function writeBooleanSetting(key, value, storage = localStorage) {
  try {
    if (storage) storage.setItem(key, value ? 'true' : 'false');
  } catch {
    // 写入失败（配额 / 安全限制）不影响开关本身的生效。
  }
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
    dispatchEvent(new CustomEvent(VOICE_SHORTCUT_SETTINGS_EVENT, {
      detail: { enabled: !!value },
    }));
  } catch {
    // 事件通道不可用时静默：设置本身已落盘。
  }
}

function setVoiceShortcutIntroSeen(value, storage) {
  writeBooleanSetting(VOICE_SHORTCUT_INTRO_SEEN_KEY, !!value, storage);
}

// Smart post-processing (correction + structuring) toggle, enabled by default. Note: the
// tauri bridge (classic script) cannot import this module; it reads localStorage directly
// with the same key (see platform/tauri/bridge/voice.js), so a key change must be applied
// on both sides.
function voicePostprocessEnabled(storage) {
  return readBooleanSetting(VOICE_POSTPROCESS_ENABLED_KEY, true, storage);
}

function setVoicePostprocessEnabled(value, storage) {
  writeBooleanSetting(VOICE_POSTPROCESS_ENABLED_KEY, !!value, storage);
  try {
    dispatchEvent(new CustomEvent(VOICE_SHORTCUT_SETTINGS_EVENT, {
      detail: { postprocessEnabled: !!value },
    }));
  } catch {
    // 事件通道不可用时静默：设置本身已落盘。
  }
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
