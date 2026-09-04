#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { shouldShowVoiceNotice, shouldShowVoicePill } from '../src/features/voice-composer/voice-ui-policy.mjs';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(testDir, '..');
const chatSource = fs.readFileSync(path.join(appRoot, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const codexSource = fs.readFileSync(path.join(appRoot, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
const controlsSource = fs.readFileSync(path.join(appRoot, 'src', 'features', 'voice-composer', 'VoiceComposerControls.jsx'), 'utf8');
const packageJson = JSON.parse(fs.readFileSync(path.join(appRoot, 'package.json'), 'utf8'));

assert.match(controlsSource, /<VoiceRecordingPill/, 'composer voice interaction pill must be shared');
assert.match(chatSource, /<VoiceComposerPillLayer[\s\S]*onConfirm=\{\(\) => handleVoiceTrigger\(voiceMode\)\}/, 'voice pill must expose confirm/stop action');
assert.equal(shouldShowVoicePill({ status: 'requesting_permission', stage: 'device' }), false, 'dependency/model checking must not flash the voice interaction pill');
assert.equal(shouldShowVoicePill({ status: 'requesting_permission', stage: 'permission' }), true, 'permission request must show the voice interaction pill');
assert.equal(shouldShowVoicePill({ status: 'failed', message: 'mic error' }), false, 'failed voice state must use the composer notice only, not the interaction pill');
assert.equal(shouldShowVoicePill({ status: 'recording' }), true, 'recording must show the voice interaction pill');
assert.equal(shouldShowVoicePill({ status: 'transcribing' }), true, 'transcribing must show the voice interaction pill');
assert.equal(shouldShowVoicePill({ status: 'postprocessing' }), true, 'postprocessing must show the voice interaction pill');
assert.match(chatSource, /<VoiceComposerButton/, 'chat composer must render the shared microphone entry');
assert.doesNotMatch(controlsSource, /menuItems\.map/, 'composer microphone entry must not expose a voice mode dropdown');
assert.doesNotMatch(controlsSource, /ChevronDown/, 'composer microphone entry must not render the obsolete mode dropdown arrow');
assert.doesNotMatch(chatSource, /voiceContinueDictation[\s\S]*handleVoiceMenuTrigger/, 'chat composer must not expose continue dictation from a dropdown menu');
assert.doesNotMatch(chatSource, /key: 'edit'[\s\S]*handleVoiceMenuTrigger\('edit'\)/, 'chat composer must not expose voice edit from a dropdown menu');
assert.doesNotMatch(chatSource, /key: 'structured'[\s\S]*handleVoiceMenuTrigger\('structured'\)/, 'structured must not be exposed as a separate voice menu mode');
assert.match(controlsSource, /data-testid="voice-edit-preview"/, 'voice edit confirmation preview must be shared');
assert.match(controlsSource, /testId = 'composer-voice-button'/, 'composer microphone entry must remain available');
assert.match(codexSource, /testId="codex-voice-input"/, 'codex composer microphone entry must remain available');
assert.doesNotMatch(codexSource, /<Paperclip size=\{18\} \/>[\s\S]{0,500}<VoiceComposerButton/, 'codex composer microphone must not stay in the left tool group after attachments');
assert.match(codexSource, /<VoiceComposerButton[\s\S]*testId="codex-voice-input"[\s\S]*<button type="button" onClick=\{\(\) => send\(\)\}/, 'codex composer microphone must render next to a safe send button handler');
assert.doesNotMatch(chatSource, /data-testid="floating-voice-button"/, 'page-level floating voice entry must not render');
assert.doesNotMatch(chatSource, /\btabletVoiceMode\b/, 'tablet or touch mode must not enable a separate voice entry');
assert.doesNotMatch(chatSource, /\bfloatingVoice[A-Z]/, 'floating voice state and refs should stay removed');
assert.equal(shouldShowVoiceNotice({ status: 'failed', message: 'mic error' }), true, 'failed voice state must render a composer-wide notice');
assert.equal(shouldShowVoiceNotice({ status: 'recording', message: 'recording…' }), true, 'active voice state with a message must render a composer-wide notice');
assert.equal(shouldShowVoiceNotice({ status: 'cancelled', message: 'cancelled' }), false, 'cancelled voice state must not render a composer-wide notice');
assert.equal(shouldShowVoiceNotice({ status: 'completed', message: 'done' }), false, 'completed voice state must not render a composer-wide notice');
assert.equal(shouldShowVoiceNotice({ status: 'recording', message: '' }), false, 'voice notice requires a non-empty message');
assert.equal(packageJson.scripts['test:floating-voice-drag'], undefined, 'obsolete floating voice drag test script must stay removed');

console.log('voice_entry_policy: ok');
