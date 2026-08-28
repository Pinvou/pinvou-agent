import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
  invokeObservedPanelSelection,
  isSubagentPanelPublicationCurrent,
} from '../src/features/chat/subagent-panel-publication.mjs';

const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8');

const rightDock = read('../src/components/layout/RightDock.jsx');
const composerPopover = read('../src/components/ComposerPopover.jsx');
const attachmentDrop = read('../src/features/attachments/AttachmentDropOverlay.jsx');
const chatView = read('../src/features/chat/ChatView.jsx');
const main = read('../src/app/main.jsx');

test('RightDock occlusion is a publication permit rather than a post-commit notice', () => {
  assert.match(rightDock, /onBeforeOcclusionPublish\(occlusionId, commit\)/);
  assert.match(rightDock, /const publish = \(\) => \{[\s\S]*setPublicationReady\(true\)/);
  assert.match(rightDock, /return active \? \(!dock \|\| !occlusionId \? true : publicationReady\) : false/);
  assert.match(rightDock, /const releaseOcclusion = dock\?\.releaseOcclusion/);
  assert.match(rightDock, /releaseOcclusion\(occlusionId\)/);
});

test('every child overlay that can cover the native browser waits for the permit', () => {
  assert.match(composerPopover, /if \(!open \|\| !publicationReady\) return null/);
  assert.match(attachmentDrop, /if \(active && !publicationReady\) return null/);
  assert.match(chatView, /voiceAsrSetupPublicationReady && \(\(\) =>/);
  assert.match(chatView, /data-testid="voice-asr-setup-dialog"/);
  assert.match(
    chatView,
    /useRightDockOcclusion\(\s*'artifact-fullscreen',[\s\S]*?artifactsVisible && artifactsFullscreen/,
  );
  assert.match(
    chatView,
    /artifactsVisible && artifactsFullscreen && artifactFullscreenPublicationReady && createPortal/,
  );
});

test('App reserves BrowserView suspension in the same gated publication batch', () => {
  assert.match(main, /channel: `right-dock-occlusion:\$\{occlusionId\}`,[\s\S]*hideMode: 'visible'/);
  assert.match(main, /const published = publish\(\);[\s\S]*setRightDockOcclusionPublications/);
  assert.match(main, /rightDockOcclusionPublications\.length > 0[\s\S]*rightDockState\.occluded/);
  assert.match(main, /onBeforeOcclusionPublish=\{publishRightDockOcclusion\}/);
  assert.match(main, /onOcclusionRelease=\{releaseRightDockOcclusion\}/);
});

test('subagent selection and its first render share the App ACK-gated publication', () => {
  assert.match(main, /selectRightDockPanel = useCallback\(\(panelId, sessionId, publishSelection\)/);
  assert.match(main, /const childPublished = publishSelection\?\.\(\{/);
  assert.match(main, /browserSessionIdRef\.current === selectedSessionId/);
  assert.match(
    chatView,
    /invokeObservedPanelSelection\(\s*onRightDockPanelSelectionChange,[\s\S]*?\['subagent-transcript', requestedSessionId, publishOpen\]/,
  );
  assert.match(
    chatView,
    /isSubagentPanelPublicationCurrent\(\{[\s\S]*?sessionId: requestedSessionId,[\s\S]*?currentSessionId: activeSessionIdRef\.current/,
  );
  assert.match(chatView, /restorePanelId: current[\s\S]*?current\.restorePanelId/);
});

test('a newer subagent open invalidates a delayed close across same-session ABA', () => {
  const sessionId = 'session-a';
  const delayedCloseRequestId = 2;
  const newerOpenRequestId = 3;

  assert.equal(isSubagentPanelPublicationCurrent({
    transitionCurrent: true,
    requestId: delayedCloseRequestId,
    currentRequestId: newerOpenRequestId,
    sessionId,
    currentSessionId: sessionId,
  }), false);
  assert.equal(isSubagentPanelPublicationCurrent({
    transitionCurrent: true,
    requestId: delayedCloseRequestId,
    currentRequestId: delayedCloseRequestId,
    sessionId,
    currentSessionId: 'session-b',
  }), false);
  assert.equal(isSubagentPanelPublicationCurrent({
    transitionCurrent: false,
    requestId: newerOpenRequestId,
    currentRequestId: newerOpenRequestId,
    sessionId,
    currentSessionId: sessionId,
  }), false);
  assert.equal(isSubagentPanelPublicationCurrent({
    transitionCurrent: true,
    requestId: newerOpenRequestId,
    currentRequestId: newerOpenRequestId,
    sessionId,
    currentSessionId: sessionId,
  }), true);
  assert.match(
    chatView,
    /const closeSubagentPanel[\s\S]*?const requestId = subagentPanelRequestRef\.current \+ 1[\s\S]*?isSubagentPanelPublicationCurrent\(\{[\s\S]*?currentRequestId: subagentPanelRequestRef\.current/,
  );
});

test('asynchronous and synchronous panel selection failures are observed', async () => {
  const asyncFailure = new Error('async selection failed');
  const syncFailure = new Error('sync selection failed');
  const reported = [];
  const onError = (error) => reported.push(error);

  const asyncResult = invokeObservedPanelSelection(
    () => Promise.reject(asyncFailure),
    [],
    onError,
  );
  assert.equal(await asyncResult, false);
  assert.equal(invokeObservedPanelSelection(() => {
    throw syncFailure;
  }, [], onError), false);
  assert.deepEqual(reported, [asyncFailure, syncFailure]);
  assert.match(
    chatView,
    /invokeObservedPanelSelection\([\s\S]*?onRightDockPanelSelectionChange,[\s\S]*?reportRightDockSelectionFailure/,
  );
});
