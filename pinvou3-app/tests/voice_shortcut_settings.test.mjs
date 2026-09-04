#!/usr/bin/env node
import assert from 'assert';
import {
  VOICE_POSTPROCESS_ENABLED_KEY,
  VOICE_SHORTCUT_ENABLED_KEY,
  VOICE_SHORTCUT_INTRO_SEEN_KEY,
  readBooleanSetting,
  setVoicePostprocessEnabled,
  setVoiceShortcutEnabled,
  setVoiceShortcutIntroSeen,
  voicePostprocessEnabled,
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

// 智能整理开关:缺省开启(隐私告知在设置文案中,关闭才停发 LLM 请求)。
assert.strictEqual(voicePostprocessEnabled(storage), true, 'voice postprocess must default on');
setVoicePostprocessEnabled(false, storage);
assert.strictEqual(storage.getItem(VOICE_POSTPROCESS_ENABLED_KEY), 'false');
assert.strictEqual(voicePostprocessEnabled(storage), false);
setVoicePostprocessEnabled(true, storage);
assert.strictEqual(voicePostprocessEnabled(storage), true);

console.log('voice_shortcut_settings: ok');
