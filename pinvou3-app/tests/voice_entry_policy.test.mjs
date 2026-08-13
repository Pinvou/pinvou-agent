#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(testDir, '..');
const chatSource = fs.readFileSync(path.join(appRoot, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
const controlsSource = fs.readFileSync(path.join(appRoot, 'src', 'features', 'voice-composer', 'VoiceComposerControls.jsx'), 'utf8');
const policySource = fs.readFileSync(path.join(appRoot, 'src', 'features', 'voice-composer', 'voice-ui-policy.mjs'), 'utf8');
const packageJson = JSON.parse(fs.readFileSync(path.join(appRoot, 'package.json'), 'utf8'));

assert.match(controlsSource, /<VoiceRecordingPill/, 'composer voice interaction pill must be shared');
assert.match(chatSource, /<VoiceComposerPillLayer[\s\S]*onConfirm=\{\(\) => handleVoiceTrigger\(voiceMode\)\}/, 'voice pill must expose confirm/stop action');
assert.match(policySource, /status === 'requesting_permission' && voiceInput\.stage !== 'device'/, 'dependency/model checking must not flash the voice interaction pill');
const voicePillVisibleExpression = policySource.match(/function shouldShowVoicePill[\s\S]*?\n\}/)?.[0] || '';
assert.doesNotMatch(voicePillVisibleExpression, /voiceInput\.status === 'failed'/, 'failed voice state must use the composer notice only, not the interaction pill');
assert.match(chatSource, /<VoiceComposerButton/, 'chat composer must render the shared microphone entry');
assert.match(controlsSource, /menuItems\.map/, 'composer microphone entry must expose shared voice mode menu items');
assert.doesNotMatch(chatSource, /key: 'structured'[\s\S]*handleVoiceMenuTrigger\('structured'\)/, 'structured must not be exposed as a separate voice menu mode');
assert.match(chatSource, /key: 'edit'[\s\S]*handleVoiceMenuTrigger\('edit'\)/, 'chat composer must expose voice edit from the voice menu when draft text exists');
assert.match(controlsSource, /data-testid="voice-edit-preview"/, 'voice edit confirmation preview must be shared');
assert.match(controlsSource, /testId = 'composer-voice-button'/, 'composer microphone entry must remain available');
assert.doesNotMatch(chatSource, /data-testid="floating-voice-button"/, 'page-level floating voice entry must not render');
assert.doesNotMatch(chatSource, /\btabletVoiceMode\b/, 'tablet or touch mode must not enable a separate voice entry');
assert.doesNotMatch(chatSource, /\bfloatingVoice[A-Z]/, 'floating voice state and refs should stay removed');
assert.match(policySource, /return isVoiceActive\(voiceInput\) \|\| voiceInput\.status === 'failed';/, 'cancelled or completed voice states must not render a composer-wide notice');
assert.equal(packageJson.scripts['test:floating-voice-drag'], undefined, 'obsolete floating voice drag test script must stay removed');

console.log('voice_entry_policy: ok');
