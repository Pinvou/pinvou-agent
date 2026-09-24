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
const auxQuoteSelection = read('../src/features/aux-chat/AuxQuoteSelection.jsx');
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
  // The aux quote popover portals a `fixed` viewport-clamped button to <body>
  // (round-30 D4): with the dock open it can land inside the native surface,
  // and its zero-rect fallback centers on the viewport — so it must hold the
  // same occlusion permit as the other overlays before painting.
  assert.match(auxQuoteSelection, /useRightDockOcclusion\(\s*'aux-quote-selection',[\s\S]*?!!popover\s*\)/);
  assert.match(auxQuoteSelection, /if \(!popover \|\| !publicationReady\) return null/);
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

test('closing the aux chat panel restores the dock panel recorded at open', () => {
  // Full parity with the subagent panel (round-30 D3): the first open records
  // restorePanelId (repeat opens keep the first record) and close jumps back
  // to the recorded panel — and ONLY to a recorded panel. The earlier aux
  // copy dropped the `restorePanelId &&` guard and fell back to 'browser',
  // so any close without a record (aux opened with the dock closed, or after
  // a session switch) popped the native browser webview over the panel the
  // user was actually on — and its own rationale contradicted the
  // `browserDockOpen === true` guard the branch sits behind.
  // Anchor-resolution guards (the vacuous-slice class ui_language_coverage
  // fixed for its own slices): a renamed anchor would make indexOf return -1
  // and the pair-slice run to near-EOF, letting these assertions pass on
  // handleRestart-style copies elsewhere in the file.
  const openStart = chatView.indexOf('const openAuxChatPanel');
  const closeStart = chatView.indexOf('const closeAuxChatPanel');
  const closeEnd = chatView.indexOf('const handlePreviewArtifact');
  assert.ok(
    openStart >= 0 && closeStart > openStart && closeEnd > closeStart,
    'open/close block anchors must resolve (a vacuous slice would pass on unrelated copies)',
  );
  const openBlock = chatView.slice(openStart, closeStart);
  const closeBlock = chatView.slice(closeStart, closeEnd);
  assert.match(openBlock, /restorePanelId: current[\s\S]*?current\.restorePanelId[\s\S]*?rightDockActivePanelId/);
  assert.match(closeBlock, /const restorePanelId = auxChatPanel\?\.restorePanelId \|\| null/);
  // Subagent-pattern parity: the dock restore runs only with a recorded
  // target (no 'browser' fallback), matching closeSubagentPanel exactly.
  assert.match(
    closeBlock,
    /if \(browserDockOpen && restorePanelId && onRightDockPanelSelectionChange\) \{/,
    'closeAuxChatPanel must gate the dock restore on a recorded target (round-30 D3: the dropped guard popped the native browser webview over the current panel)',
  );
  assert.match(closeBlock, /\[restorePanelId, requestedSessionId, publishClose\]/);
  assert.doesNotMatch(
    closeBlock,
    /restorePanelId \|\| 'browser'/,
    'no browser fallback: an unrecorded close must leave the dock selection alone (round-30 D3)',
  );
  // The recorded target must not outlive a session switch (round-30 D3): the
  // aux panel rebinds instead of closing, so without an explicit reset task
  // A's recorded target would apply to task B (the subagent panel gets the
  // same reset by closing outright).
  assert.match(
    chatView,
    /setAuxChatPanel\(\(current\) => \(current \? \{ \.\.\.current, restorePanelId: null \} : current\)\);\s*\}, \[activeSessionId\]\);/,
    'restorePanelId must reset on session switch (round-30 D3)',
  );
  // Round-26 minor M5: the close publishes through the same currency guard as
  // closeSubagentPanel — a rapid close→open must invalidate the stale close's
  // dock restore, or it settles out of order and leaves the dock on the
  // restore panel with the aux panel mounted but occluded.
  assert.match(
    closeBlock,
    /const requestId = auxChatPanelRequestRef\.current \+ 1;\s*auxChatPanelRequestRef\.current = requestId;[\s\S]*?isSubagentPanelPublicationCurrent\(\{[\s\S]*?currentRequestId: auxChatPanelRequestRef\.current,[\s\S]*?sessionId: requestedSessionId,[\s\S]*?currentSessionId: activeSessionIdRef\.current/,
    'closeAuxChatPanel must gate its publication on request/session currency (round-26 minor M5)',
  );
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
