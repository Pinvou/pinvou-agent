import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { dispatchBrowserNavigation } from '../src/features/browser/browser-navigation.mjs';
import {
  awaitBrowserListenerReadiness,
  browserStatusRetryDelay,
  createBrowserSessionCommandEchoGuard,
  createBrowserSessionEpochTracker,
  eventTargetsActiveBrowserTab,
  isFragmentOnlyBrowserNavigation,
  isBrowserSnapshotDomainCurrent,
  isMonotonicControlRevision,
  navigationEventSettlesPending,
  reconcilePendingNavigationWithActiveTab,
  shouldHydrateBrowserAddressInput,
} from '../src/features/browser/browser-state-sync.mjs';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const browserView = fs.readFileSync(
  path.resolve(here, '..', 'src', 'features', 'browser', 'BrowserView.jsx'),
  'utf8',
);
const appMain = fs.readFileSync(
  path.resolve(here, '..', 'src', 'app', 'main.jsx'),
  'utf8',
);
const browserI18n = fs.readFileSync(
  path.resolve(here, '..', 'src', 'shared', 'i18n', 'browser.js'),
  'utf8',
);

test('hide retry is guarded by the latest surface identity and visibility intent', () => {
  assert.match(browserView, /hideNativeSurfaceWithRetry\(\{/);
  assert.match(browserView, /nativeSurfaceCoordinator\.desired === 'hide'/);
  assert.match(browserView, /nativeSurfaceCoordinator\.sessionId === sessionId/);
  assert.match(
    browserView,
    /nativeSurfaceCoordinator\.owner === nativeSurfaceCoordinator\.intentOwner/,
  );
  assert.match(browserView, /nativeSurfaceCoordinator\.intentSequence === intentSequence/);
  assert.match(
    browserView,
    /nativeSurfaceCoordinator\.intentSequence = \+\+nativeSurfaceIntentSequence;[\s\S]*?browser_show_native_surface/,
  );
  assert.match(browserView, /visibilityGeneration,[\s\S]*?visibilitySequence:/);
  assert.doesNotMatch(
    browserView,
    /(?:claimNativeSurfaceHide|releaseNativeSurface)\([^;]+?\)\.catch\(\(\) => \{\}\)/,
  );
  assert.match(browserView, /console\.error\([\s\S]*?native surface hide failed/);
});

test('persistence warning has a local, translated dismiss control', () => {
  const warningMarkup = browserView.match(
    /data-testid="browser-persistence-warning"[\s\S]*?data-testid="browser-persistence-warning-dismiss"[\s\S]*?<\/button>/,
  )?.[0] || '';

  assert.match(warningMarkup, /title=\{t\.winClose\}/);
  assert.match(warningMarkup, /aria-label=\{t\.winClose\}/);
  assert.match(warningMarkup, /dispatchPersistenceWarning\(\{ type: 'dismiss' \}\)/);
  assert.match(warningMarkup, /<XIcon size=\{13\}/);
  assert.doesNotMatch(warningMarkup, /invokeTauri|persistence-restored/);
});

test('status hydration preserves a dismissed warning while backend events report new occurrences', () => {
  assert.match(
    browserView,
    /dispatchPersistenceWarning\(\{ type: 'hydrate', message: st\.persistenceWarning \|\| '' \}\)/,
  );
  assert.match(
    browserView,
    /listenTauri\('browser:persistence-warning',[\s\S]*?dispatchPersistenceWarning\(\{[\s\S]*?type: 'report'/,
  );
});

test('a failed status request cannot clear an existing persistence warning', () => {
  const refreshStatus = browserView.slice(
    browserView.indexOf('const refreshStatus = useCallback'),
    browserView.indexOf('const refreshTabs = useCallback'),
  );
  const catchBlock = refreshStatus.slice(refreshStatus.indexOf('} catch (e)'));

  assert.doesNotMatch(catchBlock, /dispatchPersistenceWarning\(\{ type: 'clear' \}\)/);
  assert.match(catchBlock, /failed status RPC is not evidence/);
});

test('a fast navigation commit cannot be overwritten by the dispatch acknowledgement', async () => {
  let resolveDispatch;
  let address = '';
  const dispatched = new Promise((resolve) => { resolveDispatch = resolve; });
  const navigation = dispatchBrowserNavigation({
    target: 'http://example.test/',
    publishInput: (value) => { address = value; },
    dispatch: () => dispatched,
  });

  assert.equal(address, 'http://example.test/');
  // Model a Finished event delivered before invoke() resolves.
  address = 'https://example.test/';
  resolveDispatch();
  await navigation;

  assert.equal(address, 'https://example.test/');
});

test('status and tab snapshots cannot overwrite newer scoped browser events', () => {
  const requestEpoch = 4;
  assert.equal(isBrowserSnapshotDomainCurrent(requestEpoch, 4), true);
  assert.equal(
    isBrowserSnapshotDomainCurrent(requestEpoch, 5),
    false,
    'an event received after request start invalidates that status domain',
  );
  assert.equal(eventTargetsActiveBrowserTab('tab-a', 'tab-a'), true);
  assert.equal(eventTargetsActiveBrowserTab('tab-b', 'tab-a'), false);
  assert.equal(eventTargetsActiveBrowserTab('tab-a', null), false);
  assert.equal(isMonotonicControlRevision(8, 9), true);
  assert.equal(isMonotonicControlRevision(8, 8), true);
  assert.equal(isMonotonicControlRevision(8, 7), false);
  assert.equal(isMonotonicControlRevision(8, null), false);
});

test('app lifecycle epochs isolate background sessions and support global stop', () => {
  const events = createBrowserSessionEpochTracker();
  const requests = createBrowserSessionEpochTracker();
  const sessionAEvent = events.snapshot('session-a');
  const sessionARequest = requests.advance('session-a');

  events.advance('session-b');
  requests.advance('session-b');
  assert.equal(events.isCurrent('session-a', sessionAEvent), true);
  assert.equal(requests.isCurrent('session-a', sessionARequest), true);

  events.advance('session-a');
  assert.equal(events.isCurrent('session-a', sessionAEvent), false);
  const afterSessionEvent = events.snapshot('session-a');
  events.advance();
  assert.equal(events.isCurrent('session-a', afterSessionEvent), false);
});

test('listeners become ready before first hydration and failed listeners enable polling', () => {
  const syncEffect = browserView.slice(
    browserView.indexOf('const registrations = []'),
    browserView.indexOf('// ---- Navigation ----'),
  );
  const barrier = syncEffect.indexOf('awaitBrowserListenerReadiness(registrations');
  assert.ok(barrier > 0);
  assert.ok(syncEffect.indexOf('hydrateInitialStatus', barrier) > barrier);
  assert.ok(syncEffect.indexOf('refreshTabs();', barrier) > barrier);
  assert.match(syncEffect, /listenerRegistrationFailed[\s\S]*?window\.setInterval/);
  assert.match(syncEffect, /failed to register \$\{eventName\} listener/);
});

test('initial status retry is bounded and the failed empty state exposes retry', () => {
  assert.equal(browserStatusRetryDelay(0), 250);
  assert.equal(browserStatusRetryDelay(1), 750);
  assert.equal(browserStatusRetryDelay(2), 1500);
  assert.equal(browserStatusRetryDelay(3), null);
  assert.throws(() => browserStatusRetryDelay(-1), /non-negative safe integer/);
  assert.match(browserView, /const hydrateInitialStatus = async \(failedAttempt = 0\)/);
  assert.match(browserView, /outcome !== 'failed'/);
  assert.match(browserView, /browserStatusRetryDelay\(failedAttempt\)/);
  assert.match(browserView, /data-testid="browser-status-retry"/);
  assert.match(browserView, /onClick=\{retryStatus\}/);
  assert.match(browserI18n, /export const browserZh = \{[\s\S]*?browserRetry: '重试'/);
  assert.match(browserI18n, /export const browserEn = \{[\s\S]*?browserRetry: 'Retry'/);
  assert.match(browserI18n, /export const browserJa = \{[\s\S]*?browserRetry: '再試行'/);
});

test('BrowserView invalidates async work and ignores queued events after unmount', () => {
  assert.match(browserView, /const browserViewMountedRef = useRef\(true\)/);
  assert.match(
    browserView,
    /browserViewMountedRef\.current = false;[\s\S]*?statusRequestEpochRef\.current \+= 1;[\s\S]*?tabsRequestEpochRef\.current \+= 1;[\s\S]*?errorEventEpochRef\.current \+= 1/,
  );
  assert.match(
    browserView,
    /const isCurrent = \(\) => \(\s*browserViewMountedRef\.current[\s\S]*?statusRequestEpochRef\.current === requestEpoch/,
  );
  assert.match(
    browserView,
    /!browserViewMountedRef\.current\s*\|\| sessionIdRef\.current !== requestedSessionId/,
  );
  const queuedEventGuards = browserView.match(
    /if \(disposed \|\| !browserViewMountedRef\.current\) return;/g,
  ) || [];
  assert.equal(queuedEventGuards.length, 11);
  assert.match(
    browserView,
    /await invokeTauri\('browser_activate_tab'[\s\S]*?!browserViewMountedRef\.current/,
  );
  assert.match(
    browserView,
    /await invokeTauri\('browser_stop'[\s\S]*?!browserViewMountedRef\.current/,
  );
  assert.match(
    browserView,
    /const control = await invokeTauri\('browser_hand_back_to_agent'[\s\S]*?!browserViewMountedRef\.current/,
  );
});

test('app lifecycle discovery awaits listener registration and polls after registration failure', () => {
  assert.match(appMain, /const browserLifecycleListenersReadyRef = useRef\(null\)/);
  assert.match(
    appMain,
    /const readiness = awaitBrowserListenerReadiness\([\s\S]*?registerActivated, registerStopped/,
  );
  assert.match(
    appMain,
    /browserLifecycleListenersReadyRef\.current = readiness;[\s\S]*?Promise\.resolve\(readiness\)\.then/,
  );
  assert.match(appMain, /listenerRegistrationFailed[\s\S]*?window\.setInterval\(reconcileCurrentSession, 2000\)/);
  assert.match(
    appMain,
    /if \(!st\.running && !st\.restoreError\) \{[\s\S]*?delete next\[requestedSessionId\][\s\S]*?removeBrowserPaneState\(current, requestedSessionId\)/,
  );
  assert.match(appMain, /failed to register browser:activated listener/);
  assert.match(appMain, /failed to register browser:stopped listener/);
  const initialHydration = appMain.slice(
    appMain.indexOf('// Query the session after a WebView reload or chat switch.'),
    appMain.indexOf('// Compact layouts keep the fullscreen browser view'),
  );
  assert.match(
    initialHydration,
    /if \(!st\.running && !st\.restoreError\) \{[\s\S]*?delete next\[requestedSessionId\][\s\S]*?removeBrowserPaneState\(current, requestedSessionId\)/,
  );
});

test('browser prepare attempts use a monotonic sequence across stop/reset ABA', () => {
  assert.match(appMain, /const browserOpenAttemptSequenceRef = useRef\(0\)/);
  assert.match(
    appMain,
    /const attempt = browserOpenAttemptSequenceRef\.current \+ 1;[\s\S]*?browserOpenAttemptsRef\.current\[requestedSessionId\] = attempt/,
  );
  assert.match(
    appMain,
    /browserOpenAttemptSequenceRef\.current \+= 1;[\s\S]*?browserOpenAttemptsRef\.current\[sessionId\] = browserOpenAttemptSequenceRef\.current/,
  );
  assert.match(
    appMain,
    /browserOpenAttemptsRef\.current\[requestedSessionId\] !== attempt/,
  );
});

test('listener readiness times out instead of blocking initial hydration forever', async () => {
  let scheduled;
  const readiness = awaitBrowserListenerReadiness(
    [new Promise(() => {})],
    {
      timeoutMs: 25,
      schedule: (callback) => {
        scheduled = callback;
        return 7;
      },
      cancel: () => assert.fail('a fired timeout must not be cancelled'),
    },
  );

  scheduled();
  assert.equal(await readiness, false);
  assert.match(browserView, /listener registration timed out; enabling reconciliation/);
  assert.match(appMain, /lifecycle listener registration timed out; enabling reconciliation/);
});

test('optimistic navigation invalidates an older status URL until a page event settles', () => {
  let lifecycleEpoch = 11;
  const statusRequestEpoch = lifecycleEpoch;
  lifecycleEpoch += 1;
  assert.equal(
    isBrowserSnapshotDomainCurrent(statusRequestEpoch, lifecycleEpoch),
    false,
  );
  assert.match(
    browserView,
    /const reconciledPendingNavigation[\s\S]*?if \(reconciledPendingNavigation == null\)[\s\S]*?publishCommittedUrl/,
  );
  assert.match(browserView, /eventTargetsActiveBrowserTab\(p\.tab, activeSessionRef\.current\)/);
});

test('focused dirty address input owns presentation until submit or blur', () => {
  assert.equal(shouldHydrateBrowserAddressInput({ focused: true, dirty: true }), false);
  assert.equal(shouldHydrateBrowserAddressInput({ focused: true, dirty: false }), true);
  assert.equal(shouldHydrateBrowserAddressInput({ focused: false, dirty: true }), true);
  assert.match(browserView, /const committedUrlRef = useRef\(''\)/);
  assert.match(browserView, /const urlInputFocusedRef = useRef\(false\)/);
  assert.match(browserView, /const urlInputDirtyRef = useRef\(false\)/);
  assert.match(
    browserView,
    /shouldHydrateBrowserAddressInput\(\{[\s\S]*?focused: urlInputFocusedRef\.current,[\s\S]*?dirty: urlInputDirtyRef\.current/,
  );
  assert.match(
    browserView,
    /const handleAddressBlur[\s\S]*?urlInputDirtyRef\.current = false;[\s\S]*?browserAddressValue\(committedUrlRef\.current\)/,
  );
  assert.match(
    browserView,
    /const handleAddressSubmit[\s\S]*?urlInputDirtyRef\.current = false;[\s\S]*?navigate\(urlInput\)/,
  );
  assert.match(browserView, /onFocus=\{handleAddressFocus\}/);
  assert.match(browserView, /onBlur=\{handleAddressBlur\}/);
});

test('a session change releases focused dirty address ownership before hydration', () => {
  assert.match(browserView, /const addressOwnerSessionIdRef = useRef\(sessionId\)/);
  assert.match(
    browserView,
    /useLayoutEffect\(\(\) => \{\s*sessionIdRef\.current = sessionId;\s*if \(addressOwnerSessionIdRef\.current === sessionId\) return;[\s\S]*?urlInputFocusedRef\.current = false;[\s\S]*?urlInputDirtyRef\.current = false;[\s\S]*?committedUrlRef\.current = '';[\s\S]*?activeSessionRef\.current = null;[\s\S]*?setUrlInput\(''\);[\s\S]*?setActiveSession\(null\);[\s\S]*?\}, \[sessionId\]\)/,
  );
  assert.match(
    browserView,
    /const publishCommittedUrl = useCallback\(\(nextUrl, ownerSessionId = sessionIdRef\.current\) => \{\s*if \(ownerSessionId !== addressOwnerSessionIdRef\.current\) return false;/,
  );
  assert.match(browserView, /publishCommittedUrl\(p\.url, sessionId\)/);
});

test('pending navigation follows its tab and cannot freeze a newly activated tab', () => {
  const pending = { epoch: 3, tab: 'tab-a', requestId: 'request-b' };
  assert.equal(reconcilePendingNavigationWithActiveTab(pending, 'tab-a'), pending);
  assert.equal(reconcilePendingNavigationWithActiveTab(pending, 'tab-b'), null);
  assert.deepEqual(
    reconcilePendingNavigationWithActiveTab(
      { epoch: 4, tab: null, requestId: 'request-b' },
      'tab-a',
    ),
    { epoch: 4, tab: 'tab-a', requestId: 'request-b' },
  );
  assert.equal(
    navigationEventSettlesPending(pending, 'tab-a', 'request-b', 'tab-b'),
    true,
  );
  assert.equal(
    navigationEventSettlesPending(pending, 'tab-a', 'request-a', 'tab-a'),
    false,
  );
  assert.equal(
    navigationEventSettlesPending(pending, 'tab-b', 'request-b', 'tab-b'),
    false,
  );
});

test('fragment-only navigation is optimistic without waiting for a cross-document request id', () => {
  assert.equal(
    isFragmentOnlyBrowserNavigation(
      'https://example.test/path?q=1#old',
      'https://example.test/path?q=1#next',
    ),
    true,
  );
  assert.equal(
    isFragmentOnlyBrowserNavigation(
      'https://example.test/path?q=1',
      'https://example.test/path?q=2#next',
    ),
    false,
  );
  assert.match(browserView, /const fragmentOnly = isFragmentOnlyBrowserNavigation\(url, target\)/);
  assert.match(browserView, /pendingNavigationRef\.current = fragmentOnly\s*\? null/);
  assert.match(browserView, /if \(fragmentOnly\) publishCommittedUrl\(target, sessionId\)/);
});

test('history and reload supersede an optimistic address-bar request', () => {
  const runNav = browserView.slice(
    browserView.indexOf('const runNav = useCallback'),
    browserView.indexOf('const openExternal = useCallback'),
  );
  assert.match(runNav, /navigationRequestEpochRef\.current \+= 1/);
  assert.match(runNav, /pendingNavigationRef\.current = null/);
  assert.match(runNav, /refreshStatus\(\)/);
  assert.match(runNav, /refreshTabs\(\)/);
});

test('a successful stop invalidates status snapshots even if its event is delivered later', () => {
  const stopBrowser = browserView.slice(
    browserView.indexOf('const stopBrowser = useCallback'),
    browserView.indexOf('const handBackToAgent = useCallback'),
  );
  assert.match(stopBrowser, /await invokeTauri\('browser_stop'/);
  assert.match(stopBrowser, /statusRequestEpochRef\.current \+= 1/);
  assert.match(stopBrowser, /lifecycleEventEpochRef\.current \+= 1/);
  assert.ok(
    stopBrowser.indexOf('statusRequestEpochRef.current += 1')
      < stopBrowser.indexOf('setRunning(false)'),
  );
});

test('workspace control events are revision ordered rather than scoped to one tab', () => {
  const controlListener = browserView.match(
    /listenTauri\('browser:control-changed',[\s\S]*?\n {4}\}\)\);/,
  )?.[0] || '';

  assert.match(controlListener, /payload\.sessionId !== sessionId/);
  assert.match(controlListener, /WorkspaceControl is shared by every tab/);
  assert.match(controlListener, /isMonotonicControlRevision\(controlRevisionRef\.current, payload\.revision\)/);
  assert.doesNotMatch(controlListener, /eventTargetsActiveBrowserTab/);
  assert.doesNotMatch(controlListener, /payload\.tab\b/);
});

test('late command failures cannot replace the result of a newer operation or host event', () => {
  assert.match(browserView, /const beginErrorOperation = useCallback/);
  assert.match(
    browserView,
    /!browserViewMountedRef\.current[\s\S]*?errorEventEpochRef\.current !== operationEpoch[\s\S]*?\) return false;/,
  );
  assert.match(browserView, /reportErrorForOperation\(errorOperationEpoch, e\)/);
  assert.match(
    browserView,
    /reportErrorForOperation\(errorOperationEpoch, e\)[\s\S]*?refreshStatus\(\{ preserveError: true \}\)/,
  );
  assert.match(browserView, /!preserveError[\s\S]*?setError\(st\.restoreError \|\| ''\)/);
});

test('title and blocked-navigation events reconcile state invalidated by their epochs', () => {
  const titleListener = browserView.match(
    /listenTauri\('browser:tab-title',[\s\S]*?\n {4}\}\)\);/,
  )?.[0] || '';
  const blockedListener = browserView.match(
    /listenTauri\('browser:navigation-blocked',[\s\S]*?\n {4}\}\)\);/,
  )?.[0] || '';

  assert.match(titleListener, /tabsEventEpochRef\.current \+= 1/);
  assert.match(titleListener, /setTabs\(/);
  assert.match(titleListener, /refreshTabs\(\)/);
  assert.match(blockedListener, /pendingNavigationRef\.current = null/);
  assert.match(blockedListener, /refreshStatus\(\{ preserveError: true \}\)/);
});

test('app lifecycle snapshots cannot resurrect state after activated or stopped events', () => {
  assert.match(appMain, /browserLifecycleEventEpochRef\.current = createBrowserSessionEpochTracker\(\)/);
  assert.match(appMain, /browserLifecycleStatusRequestEpochRef\.current = createBrowserSessionEpochTracker\(\)/);
  assert.match(
    appMain,
    /listen\('browser:stopped'[\s\S]*?browserLifecycleEventEpochRef\.current\.advance\(sessionId \|\| null\)/,
  );
  assert.match(
    appMain,
    /browserLifecycleEventEpochRef\.current\.isCurrent\([\s\S]*?eventEpoch[\s\S]*?browserLifecycleStatusRequestEpochRef\.current\.isCurrent\([\s\S]*?requestEpoch/,
  );
  assert.match(
    appMain,
    /browserLifecycleEventEpochRef\.current\.isCurrent\([\s\S]*?snapshot\.eventEpoch[\s\S]*?browserLifecycleStatusRequestEpochRef\.current\.isCurrent\([\s\S]*?snapshot\.requestEpoch/,
  );
});

test('rapid bridge session changes always enqueue the latest serialized publication', () => {
  const syncEffect = appMain.slice(
    appMain.indexOf('// Sync from bridge state'),
    appMain.indexOf('// HMR or legacy frontend state'),
  );

  assert.match(syncEffect, /const bridgeObservation = browserSessionCommandEchoGuard\.observe\(nextSessionId\)/);
  assert.match(syncEffect, /const isCommandEcho = bridgeObservation\.type === 'command-echo'/);
  assert.match(appMain, /browserSessionCommandEchoGuard\.begin\(/);
  assert.match(appMain, /browserSessionCommandEchoGuard\.settle\(sessionCommandToken\)/);
  assert.equal((appMain.match(/sessionTarget:/g) || []).length, 4);
  assert.match(syncEffect, /bridgeTransition\?\.sessionId !== nextSessionId/);
  assert.match(
    syncEffect,
    /bridgeTransition != null\s*&& bridgeTransition\.sessionId !== nextSessionId/,
  );
  assert.match(syncEffect, /const transitionToken = \{ sessionId: nextSessionId \}/);
  assert.match(syncEffect, /runBrowserUiTransition\(publishSession/);
  assert.match(syncEffect, /channel: 'session'/);
  assert.match(syncEffect, /serialize: true/);
  assert.match(syncEffect, /sessionSource: 'bridge'/);
  assert.match(syncEffect, /browserBridgeSessionTransitionRef\.current === transitionToken/);
  assert.doesNotMatch(syncEffect, /browserSessionTransitionPendingRef\.current === 0/);
  assert.doesNotMatch(syncEffect, /reconcileSessionOnSettle: false/);
  assert.match(syncEffect, /const publishedView = currentViewRef\.current/);
  assert.doesNotMatch(syncEffect, /nextSessionId && currentView !==/);
});

test('session command self-echo is suppressed without losing B to C to B latest-wins', () => {
  const guard = createBrowserSessionCommandEchoGuard();
  const commandB = guard.begin('B', 'A');

  assert.equal(guard.observe('A').type, 'baseline');
  assert.deepEqual(guard.observe('B'), { type: 'command-echo', token: commandB });
  assert.deepEqual(guard.observe('B'), { type: 'command-echo', token: commandB });
  assert.equal(guard.isPending(commandB), true);

  assert.equal(guard.observe('C').type, 'external');
  assert.equal(guard.isPending(commandB), false);
  assert.equal(guard.observe('B').type, 'external');
});

test('overlapping session commands use token identity across same-target ABA', () => {
  const guard = createBrowserSessionCommandEchoGuard();
  const oldB = guard.begin('B', 'A');
  const commandC = guard.begin('C', 'A');
  const newB = guard.begin('B', 'A');

  assert.equal(guard.observe('B').token, newB);
  assert.equal(guard.settle(oldB), true);
  assert.equal(guard.isPending(newB), true);
  assert.equal(guard.observe('C').token, commandC);
  assert.equal(guard.settle(oldB), false);
  assert.equal(guard.isPending(newB), true);
});
