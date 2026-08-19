const VOICE_SHORTCUT_ENABLED_KEY = 'pinvou_voice_shortcut_enabled_v1';
const VOICE_SHORTCUT_INTRO_SEEN_KEY = 'pinvou_voice_shortcut_intro_seen_v8';
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

export {
  VOICE_SHORTCUT_ENABLED_KEY,
  VOICE_SHORTCUT_INTRO_SEEN_KEY,
  VOICE_SHORTCUT_SETTINGS_EVENT,
  readBooleanSetting,
  setVoiceShortcutEnabled,
  setVoiceShortcutIntroSeen,
  voiceShortcutEnabled,
  voiceShortcutIntroSeen,
};
