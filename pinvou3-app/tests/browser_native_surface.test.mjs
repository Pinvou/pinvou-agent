import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const browserView = readFileSync(
  path.join(projectRoot, 'src', 'features', 'browser', 'BrowserView.jsx'),
  'utf8',
);
const browserManager = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'features', 'browser', 'mod.rs'),
  'utf8',
);
const tauriApp = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'lib.rs'),
  'utf8',
);
const nativeHost = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'features', 'browser', 'platform', 'host.rs'),
  'utf8',
);
const nativeState = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'features', 'browser', 'platform', 'state.rs'),
  'utf8',
);
const nativePlatform = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'features', 'browser', 'platform', 'mod.rs'),
  'utf8',
);
const linuxAutomation = readFileSync(
  path.join(
    projectRoot,
    'src-tauri',
    'src',
    'features',
    'browser',
    'platform',
    'linux_automation.rs',
  ),
  'utf8',
);
const linuxSurface = readFileSync(
  path.join(
    projectRoot,
    'src-tauri',
    'src',
    'features',
    'browser',
    'platform',
    'linux_surface.rs',
  ),
  'utf8',
);
const browserPaths = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'platform', 'paths.rs'),
  'utf8',
);
const browserCommands = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'app', 'commands', 'browser.rs'),
  'utf8',
);
const cdpClient = readFileSync(
  path.join(projectRoot, 'src-tauri', 'src', 'features', 'browser', 'cdp.rs'),
  'utf8',
);
const main = readFileSync(path.join(projectRoot, 'src', 'app', 'main.jsx'), 'utf8');
const chatView = readFileSync(
  path.join(projectRoot, 'src', 'features', 'chat', 'ChatView.jsx'),
  'utf8',
);
const composerPopover = readFileSync(
  path.join(projectRoot, 'src', 'components', 'ComposerPopover.jsx'),
  'utf8',
);
const attachmentDropOverlay = readFileSync(
  path.join(projectRoot, 'src', 'features', 'attachments', 'AttachmentDropOverlay.jsx'),
  'utf8',
);
const browserWrapper = readFileSync(
  path.join(
    projectRoot,
    'src-tauri',
    'resources',
    'common',
    'bundle',
    'mcp-servers',
    'browser-wrapper.mjs',
  ),
  'utf8',
);
const defaultCapability = JSON.parse(
  readFileSync(path.join(projectRoot, 'src-tauri', 'capabilities', 'default.json'), 'utf8'),
);

test('IPC capability is granted only to named WebViews and not inherited by child surfaces', () => {
  assert.equal(defaultCapability.windows, undefined);
  assert.deepEqual(defaultCapability.webviews, [
    'main',
    'detached-*',
    'pet',
    'code-reader',
  ]);
});

test('browser presentation renders only the native surface without screenshot streaming', () => {
  assert.match(browserView, /browser_show_native_surface/);
  assert.doesNotMatch(browserView, /listenTauri\(['"]browser:frame/);
  assert.doesNotMatch(browserView, /browser_set_streaming/);
  assert.doesNotMatch(browserView, /data:image\/(?:jpeg|png);base64/);
  assert.doesNotMatch(browserManager, /Page\.(?:start|stop)Screencast/);
  assert.doesNotMatch(browserManager, /browser:frame/);
  assert.doesNotMatch(cdpClient, /Page\.screencastFrame/);
  assert.doesNotMatch(cdpClient, /Page\.screencastFrameAck/);
});

test('Linux child WebView uses a fixed dock overlay and stays hidden on layout failure', () => {
  assert.match(nativePlatform, /#\[cfg\(target_os = "linux"\)\]\s*mod linux_surface;/);
  assert.match(nativePlatform, /linux_surface::attach\(webview\)/);
  assert.match(nativePlatform, /linux_surface::show\(webview, bounds\)/);
  assert.match(nativePlatform, /linux_surface::hide\(webview\)/);
  assert.match(nativePlatform, /linux_surface::prepare\(&main_webview\)/);
  assert.match(nativeHost, /super::attach_native_surface\(&webview\)/);
  assert.match(nativeHost, /super::show_native_surface\(&webview, workspace\.bounds\)/);
  assert.match(nativeHost, /super::hide_native_surface\(&webview\)/);
  assert.match(linuxSurface, /gtk::Overlay::new\(\)/);
  assert.match(linuxSurface, /gtk::Fixed::new\(\)/);
  assert.match(linuxSurface, /overlay\.add_overlay\(&fixed\)/);
  assert.match(linuxSurface, /overlay\.set_overlay_pass_through\(&fixed, true\)/);
  assert.match(linuxSurface, /fixed\.put\(native, 0, 0\)/);
  assert.match(linuxSurface, /fixed\.move_\(native, logical\.x, logical\.y\)/);
  assert.match(linuxSurface, /native\.set_size_request\(logical\.width, logical\.height\)/);
  assert.match(linuxSurface, /let scale = f64::from\(native\.scale_factor\(\)\)/);
  assert.match(
    linuxSurface,
    /native\.hide\(\);[\s\S]{0,360}hide_empty_overlay\(&fixed\);[\s\S]{0,160}Linux native browser surface show failed/,
  );
  assert.match(linuxSurface, /fn hide_empty_overlay\(fixed: &gtk::Fixed\)/);
  assert.match(linuxSurface, /fixed\.show\(\);\s*native\.show\(\)/);
  const installOverlay = linuxSurface.slice(
    linuxSurface.indexOf('fn install_overlay('),
    linuxSurface.indexOf('fn find_overlay_host('),
  );
  assert.doesNotMatch(installOverlay, /fixed\.show\(\)/);
  assert.doesNotMatch(linuxSurface, /\.show_all\(/);
});

test('Windows MCP enables pageId routing and structured targetId output', () => {
  assert.match(
    browserWrapper,
    /\['--experimental-page-id-routing', '--experimental-structured-content'\]/,
  );
});

test('CDP liveness probing is asynchronous and every consumer awaits the result', () => {
  assert.match(browserWrapper, /import \{ execFile, spawn \} from 'node:child_process'/);
  assert.doesNotMatch(browserWrapper, /execFileSync/);
  assert.match(browserWrapper, /async function probeCdp\(port, timeoutMs\)/);
  assert.match(browserWrapper, /await probeCdp\(portFile\.port, 1000\)/);
  assert.match(browserWrapper, /await probeCdp\(port, 2_000\)/);
  assert.match(browserWrapper, /void probeCdp\(port, 1000\)[\s\S]{0,100}\.then/);
});

test('native surface failures are explicit and retryable', () => {
  assert.match(browserView, /nativeAvailable === false/);
  assert.match(browserView, /browserNativeUnavailable/);
  assert.match(browserView, /browserRetry/);
  assert.match(browserView, /setSurfaceEpoch/);
});

test('initialized blank document renders as a product new-tab page with native surface paused', () => {
  assert.match(browserView, /const showingNewTab = running && isInternalBlankPageUrl\(url\)/);
  assert.match(browserView, /const \[initialStatusResolved, setInitialStatusResolved\] = useState\(false\)/);
  assert.match(browserView, /const nativeSurfaceReady = shouldShowNativeBrowserSurface\(\{[\s\S]*statusResolved: initialStatusResolved/);
  assert.match(browserView, /const shouldSuspendNativeSurface = !nativeSurfaceReady/);
  assert.match(browserView, /setInitialStatusResolved\(true\)/);
  assert.match(browserView, /data-testid="browser-new-tab-page"/);
  assert.match(browserView, /browserStartBrowsing/);
  assert.match(browserView, /browserStartBrowsingHint/);
  assert.match(browserView, /publishCommittedUrl\(st\.url, requestedSessionId\)/);
  assert.match(browserView, /browserTabLabel\(tab, t\.browserEmptyTab\)/);
  assert.ok(
    browserView.indexOf('{/* Keep tabs above the address bar') < browserView.indexOf('{/* Toolbar */}'),
    'the new-tab strip must be above the navigation address bar',
  );
});

test('normal mode uses one Right Dock switcher for artifact and browser entries', () => {
  assert.match(chatView, /data-testid="chat-right-dock-switcher"/);
  assert.match(chatView, /data-testid=\{`chat-right-dock-option-\$\{id\}`\}/);
  assert.match(chatView, /\{ id: 'artifact-preview', label: artifactsLabel/);
  assert.match(chatView, /\{ id: 'browser', label: browserLabel/);
  assert.match(main, /browserDockAvailable=\{browserDockAvailable\}/);
  assert.match(main, /platformCapabilities\.browserNativeDisplay/);
  assert.match(main, /invokeTauri\('browser_prepare', \{ sessionId: requestedSessionId \}\)/);
  assert.match(main, /const \[browserPaneStates, setBrowserPaneStates\] = useState\(\{\}\)/);
  assert.match(main, /browserPaneStateFor\(browserPaneStates, browserSessionId\)/);
  assert.match(main, /rightDockActivePanelId=\{browserDockSelectedPanelId\}/);
  assert.match(main, /onRightDockPanelSelectionChange=\{selectRightDockPanel\}/);
  assert.match(chatView, /panelId="artifact-preview"[\s\S]*visible=\{rightDockActivePanelId !== 'browser'\}/);
  assert.doesNotMatch(main, /data-testid="browser-pane-toggle"/);
  assert.match(browserCommands, /pub async fn browser_prepare/);
  assert.match(browserCommands, /#\[tauri::command\(async\)\]\s+pub fn browser_begin_surface_generation/);
  assert.match(browserCommands, /#\[tauri::command\(async\)\]\s+pub fn browser_hide_native_surface/);
  assert.match(browserCommands, /#\[tauri::command\(async\)\]\s+pub fn browser_hand_back_to_agent/);
  assert.match(browserManager, /pub async fn prepare_for_user/);
  assert.match(nativeHost, /pub fn prepare_unclaimed/);
});

test('native surface suspension is centrally derived for every occlusion path', () => {
  assert.match(main, /const browserSurfaceSuspended = browserResizeActive/);
  assert.match(main, /rightDockState\.activePanelId !== 'browser'/);
  assert.match(main, /rightDockState\.occluded/);
  assert.match(main, /rightDockOcclusionPublications\.length > 0/);
  assert.match(main, /browserDocumentHidden/);
  assert.match(main, /const browserOverlayIntent = \[/);
  assert.match(main, /const browserOverlayOpen = !!publishedBrowserOverlayIntent/);
  assert.match(main, /const browserBlockingLayerOpen = !browserPaneAllowed \|\| browserOverlayOpen/);
  assert.match(
    main,
    /const compactBrowserSurfaceSuspended = browserDocumentHidden[\s\S]{0,120}browserOverlayOpen[\s\S]{0,80}currentView !== 'browser'/,
  );
  assert.match(
    main,
    /isCompactShell && browserActive && currentView === 'browser'[\s\S]{0,280}nativeSurfaceSuspended=\{compactBrowserSurfaceSuspended\}/,
  );
  for (const blocker of [
    'archiveConfirm',
    'searchOverlayOpen',
    'personaEditor',
    'savedConfirm',
    'webAccessOpen',
    'apiKeyGateOpen',
    'vllmSetupModalOpen',
    'bs.pinvouModal',
    'isCompactShell && isSidebarOpen',
    'isCompactShell && mobileMoreOpen',
  ]) {
    assert.ok(main.includes(blocker), `missing native-surface blocker: ${blocker}`);
  }
  assert.match(main, /document\.addEventListener\('visibilitychange', syncVisibility\)/);
  assert.match(main, /window\.addEventListener\('pagehide', handlePageHide\)/);
  assert.match(main, /onResizeActiveChange=\{setBrowserResizeActive\}/);
  assert.match(main, /data-browser-control-slot="ownership"/);
  assert.match(chatView, /useRightDockOcclusion\(\s*'artifact-fullscreen'/);
  assert.match(chatView, /artifactsFullscreen && artifactFullscreenPublicationReady && createPortal/);
  assert.match(chatView, /panelId="artifact-preview"[\s\S]*visible=\{rightDockActivePanelId !== 'browser'\}[\s\S]*onToggleFullscreen=\{\(\) => setArtifactsFullscreen\(true\)\}/);
  assert.match(chatView, /'voice-asr-setup',[\s\S]{0,120}voiceAsrSetup\.open && canInstallLocalAsr/);
  assert.match(chatView, /voiceAsrSetupPublicationReady && \(\(\) =>/);
  assert.match(composerPopover, /useRightDockOcclusion\(`composer-popover-\$\{popoverId\}`, open\)/);
  assert.match(composerPopover, /if \(!open \|\| !publicationReady\) return null/);
  assert.match(attachmentDropOverlay, /useRightDockOcclusion\(`attachment-drop-\$\{overlayId\}`, active\)/);
  assert.match(attachmentDropOverlay, /if \(active && !publicationReady\) return null/);
  assert.match(browserView, /if \(shouldSuspendNativeSurface\) \{[\s\S]{0,160}claimNativeSurfaceHide/);
  assert.match(browserView, /const nativeSurfaceCoordinator = \{/);
  assert.match(browserView, /nativeSurfaceCoordinator\.owner !== owner/);
  assert.match(browserView, /disposed \|\| !ownsNativeSurfaceShow/);
  assert.match(browserView, /let lastShownBoundsKey = ''/);
  assert.match(browserView, /if \(boundsKey === lastShownBoundsKey\) return;/);
  assert.match(browserView, /if \(shown\) lastShownBoundsKey = boundsKey;/);
  assert.match(browserView, /browser_begin_surface_generation/);
  assert.match(browserView, /visibilityGeneration/);
  assert.match(browserView, /visibilitySequence/);
  assert.doesNotMatch(browserView, /visibilityEpoch|nextNativeSurfaceVisibilityEpoch/);
});

test('occluding UI, task switches, and Right Dock state publish only after native hide ACK', () => {
  assert.match(main, /createNativeSurfaceTransitionGate/);
  assert.match(main, /acquireHide: acquireNativeSurfaceTransitionHide/);
  assert.match(main, /channel: 'right-dock'[\s\S]{0,100}hideMode:/);
  assert.match(
    main,
    /channel: 'session'[\s\S]{0,100}hideMode: 'workspace'[\s\S]{0,80}serialize: true/,
  );
  assert.match(main, /setPublishedBrowserOverlayIntent\(browserOverlayIntent\)/);
  assert.match(main, /browserOverlayPublicationReady && createPortal/);
  assert.match(main, /closeBrowserDock\(selectedSessionId\)/);
  assert.match(browserView, /export async function acquireNativeSurfaceTransitionHide/);
  assert.match(browserView, /nativeSurfaceCoordinator\.transitionOwners\.size > 0/);
  assert.match(browserView, /nativeSurfaceCoordinator\.transitionOwners\.add\(owner\)/);
  assert.match(browserView, /nativeSurfaceCoordinator\.transitionOwners\.delete\(owner\)/);
  assert.match(browserView, /const hide = claimNativeSurfaceHide\(owner, sessionId\)/);
  assert.match(browserView, /await hide/);
  assert.match(browserView, /retainOnFailure = false/);
  assert.match(browserView, /createDegradedNativeSurfaceHideLease\(\{/);
  assert.match(browserView, /isFailedNativeSurfaceHideGenerationCurrent\(\{/);
  assert.match(browserView, /retryHide: \(\) => claimNativeSurfaceHide\(owner, sessionId\)/);
  assert.doesNotMatch(browserView, /failedIntentOwner === owner/);
  assert.match(main, /requestBrowserUiCommitAck\(\)\.then/);
  assert.match(main, /useLayoutEffect\(\(\) => \{[\s\S]*?browserUiCommitWaitersRef/);
  assert.match(main, /if \(!browserUiCommitMountedRef\.current\) return Promise\.resolve\(false\)/);
  assert.match(main, /gate\?\.dispose\(\);[\s\S]*?browserUiTransitionGateRef\.current = null/);
  assert.match(
    main,
    /publish: async \(\) => \{[\s\S]*?await publish\(transition\)[\s\S]*?browserTransitionPublishingRef\.current -= 1[\s\S]*?waitForCommit: requestBrowserUiCommitAck/,
  );
  assert.doesNotMatch(main, /startTransition\(/);
  assert.match(main, /browserBridgeSessionTransitionRef/);
  assert.doesNotMatch(main, /browserSessionTransitionPublishingRef/);
  assert.doesNotMatch(main, /browserActiveSessionTransitionIsCurrentRef/);
  assert.match(
    main,
    /bridgeTransition\?\.sessionId !== nextSessionId[\s\S]*?serialize: true/,
  );
});

test('task switches discard stale status and mount a fresh BrowserView instance', () => {
  assert.match(browserView, /const sessionIdRef = useRef\(sessionId\)/);
  assert.match(browserView, /const statusRequestEpochRef = useRef\(0\)/);
  assert.match(browserView, /const tabsRequestEpochRef = useRef\(0\)/);
  assert.match(browserView, /sessionIdRef\.current === requestedSessionId/);
  assert.match(browserView, /st\?\.sessionId !== requestedSessionId/);
  const keyedBrowserViews = main.match(/<BrowserView\s+[\s\S]{0,100}?key=\{browserViewSessionId\}/g) || [];
  assert.equal(keyedBrowserViews.length, 2, 'compact and dock BrowserView instances must both be keyed by session');
});

test('user takeover is visible, recovers after idle, and supports immediate handoff', () => {
  assert.match(browserView, /listenTauri\('browser:control-changed'/);
  assert.match(browserView, /data-testid="browser-control-owner"/);
  assert.match(browserView, /data-testid="browser-hand-back"/);
  assert.match(browserView, /invokeTauri\('browser_hand_back_to_agent', \{ sessionId \}\)/);
  assert.match(browserView, /createPortal\(ownershipControl, ownershipSlot\)/);
  assert.match(main, /ownershipSlot=\{browserOwnershipSlot\}/);
  assert.match(nativeHost, /const USER_CONTROL_IDLE_TIMEOUT: Duration = Duration::from_secs\(3\)/);
  assert.match(nativeHost, /release_user_control_if_idle/);
  assert.match(nativeState, /release_user_control_if_unchanged/);
  assert.match(browserManager, /pub\(crate\) fn release_user_control_if_idle/);
  assert.doesNotMatch(
    nativeHost,
    /lastSignalAt/,
    'page-global deduplication would swallow a real user takeover after an Agent event',
  );
});

test('sessionless browser events fail closed without creating a global compatibility workspace', () => {
  assert.doesNotMatch(main, /__global__/);
  assert.match(main, /const sessionId = event\.payload\?\.sessionId;[\s\S]{0,80}if \(!sessionId\) return;/);
  assert.match(
    main,
    /const browserActive = browserNativeDisplayAvailable\s*&& !!\(browserSessionId && browserSessions\[browserSessionId\]\)/,
  );
  assert.match(browserView, /if \(payload\.sessionId !== sessionId\) return;/);
  assert.doesNotMatch(browserCommands, /session_id: Option<String>/);
  assert.doesNotMatch(browserCommands, /session_id\.as_deref\(\)/);
  assert.doesNotMatch(
    browserCommands,
    /mgr\.(?:stop_for_session|status|navigate|go_back|go_forward|reload|list_tabs|create_tab|close_tab|activate_tab)\(Some\(&session_id\)/,
  );
  assert.doesNotMatch(browserManager, /browser_session_id: Option<&str>/);
  assert.doesNotMatch(browserManager, /pub async fn input_event/);

  const cdpEventLoop = browserManager.slice(
    browserManager.indexOf('async fn run_event_loop'),
    browserManager.indexOf('fn is_allowed_url'),
  );
  assert.doesNotMatch(cdpEventLoop, /app\.emit\("browser:(?:navigation|tabs-changed)"/);
});

test('host requests wake through file events without high-frequency idle polling', () => {
  assert.match(browserManager, /notify::recommended_watcher/);
  const watcherStart = browserManager.indexOf('pub fn spawn_watch');
  const watcherEnd = browserManager.indexOf('async fn reattach_existing', watcherStart);
  const spawnWatch = browserManager.slice(watcherStart, watcherEnd);
  const controlRequest = spawnWatch.indexOf('prepare_requested_native_control_requests');
  const controlPoll = spawnWatch.indexOf('sleep(Duration::from_millis(100))');
  const dataRequest = spawnWatch.indexOf('prepare_requested_native_surfaces(&request_app)');
  const dataPoll = spawnWatch.indexOf('sleep(Duration::from_secs(1))');
  assert.ok(
    controlRequest >= 0 && controlPoll > controlRequest,
    'the lightweight control plane must poll within the operation-heartbeat budget',
  );
  assert.ok(
    dataRequest > controlPoll && dataPoll > dataRequest,
    'the potentially slow data plane must remain on the one-second reconciliation loop',
  );
  assert.equal(
    (spawnWatch.match(/sleep\(Duration::from_millis\(100\)\)/g) || []).length,
    1,
    'high-frequency polling is reserved for the lightweight control plane',
  );
});

test('app automation never starts external Chrome after host failure', () => {
  assert.match(browserManager, /let port = live_port\(\)/);
  assert.doesNotMatch(browserManager, /self\.acquire_or_start_chrome\(\)\.await/);
  assert.match(browserManager, /parse_host_owned_port_json/);
  assert.match(browserManager, /native_surface\.lock\(\)\.owns_port\(port\)/);
});

test('session-scoped browser lookup fails closed without falling back to global CDP', () => {
  const failures = browserManager.match(
    /Native browser workspace for the specified conversation(?: or tab)? does not exist/g,
  ) || [];
  assert.ok(failures.length >= 7, `expected fail-closed guards for scoped operations, got ${failures.length}`);
  assert.match(browserManager, /"restoreError": error/);
  assert.match(browserManager, /"missing": true/);
  assert.match(main, /\(!st\.running && !st\.restoreError\)/);
  assert.match(browserView, /setError\(st\.restoreError \|\| ''\)/);
});

test('post-prepare failure rolls back only the newly created workspace', () => {
  assert.match(browserManager, /fn rollback_new_native_workspace/);
  assert.match(browserManager, /surface\.close_session\(Some\(app\), session_id\)/);
  assert.doesNotMatch(
    browserManager,
    /if !probe_cdp\([\s\S]{0,300}native_surface\.lock\(\)\.close\(/,
  );
});

test('exit events do not wait on the browser lock and always clean coordination files', () => {
  const start = browserManager.indexOf('pub fn shutdown_on_exit');
  const end = browserManager.indexOf('pub async fn status', start);
  assert.ok(start >= 0 && end > start, 'shutdown_on_exit body must remain discoverable');
  const shutdown = browserManager.slice(start, end);

  const persistenceTry = shutdown.indexOf('persistence_io.try_lock()');
  const nativeTry = shutdown.indexOf('native_surface.try_lock()');
  assert.ok(
    persistenceTry >= 0 && nativeTry > persistenceTry,
    'exit must preserve persistence_io -> native_surface lock order without waiting',
  );
  assert.doesNotMatch(
    shutdown,
    /self\.[a-z_]+\.lock\(\)/,
    'the Tauri exit thread must not block on any BrowserManager lock',
  );
  assert.doesNotMatch(shutdown, /\breturn\s*;/, 'busy-lock paths must still reach coordination cleanup');

  const portCleanup = [...shutdown.matchAll(/remove_file\(paths::browser_cdp_port_json\(\)\)/g)];
  const hostCleanup = [...shutdown.matchAll(/clear_host_request_files\(\)/g)];
  assert.equal(portCleanup.length, 1, 'port cleanup must have one unconditional exit path');
  assert.equal(hostCleanup.length, 1, 'host-request cleanup must have one unconditional exit path');
  assert.ok(
    portCleanup[0].index > shutdown.indexOf('self.inner.try_lock()')
      && hostCleanup[0].index > portCleanup[0].index,
    'coordination cleanup must run after the best-effort in-memory cleanup branch',
  );

  const exitStart = tauriApp.indexOf('tauri::RunEvent::Exit =>');
  const exitEnd = tauriApp.indexOf('tauri::RunEvent::Resumed', exitStart);
  assert.ok(exitStart >= 0 && exitEnd > exitStart, 'Tauri exit handler must remain discoverable');
  const exitHandler = tauriApp.slice(exitStart, exitEnd);
  assert.match(exitHandler, /shutdown_browser_before_process_end\(app\)/);
  const shutdownHelper = tauriApp.slice(
    tauriApp.indexOf('fn shutdown_browser_before_process_end'),
    tauriApp.indexOf('pub(crate) async fn prepare_app_restart'),
  );
  assert.match(shutdownHelper, /browser\.shutdown_on_exit\(\)/);
  assert.doesNotMatch(
    exitHandler,
    /mgr\.stop\(\)\.await/,
    'exit fallback must not delete restore data after a busy preserving shutdown',
  );
});

test('navigation ACK confirms dispatch only and page state verifies loading', () => {
  assert.doesNotMatch(browserManager, /Opened page:/);
  assert.doesNotMatch(browserManager, /Navigation requested:/);
  const acknowledgements = browserManager.match(/page load is not verified/g) || [];
  assert.equal(acknowledgements.length, 3);
  assert.match(browserManager, /"navigationDispatched": true/);
  assert.match(browserManager, /"loadVerified": false/);
  assert.match(browserManager, /Call take_snapshot or list_pages to verify/);
});

test('status and persistence prefer host-committed URL and recover only from valid live URL', () => {
  assert.match(nativeState, /last_known_url: Arc<parking_lot::RwLock<String>>/);
  assert.match(nativeState, /pub\(super\) fn remember_url/);
  const sessionState = nativeHost.slice(
    nativeHost.indexOf('pub fn session_state'),
    nativeHost.indexOf('pub fn tab_token_for_page_id'),
  );
  const persistence = nativeHost.slice(
    nativeHost.indexOf('pub fn persist_restore_workspace'),
    nativeHost.indexOf('pub fn persist_all_restore'),
  );
  assert.match(sessionState, /resolve_surface_url\(entry, Some\(&webview\)\)/);
  assert.match(persistence, /resolve_surface_url\(entry, Some\(&webview\)\)\?/);
  assert.doesNotMatch(sessionState, /\.url\(\)\.ok\(\)\?/);
  assert.match(nativeHost, /Browser tab has no recoverable top-level URL/);

  const resolver = nativeHost.slice(
    nativeHost.indexOf('fn resolve_surface_url_value'),
    nativeHost.indexOf('fn has_internal_marker_for_token'),
  );
  assert.ok(
    resolver.indexOf('if let Some(fallback) = fallback') <
      resolver.lastIndexOf('if let Some(url) = live_url'),
    'a valid host-owned Finished commit must win over every sampled WebView URL',
  );
  assert.match(resolver, /has_internal_marker/);
  assert.match(resolver, /is_browser_core_binding_url/);
  assert.match(resolver, /if url == "about:blank"/);
  assert.match(resolver, /entry\.remember_url\(&url\)/);
  assert.match(nativeHost, /pinvou-location-change/);
  assert.match(nativeHost, /location_change_signal_nonce/);
  assert.match(nativeHost, /signalLocationChange/);
  assert.match(nativeState, /last_known_title/);
  assert.match(nativeHost, /entry\.title_for_url\(&url\)/);
});

test('app restart rebuilds page identities from URL inventory and restores the dock entry', () => {
  assert.match(browserPaths, /browser_workspace_restore_json/);
  const pageIdAllocator = nativeHost.slice(
    nativeHost.indexOf('const MAX_SAFE_PAGE_ID'),
    nativeHost.indexOf('#[derive(Debug, Deserialize)]'),
  );
  assert.match(pageIdAllocator, /NATIVE_PAGE_ID_INCARNATION/);
  assert.match(pageIdAllocator, /rand::random::<u64>/);
  assert.match(pageIdAllocator, /NATIVE_PAGE_ID_SEQUENCE_BITS/);
  assert.match(pageIdAllocator, /page_id <= MAX_SAFE_PAGE_ID/);
  assert.doesNotMatch(pageIdAllocator, /std::process::id/);
  assert.match(browserManager, /restore_saved_workspace\(browser_session_id\)\.await/);
  assert.match(browserManager, /prepare_restored_surface/);
  assert.match(browserManager, /discover_native_target\(port, tab_token\)/);
  assert.match(browserManager, /"browser:activated"[\s\S]{0,120}"restored": true/);
  assert.match(browserManager, /persist_all_restore/);
  assert.match(browserManager, /close_preserving_restore/);
  assert.match(browserManager, /delete_restore_workspace\(browser_session_id\)/);
  assert.match(nativeHost, /atomic_write_private_anchored\(path, &encoded\)/);
  const restoreWriter = nativeHost.slice(
    nativeHost.indexOf('fn write_restore_workspace_file'),
    nativeHost.indexOf('fn remove_workspace_state_file'),
  );
  assert.doesNotMatch(restoreWriter, /create_dir_all|make_private_dir/);
  assert.doesNotMatch(restoreWriter, /target_id|lease|session_token|tab_token/);
  assert.match(nativeState, /NativeControlOwner \{[\s\S]{0,120}Unclaimed/);
  assert.match(nativeHost, /WorkspaceControl::new\(1, NativeControlOwner::Unclaimed\)/);
  assert.doesNotMatch(
    browserManager.slice(
      browserManager.indexOf('async fn restore_saved_workspace'),
      browserManager.indexOf('fn rollback_new_native_workspace'),
    ),
    /\.activate_tab\(/,
  );
  assert.match(browserView, /controlOwner === 'unclaimed'/);
  const sessionStop = browserManager.slice(
    browserManager.indexOf('pub async fn stop_for_session'),
    browserManager.indexOf('pub async fn delete_for_session'),
  );
  assert.ok(
    sessionStop.indexOf('delete_restore_workspace(browser_session_id)?')
      < sessionStop.indexOf('close_session_preserving_restore'),
    'stop must durably remove the restore point before irreversible WebView close',
  );
  assert.ok(
    sessionStop.indexOf('let mut surface = self.native_surface.lock();')
      < sessionStop.lastIndexOf('delete_restore_workspace(browser_session_id)?'),
    'stop must lock native mutations before deleting the restore point so UI writes cannot resurrect it',
  );
  assert.doesNotMatch(
    sessionStop,
    /write_restore_workspace/,
    'partial close must not resurrect the complete pre-close manifest',
  );
  const closeSession = nativeHost.slice(
    nativeHost.indexOf('fn close_session_impl'),
    nativeHost.indexOf('fn close_staged_for_session'),
  );
  assert.match(
    closeSession,
    /workspace\.tabs\.iter\(\)\.any\(SurfaceEntry::is_published\)[\s\S]{0,220}persist_restore_workspace\(app, session_id\)/,
    'published partial-close survivors must become the new restore truth',
  );
  assert.match(
    closeSession,
    /if workspace_empty[\s\S]{0,700}if delete_restore[\s\S]{0,180}remove_restore_file/,
    'a fully closed workspace must delete its restore point',
  );
  const closeReconcile = nativeHost.slice(
    nativeHost.indexOf('fn reconcile_workspace_close'),
    nativeHost.indexOf('fn reconcile_staged_close'),
  );
  assert.match(
    closeReconcile,
    /Ok\(\(\)\)[\s\S]{0,180}remove_tab_from_workspace[\s\S]{0,120}Err\(error\) => errors\.push/,
    'irreversible close must remove successful entries and retain only failed survivors',
  );
});

test('Agent new tab publishes lease CAS only after target discovery and initial navigation', () => {
  assert.match(nativeHost, /staged_tabs: HashMap/);
  assert.match(nativeHost, /webview[\s\S]{0,120}\.navigate\(requested_url\)/);
  assert.match(nativeHost, /commit_agent_mutation\([\s\S]{0,1000}workspace\.tabs\.insert/);
  assert.match(nativeHost, /staged_publication\.store\(true/);
  assert.match(nativeHost, /created_at_revision/);
  assert.match(nativeHost, /commit_agent_generation_rollback/);
  assert.doesNotMatch(browserManager, /create_tab_for_agent\([\s\S]{0,900}navigate_tab_after_bind/);
});

test('BrowserCore tab commits marker, WebDriver bind, initial navigation, then publication', () => {
  const resolver = browserManager.slice(
    browserManager.indexOf('async fn bind_staged_native_target'),
    browserManager.indexOf('fn rollback_staged_agent_tab'),
  );
  assert.match(
    resolver,
    /if platform::browser_core_available\(\)[\s\S]{0,900}bind_browser_core_webview\(&webview\)\.await\?[\s\S]{0,180}native:\{tab_token\}/,
  );
  const browserCoreBranch = resolver.slice(0, resolver.indexOf('let port = live_port()'));
  assert.doesNotMatch(browserCoreBranch, /live_port|discover_native_target|owns_port/);

  const coreNewPage = browserManager.slice(
    browserManager.indexOf('if tool_name == "new_page"'),
    browserManager.indexOf('let page_id = arguments.get("pageId")'),
  );
  const createIndex = coreNewPage.indexOf('create_tab_for_agent(');
  const bindIndex = coreNewPage.indexOf('bind_staged_native_target(', createIndex);
  const commitIndex = coreNewPage.indexOf('commit_created_tab_for_agent(', bindIndex);
  assert.ok(
    createIndex >= 0 && createIndex < bindIndex && bindIndex < commitIndex,
    'BrowserCore new tab must create a marker, bind the native target, then publish',
  );
  assert.doesNotMatch(coreNewPage, /commit_created_tab_for_agent\([\s\S]{0,900}bind_browser_core_webview/);

  const userCreate = browserManager.slice(
    browserManager.indexOf('async fn create_native_bound_tab'),
    browserManager.indexOf('pub async fn create_tab'),
  );
  assert.match(
    userCreate,
    /about:blank#pinvou-tab-[\s\S]{0,900}bind_staged_native_target\([\s\S]{0,900}bind_target\([\s\S]{0,900}navigate_tab_after_bind\(/,
  );
  assert.match(nativeHost, /staged_user_tabs: HashMap/);
  assert.match(nativeHost, /requires_automation_binding[\s\S]{0,240}capabilities\(\)\.agent_automation/);
  assert.match(nativeHost, /candidate\.publish\(\)/);
});

test('Linux WebDriver safely rebinds with a host marker without injecting remote-page identity', () => {
  assert.match(nativePlatform, /register_browser_core_webview_binding/);
  assert.match(
    nativeHost,
    /register_browser_core_webview_binding\(\s*&label,\s*tab_token,\s*&control,\s*&user_navigation,\s*has_internal_marker_for_token\(url,\s*tab_token\),\s*\)/,
  );
  const registration = linuxAutomation.slice(
    linuxAutomation.indexOf('pub(super) fn register_webview_binding'),
    linuxAutomation.indexOf('pub(super) fn unregister_webview_binding'),
  );
  assert.doesNotMatch(registration, /globalThis|Object\.defineProperty|BINDING_NONCE_PROPERTY/);
  assert.match(registration, /tab_token: &str/);
  assert.match(registration, /Arc::downgrade\(control\)/);
  assert.match(linuxAutomation, /control: Weak<WorkspaceControl>/);
  const marker = linuxAutomation.slice(
    linuxAutomation.indexOf('fn binding_marker_url'),
    linuxAutomation.indexOf('pub(super) async fn wait_until_ready'),
  );
  assert.match(marker, /BINDING_MARKER_PREFIX/);
  assert.match(linuxAutomation, /BINDING_MARKER_PREFIX: &str = "about:blank#pinvou-webdriver-bind-"/);
  assert.match(
    linuxAutomation,
    /dispatch_guarded_binding_navigation\(\s*webview,\s*&label,\s*authorization,\s*None,\s*move \|webview\|[\s\S]{0,700}webview\.navigate\(marker_url\)/,
  );
  assert.match(linuxAutomation, /locate_binding_marker_locked/);
  assert.match(
    linuxAutomation,
    /dispatch_guarded_binding_navigation\(\s*webview,\s*&label,\s*authorization,\s*Some\(&binding_generation\),\s*move \|webview\|[\s\S]{0,700}webview\.navigate\(restore_url\)/,
  );
  const guardedDispatch = linuxAutomation.slice(
    linuxAutomation.indexOf('fn dispatch_guarded_binding_navigation'),
    linuxAutomation.indexOf('fn ready_session_for_live_process'),
  );
  assert.match(linuxAutomation, /fn validate_binding_navigation_generation[\s\S]{0,900}navigation_state\.navigation_admission_busy\(\)/);
  assert.match(guardedDispatch, /let mut navigation_state = navigation\.lock\(\)/);
  assert.match(guardedDispatch, /binding_registration_matches\(label, &generation\)/);
  assert.match(
    guardedDispatch,
    /control[\s\S]{0,160}\.dispatch_if_agent_authorized\(authorization, dispatch_with_navigation\)/,
  );
  assert.match(linuxAutomation, /active_binding_nonce: Option<String>/);
  // The marker-window semantics are enforced by active_binding_nonce (armed on
  // rotation, cleared on restore/foreign navigation in classify_binding_navigation);
  // no separate "marker seen" flag is tracked.
  assert.match(linuxAutomation, /registered_host_bootstrap: bool/);
  assert.match(linuxAutomation, /host_bootstrap_pending: bool/);
  assert.match(linuxAutomation, /host_bootstrap_settled: Arc<tokio::sync::Notify>/);
  assert.match(
    linuxAutomation,
    /fn classify_binding_navigation[\s\S]{0,900}strip_prefix\(BINDING_MARKER_PREFIX\)/,
  );
  const classifyBinding = linuxAutomation.slice(
    linuxAutomation.indexOf('pub(super) fn classify_binding_navigation'),
    linuxAutomation.indexOf('fn arm_binding_restore_url'),
  );
  assert.match(
    classifyBinding,
    /active_binding_restore_url[\s\S]{0,360}return true;[\s\S]{0,420}binding\.active_binding_nonce = None;[\s\S]{0,180}\bfalse\s*}/,
  );
  assert.doesNotMatch(
    linuxAutomation,
    /is_pre_binding_page_load|active_binding_original_is_host_bootstrap/,
  );

  const bootstrapSettle = linuxAutomation.slice(
    linuxAutomation.indexOf('pub(super) fn settle_host_bootstrap_page_load'),
    linuxAutomation.indexOf('async fn wait_for_host_bootstrap_and_rotate'),
  );
  assert.match(
    bootstrapSettle,
    /registered_host_bootstrap[\s\S]{0,180}host_bootstrap_pending[\s\S]{0,180}!payload_exact && !live_exact/,
  );

  const bootstrapWait = linuxAutomation.slice(
    linuxAutomation.indexOf('async fn wait_for_host_bootstrap_and_rotate'),
    linuxAutomation.indexOf('fn fresh_binding_nonce'),
  );
  assert.match(
    bootstrapWait,
    /binding\.nonce != expected_registration_nonce[\s\S]{0,360}binding\.host_bootstrap_pending[\s\S]{0,360}if !pending[\s\S]{0,260}rotate_binding_nonce_locked/,
  );
  assert.match(bootstrapWait, /host_bootstrap_settled[\s\S]{0,120}notified_owned\(\)/);
  assert.match(bootstrapWait, /timeout_at\(deadline, notification\)/);
  assert.match(bootstrapWait, /browser\/webkit-host-bootstrap-settle-timeout/);

  const selectWebview = linuxAutomation.slice(
    linuxAutomation.indexOf('async fn select_webview_locked'),
    linuxAutomation.indexOf('async fn element_for_uid_locked'),
  );
  assert.match(
    selectWebview,
    /let expected_nonce =[\s\S]{0,120}wait_for_host_bootstrap_and_rotate\(&label, &registration_nonce\)\.await\?/,
  );
  assert.doesNotMatch(
    selectWebview,
    /wait_for_host_bootstrap_and_rotate[\s\S]{0,240}rotate_binding_nonce_if_current/,
  );

  const finishIndex = nativeHost.indexOf(
    'committed_user_navigation.lock().finish(&committed_url)',
  );
  const settleIndex = nativeHost.indexOf(
    'super::settle_browser_core_host_bootstrap(',
    finishIndex,
  );
  assert.ok(
    finishIndex >= 0 && settleIndex > finishIndex,
    'only a Finished callback accepted as Current may release the bootstrap barrier',
  );
  assert.match(
    nativeHost,
    /let binding_marker = super::classify_browser_core_binding_navigation/,
  );
  assert.match(nativeHost, /if binding_marker \{\s*return true;/);
  assert.doesNotMatch(
    linuxAutomation,
    /BINDING_NONCE_PROPERTY|binding_challenge_script|current_binding_nonce_locked/,
  );
  assert.doesNotMatch(marker, /session_id|session_token|tab_token|target_id|authorization|lease|control/);
  const pageScript = linuxAutomation.slice(
    linuxAutomation.indexOf('async fn element_for_uid_locked'),
    linuxAutomation.indexOf('async fn active_element_locked'),
  );
  assert.doesNotMatch(pageScript, /session_id|session_token|tab_token|target_id|authorization|lease/);
  assert.match(
    nativeHost,
    /browser_initialization_script\(cdp_tab_token, &location_signal_nonce\)/,
  );
  assert.doesNotMatch(nativeHost, /BINDING_NONCE_PROPERTY/);
  assert.match(nativeHost, /browser_core_page_script_contains_no_task_or_tab_identity/);
});

test('each Linux WebDriver mutation revalidates exact tab and active lease before POST', () => {
  assert.match(
    linuxAutomation,
    /fn authorize_registered_mutation[\s\S]{0,900}binding\.tab_token != authorization\.tab_token[\s\S]{0,700}refresh_agent_input_window\(authorization\)[\s\S]{0,180}authorize_agent_dispatch\(authorization\)/,
  );
  assert.match(
    linuxAutomation,
    /async fn request_authorized_locked[\s\S]{0,900}current_session_locked\(\)[\s\S]{0,260}authorize_registered_mutation\(label, authorization, emits_takeover_signal\)\?[\s\S]{0,180}raw_request\(session, method, path, body\)/,
  );
  assert.match(linuxAutomation, /browser\/webkit-session-changed-before-dispatch/);
  assert.match(linuxAutomation, /browser\/action-partially-committed/);
  const mutationDispatch = linuxAutomation.slice(
    linuxAutomation.indexOf('pub(super) async fn dispatch_input'),
    linuxAutomation.indexOf('pub(super) async fn shutdown_for_stop'),
  );
  assert.match(mutationDispatch, /"actions"/);
  assert.match(mutationDispatch, /element\/\{element\}\/click/);
  assert.match(mutationDispatch, /element\/\{element\}\/clear/);
  assert.match(mutationDispatch, /send_keys_to_element_locked/);
  assert.match(mutationDispatch, /"alert\/text"/);
  assert.match(mutationDispatch, /request_authorized_locked/g);
  assert.doesNotMatch(
    mutationDispatch,
    /request_locked\(\s*Method::POST,\s*(?:"actions"|&format!\("element\/\{element\}\/(?:click|clear)"\)|endpoint)/,
  );
});

test('BrowserCore reuses the product blank page and closes the shared driver at last stop', () => {
  assert.match(browserManager, /should_reuse_browser_core_initial_tab/);
  assert.match(browserManager, /fn should_reuse_browser_core_initial_tab[\s\S]{0,300}!background/);
  assert.match(browserManager, /"reusedInitialBlank": true/);
  const stop = browserManager.slice(
    browserManager.indexOf('async fn stop_with_start_lock'),
    browserManager.indexOf('pub async fn stop_for_session'),
  );
  assert.match(stop, /platform::shutdown_browser_core_for_stop\(\)\.await/);
  assert.match(browserManager, /platform::shutdown_browser_core_for_exit\(\)/);
});

test('BrowserCore pageId is stable and page tools fail closed on missing or malformed identity', () => {
  const coreDispatch = browserManager.slice(
    browserManager.indexOf('async fn handle_browser_core_tool'),
    browserManager.indexOf('async fn rollback_staged_agent_tab'),
  );
  assert.match(coreDispatch, /let page_id = tab[\s\S]{0,100}\.page_id/);
  assert.match(coreDispatch, /tab_token_for_page_id\(&request\.session_id, page_id\)/);
  assert.match(coreDispatch, /browser\/missing-argument: pageId/);
  assert.match(coreDispatch, /browser\/invalid-argument: pageId/);
  assert.doesNotMatch(coreDispatch, /\.enumerate\(\)[\s\S]{0,120}"id": page_id/);
  assert.doesNotMatch(coreDispatch, /tabs\s*\.get\(index\)/);
});

test('host protocol v3 echoes identity and scopes automation-loss events by session', () => {
  assert.match(browserManager, /if protocol_version != 3/);
  assert.match(browserManager, /"protocol_version": request\.protocol_version/);
  assert.match(browserManager, /"request_id": request\.request_id/);
  assert.match(browserManager, /"idempotency_key": request\.idempotency_key/);
  assert.match(browserManager, /for session_id in session_ids[\s\S]{0,220}"browser:automation-unavailable"[\s\S]{0,120}"sessionId": session_id/);
  assert.match(browserManager, /RefreshAgentInput/);
  assert.match(
    browserManager,
    /HostedBrowserOperation::RefreshAgentInput[\s\S]{0,260}refresh_agent_input\(&lease\)/,
  );
  assert.match(
    nativeHost,
    /pub fn refresh_agent_input[\s\S]{0,500}assert_lease\(lease\)[\s\S]{0,500}refresh_agent_input_window\(lease\)/,
  );
});

test('persistence failures after irreversible operations are visible and retried with backoff', () => {
  assert.match(browserManager, /persistence_io: parking_lot::Mutex<\(\)>/);
  assert.match(browserManager, /persistence_warnings: parking_lot::Mutex<HashMap<String, String>>/);
  assert.match(browserManager, /persistence_retries: parking_lot::Mutex<HashSet<String>>/);
  assert.match(browserManager, /let _persistence_guard = (?:self|manager)\.persistence_io\.lock\(\)/);
  assert.match(browserManager, /"browser:persistence-warning"/);
  assert.match(browserManager, /"browser:persistence-restored"/);
  assert.match(browserManager, /delay = delay\.saturating_mul\(2\)\.min\(Duration::from_secs\(30\)\)/);
  assert.match(browserManager, /status\["persistenceWarning"\] = json!\(warning\)/);
});

test('popup reuses only a fully begun lease and other popups become User-controlled', () => {
  const popupHandler = nativeHost.slice(
    nativeHost.indexOf('.on_new_window'),
    nativeHost.indexOf('let webview = match window.add_child'),
  );
  assert.match(popupHandler, /popup_agent_authorization/);
  assert.match(popupHandler, /create_popup_tab\(&session_id, url\.to_string\(\), authorization\)/);
  assert.doesNotMatch(popupHandler, /agent_input_in_progress/);
  assert.doesNotMatch(popupHandler, /agent_initiated/);
  assert.match(browserManager, /if let Some\(retained\) = authorization/);
  assert.match(browserManager, /create_agent_popup_tab/);
  const agentPopup = browserManager.slice(
    browserManager.indexOf('async fn create_agent_popup_tab'),
    browserManager.indexOf('async fn create_native_bound_tab'),
  );
  assert.match(agentPopup, /create_tab_for_agent\(/);
  assert.match(agentPopup, /ensure_hosted_caller_epoch_live\(/);
  assert.match(agentPopup, /authorize_popup_agent_operation\(retained\)/);
  assert.match(agentPopup, /commit_created_tab_for_agent\(/);
  assert.match(browserManager, /create_native_bound_tab\(&app, browser_session_id, url, false\)/);
});

test('restore cancellation and orphan cleanup preserve data for existing sessions', () => {
  assert.match(browserManager, /RestoredExisting => Some\("restored_session"\)/);
  assert.match(browserManager, /record_prepare_generation/);
  assert.match(browserManager, /prepare_generation_revision/);
  assert.match(browserManager, /rollback_prepared_session/);
  assert.match(nativeHost, /rollback_prepare_generation/);
  assert.match(browserManager, /close_session_preserving_restore/);
  assert.match(browserManager, /pub fn reconcile_session_files/);
  assert.match(browserManager, /browser_workspace_restore_dir/);
  assert.match(browserManager, /browser_workspaces_dir/);
  assert.match(browserManager, /browser_session_mcp_dir/);
  assert.match(browserManager, /bind_session_validator/);
  assert.match(browserManager, /mark_session_deleted/);
  assert.match(browserManager, /ensure_browser_session_allowed/);
  assert.match(browserManager, /startup_reconcile_cutoff: SystemTime/);
  assert.match(browserManager, /modified >= startup_cutoff/);
});

test('transient host requests are not replayed across processes and cancellation waits for ACK', () => {
  const spawnWatch = browserManager.slice(
    browserManager.indexOf('pub fn spawn_watch'),
    browserManager.indexOf('// -----------------------------------------------------------------------\n    // Lifecycle'),
  );
  assert.match(spawnWatch, /reset_host_request_directory_for_process_start/);
  assert.match(browserManager, /std::fs::rename\(request_dir, &candidate\)/);
  assert.match(nativeState, /CancelAwaitingCompletion/);
  assert.match(nativeState, /CancelPendingRollback\(Value\)/);
  assert.match(nativeState, /acknowledge_cancellation/);
  assert.match(browserManager, /acknowledge_request_cancellation/);
  assert.match(browserManager, /tokio::time::sleep\(Duration::from_millis\(250\)\)/);

  const scopedStop = browserManager.slice(
    browserManager.indexOf('async fn stop_with_start_lock'),
    browserManager.indexOf('pub async fn stop_for_session'),
  );
  assert.doesNotMatch(scopedStop, /clear_host_request_files/);
});

test('one failing cancellation compensation does not starve other session requests', () => {
  const prepareRequests = browserManager.slice(
    browserManager.indexOf('async fn prepare_requested_native_surfaces'),
    browserManager.indexOf('async fn process_hosted_cancellation'),
  );
  assert.match(prepareRequests, /let mut errors = Vec::new\(\)/);
  assert.match(prepareRequests, /blocked_requests\.insert\(cancellation_path\.with_extension\("json"\)\)/);
  assert.match(prepareRequests, /for request_path in request_paths/);
  assert.match(prepareRequests, /if blocked_requests\.contains\(&request_path\)/);
  assert.match(prepareRequests, /errors\.push\(format!\("\{\}: \{error\}"/);
  assert.match(prepareRequests, /Err\(errors\.join\("; "\)\)/);
  // Collect every artifact error in this pass; `?` must not abort the scan early.
  assert.doesNotMatch(prepareRequests, /\.await\?/);
  assert.doesNotMatch(prepareRequests, /write_hosted_response\([^;]+\)\?/);
});

test('Host Core precisely compensates cancellation persistence before operation and staging ACK', () => {
  const prepareRequests = browserManager.slice(
    browserManager.indexOf('async fn prepare_requested_native_surfaces'),
    browserManager.indexOf('async fn process_hosted_cancellation'),
  );
  assert.match(
    prepareRequests,
    /matches!\(request\.operation, HostedBrowserOperation::CoreTool\)/,
  );
  assert.match(prepareRequests, /tokio::select!\s*\{\s*biased;/);
  assert.match(
    prepareRequests,
    /wait_for_hosted_cancellation\(&cancellation_path\)/,
  );
  assert.match(prepareRequests, /core_cancellation_needs_compensation/);
  assert.match(prepareRequests, /"kind": "cancelled_core_request"/);
  assert.match(
    browserManager,
    /Some\("cancelled_core_request"\)[\s\S]*cancel_in_flight_core_request/,
  );
  assert.match(nativeHost, /pub fn cancel_in_flight_core_request/);
  assert.match(nativeHost, /cancel_agent_operation_for_session\(session_id\)/);
  assert.match(nativeHost, /created_by_request_id\.as_deref\(\) == Some\(request_id\)/);
  assert.match(nativeState, /pub\(super\) fn cancel_agent_operation_for_session/);
  assert.match(
    browserManager,
    /async fn wait_for_hosted_cancellation\(path: &Path\)/,
  );
});

test('native visibility uses host generation and sequence so stale renderer requests are inert', () => {
  assert.match(browserManager, /surface_visibility: parking_lot::Mutex<SurfaceVisibilityClock>/);
  assert.match(browserManager, /pub fn begin_surface_generation/);
  assert.match(browserManager, /visibility\.claim\(visibility_generation, visibility_sequence\)/);
  assert.match(browserCommands, /browser_begin_surface_generation/);
  assert.match(browserCommands, /visibility_generation: u64/);
  assert.match(browserCommands, /visibility_sequence: u64/);
});

test('User create, activate, and close display failures have explicit commit boundaries', () => {
  assert.match(nativeHost, /Failed to roll back user-created tab mapping/);
  assert.match(nativeHost, /workspace\.tabs\.remove_token\(tab_token\)/);
  assert.match(nativeHost, /Failed to roll back user tab activation mapping/);
  assert.match(nativeHost, /workspace\.active_tab = previous_active/);
  assert.match(nativeHost, /Failed to show fallback page after Agent tab close/);
  assert.match(nativeHost, /Failed to show fallback page after user tab close/);
  assert.match(nativeHost, /Failed to roll back new-tab display/);
});

test('one global native surface prevents background Agent activation stealing the foreground', () => {
  assert.match(nativeHost, /set_exclusive_workspace_visibility\(&mut self\.workspaces, session_id\)/);
  assert.match(nativeHost, /workspace\.visible = workspace_session_id == session_id/);
  assert.match(
    nativeHost,
    /self\.active_session\.as_deref\(\) == Some\(session_id\)[\s\S]{0,900}workspace_may_present_native_surface\([\s\S]{0,120}session_owns_visible_surface,[\s\S]{0,120}workspace\.visible/,
  );
  assert.match(nativeHost, /session_owns_visible_surface && workspace_visible/);
});

test('remote pages lack global clipboard access and downloads fail closed without path leakage', () => {
  assert.doesNotMatch(nativeHost, /enable_clipboard_access/);
  assert.match(nativeHost, /\.on_download\(/);
  assert.match(nativeHost, /"browser:download-blocked"/);
  assert.match(nativeHost, /"sessionId": download_session_id/);
  assert.match(nativeHost, /"tab": download_tab_token/);
  assert.match(nativeHost, /let source = match \(url\.scheme\(\), url\.host_str\(\)\)/);
  assert.doesNotMatch(nativeHost, /"destination"|"path": destination/);
  assert.match(browserView, /listenTauri\('browser:download-blocked'/);
  assert.match(browserView, /if \(payload\.sessionId !== sessionId\) return;/);
  assert.match(browserView, /t\.browserDownloadBlocked\(payload\.source \|\| ''\)/);
});
