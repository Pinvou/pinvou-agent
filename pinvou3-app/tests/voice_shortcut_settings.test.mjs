#!/usr/bin/env node
import assert from 'assert';
import {
  VOICE_SHORTCUT_ENABLED_KEY,
  VOICE_SHORTCUT_INTRO_SEEN_KEY,
  readBooleanSetting,
  setVoiceShortcutEnabled,
  setVoiceShortcutIntroSeen,
  voiceShortcutEnabled,
  voiceShortcutIntroSeen,
} from '../src/features/chat/voice-shortcut-settings.mjs';

function createStorage() {
  const entries = new Map();
  return {
    getItem(key) {
      return entries.has(key) ? entries.get(key) : null;
    },
    setItem(key, value) {
      entries.set(key, String(value));
    },
  };
}

const storage = createStorage();
storage.setItem('pinvou_voice_shortcut_intro_seen_v7', 'true');

assert.strictEqual(voiceShortcutEnabled(storage), false, 'voice shortcuts must default off');
assert.strictEqual(voiceShortcutIntroSeen(storage), false, 'old v7 intro state must not suppress the v8 intro');
assert.strictEqual(readBooleanSetting('missing', true, storage), true);

setVoiceShortcutEnabled(true, storage);
assert.strictEqual(storage.getItem(VOICE_SHORTCUT_ENABLED_KEY), 'true');
assert.strictEqual(voiceShortcutEnabled(storage), true);

setVoiceShortcutEnabled(false, storage);
assert.strictEqual(storage.getItem(VOICE_SHORTCUT_ENABLED_KEY), 'false');
assert.strictEqual(voiceShortcutEnabled(storage), false);

setVoiceShortcutIntroSeen(true, storage);
assert.strictEqual(storage.getItem(VOICE_SHORTCUT_INTRO_SEEN_KEY), 'true');
assert.strictEqual(voiceShortcutIntroSeen(storage), true);

console.log('voice_shortcut_settings: ok');
