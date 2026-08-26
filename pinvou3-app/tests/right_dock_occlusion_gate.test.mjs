import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');

const rightDock = read('../src/components/layout/RightDock.jsx');
const composerPopover = read('../src/components/ComposerPopover.jsx');
const attachmentDrop = read('../src/features/attachments/AttachmentDropOverlay.jsx');
const chatView = read('../src/features/chat/ChatView.jsx');
const main = read('../src/app/main.jsx');
const uiSmoke = read('./ui_smoke.js');

test('RightDock occlusion is a publication permit rather than a post-commit notice', () => {
  assert.match(rightDock, /onBeforeOcclusionPublish\(occlusionId, commit\)/);
  assert.match(rightDock, /const publish = \(\) => \{[\s\S]*setPublicationReady\(true\)/);
  assert.match(rightDock, /return !active \? false : \(!dock \|\| !occlusionId \? true : publicationReady\)/);
  assert.match(rightDock, /dock\.releaseOcclusion\(occlusionId\)/);
});

test('every child overlay that can cover the native browser waits for the permit', () => {
  assert.match(composerPopover, /if \(!open \|\| !publicationReady\) return null/);
  assert.match(attachmentDrop, /if \(active && !publicationReady\) return null/);
  assert.match(chatView, /voiceAsrSetupPublicationReady && \(\(\) =>/);
  assert.match(chatView, /data-testid="voice-asr-setup-dialog"/);
});

test('App reserves BrowserView suspension in the same gated publication batch', () => {
  assert.match(main, /channel: `right-dock-occlusion:\$\{occlusionId\}`,[\s\S]*hideMode: 'visible'/);
  assert.match(main, /const published = publish\(\);[\s\S]*setRightDockOcclusionPublications/);
  assert.match(main, /rightDockOcclusionPublications\.length > 0[\s\S]*rightDockState\.occluded/);
  assert.match(main, /onBeforeOcclusionPublish=\{publishRightDockOcclusion\}/);
  assert.match(main, /onOcclusionRelease=\{releaseRightDockOcclusion\}/);
});

test('artifact fullscreen relies on the already ACK-gated artifact dock switch', () => {
  assert.doesNotMatch(chatView, /useRightDockOcclusion\('artifact-fullscreen'/);
  assert.match(uiSmoke, /artifactFullscreenAfterDockHide/);
  assert.match(uiSmoke, /hideCallsBeforeArtifactFullscreen/);
});
