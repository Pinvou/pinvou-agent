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
const nativeSurfaceTransition = readFileSync(
  path.join(projectRoot, 'src', 'features', 'browser', 'native-surface-transition.mjs'),
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

test('应用 IPC capability 精确授予命名 WebView，不向窗口内浏览器子表面继承', () => {
  assert.equal(defaultCapability.windows, undefined);
  assert.deepEqual(defaultCapability.webviews, [
    'main',
    'detached-*',
    'pet',
    'code-reader',
  ]);
});

test('浏览器展示层只承载原生表面，不订阅连续截图流', () => {
  assert.match(browserView, /browser_show_native_surface/);
  assert.doesNotMatch(browserView, /listenTauri\(['"]browser:frame/);
  assert.doesNotMatch(browserView, /browser_set_streaming/);
  assert.doesNotMatch(browserView, /data:image\/(?:jpeg|png);base64/);
  assert.doesNotMatch(browserManager, /Page\.(?:start|stop)Screencast/);
  assert.doesNotMatch(browserManager, /browser:frame/);
  assert.doesNotMatch(cdpClient, /Page\.screencastFrame/);
  assert.doesNotMatch(cdpClient, /Page\.screencastFrameAck/);
});

test('Windows MCP 同时开启 pageId 路由与结构化 targetId 输出', () => {
  assert.match(
    browserWrapper,
    /\['--experimental-page-id-routing', '--experimental-structured-content'\]/,
  );
});

test('CDP 存活探测异步执行且所有消费点等待真实结果', () => {
  assert.match(browserWrapper, /import \{ execFile, spawn \} from 'node:child_process'/);
  assert.doesNotMatch(browserWrapper, /execFileSync/);
  assert.match(browserWrapper, /async function probeCdp\(port, timeoutMs\)/);
  assert.match(browserWrapper, /await probeCdp\(portFile\.port, 1000\)/);
  assert.match(browserWrapper, /await probeCdp\(port, 2_000\)/);
  assert.match(browserWrapper, /void probeCdp\(port, 1000\)[\s\S]{0,100}\.then/);
});

test('原生表面不可用时显式报错并提供重试', () => {
  assert.match(browserView, /nativeAvailable === false/);
  assert.match(browserView, /browserNativeUnavailable/);
  assert.match(browserView, /browserRetry/);
  assert.match(browserView, /setSurfaceEpoch/);
});

test('初始化空文档呈现为产品化新标签页并暂停原生空白表面', () => {
  assert.match(browserView, /const showingNewTab = running && isInternalBlankPageUrl\(url\)/);
  assert.match(browserView, /const \[initialStatusResolved, setInitialStatusResolved\] = useState\(false\)/);
  assert.match(browserView, /const nativeSurfaceReady = shouldShowNativeBrowserSurface\(\{[\s\S]*statusResolved: initialStatusResolved/);
  assert.match(browserView, /const shouldSuspendNativeSurface = !nativeSurfaceReady/);
  assert.match(browserView, /setInitialStatusResolved\(true\)/);
  assert.match(browserView, /data-testid="browser-new-tab-page"/);
  assert.match(browserView, /browserStartBrowsing/);
  assert.match(browserView, /browserStartBrowsingHint/);
  assert.match(browserView, /setUrlInput\(browserAddressValue\(st\.url\)\)/);
  assert.match(browserView, /browserTabLabel\(tab, t\.browserEmptyTab\)/);
  assert.ok(
    browserView.indexOf('{/* 标签条在地址栏上方') < browserView.indexOf('{/* 工具条 */}'),
    '新标签页栏必须位于导航地址栏上方',
  );
});

test('普通模式用一个 Right Dock 切换器承载产物与浏览器入口', () => {
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

test('内嵌下载与剪贴板读取使用安全默认值', () => {
  assert.match(nativeHost, /\.on_download\(/);
  assert.match(nativeHost, /"browser:download-blocked"/);
  assert.doesNotMatch(nativeHost, /\.enable_clipboard_access\(\)/);
  assert.match(browserView, /listenTauri\('browser:download-blocked'/);
  assert.match(browserView, /if \(payload\.sessionId !== sessionId\) return;/);
  assert.match(browserView, /t\.browserDownloadBlocked\(payload\.source \|\| ''\)/);
});

test('原生表面暂停由应用级状态集中派生并覆盖所有遮挡路径', () => {
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
  assert.doesNotMatch(chatView, /useRightDockOcclusion\('artifact-fullscreen'/);
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

test('遮挡 UI、任务切换与 Right Dock 状态只在原生 hide ACK 后发布', () => {
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
  assert.match(browserView, /if \(nativeSurfaceCoordinator\.transitionOwner\) return Promise\.resolve\(false\)/);
  assert.match(browserView, /nativeSurfaceCoordinator\.transitionOwner = owner/);
  assert.match(browserView, /await claimNativeSurfaceHide\(owner, sessionId\)/);
  assert.match(browserView, /resumeNativeSurfaceOwner\(owner, sessionId\)/);
  assert.match(nativeSurfaceTransition, /revisions\.get\(ticket\.channel\) !== ticket\.revision/);
  assert.match(
    nativeSurfaceTransition,
    /getContext\(\) \|\| \{\}\)\.sessionId === context\.sessionId/,
  );
  assert.match(nativeSurfaceTransition, /predecessor\.catch\(\(\) => false\)/);
});

test('切换任务时丢弃旧状态响应并以新实例承载浏览器', () => {
  assert.match(browserView, /const sessionIdRef = useRef\(sessionId\)/);
  assert.match(browserView, /const statusRequestEpochRef = useRef\(0\)/);
  assert.match(browserView, /const tabsRequestEpochRef = useRef\(0\)/);
  assert.match(browserView, /sessionIdRef\.current === requestedSessionId/);
  assert.match(browserView, /st\?\.sessionId !== requestedSessionId/);
  const keyedBrowserViews = main.match(/<BrowserView\s+[\s\S]{0,100}?key=\{browserViewSessionId\}/g) || [];
  assert.equal(keyedBrowserViews.length, 2, 'compact and dock BrowserView instances must both be keyed by session');
});

test('用户接管状态可见，空闲后自动恢复且支持立即交还', () => {
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
    '页面侧全局去重会吞掉 Agent 事件后紧随的真实用户接管',
  );
});

test('浏览器事件缺少任务归属时 fail-closed，不创建全局兼容工作区', () => {
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

test('宿主请求由文件事件唤醒，空闲时不做高频目录轮询', () => {
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

test('应用自动化连接不会在宿主失败后自启外部 Chrome', () => {
  assert.match(browserManager, /let port = live_port\(\)/);
  assert.doesNotMatch(browserManager, /self\.acquire_or_start_chrome\(\)\.await/);
  assert.match(browserManager, /parse_host_owned_port_json/);
  assert.match(browserManager, /native_surface\.lock\(\)\.owns_port\(port\)/);
});

test('指定对话的浏览器操作查找失败时关闭失败，不落到全局 CDP', () => {
  const failures = browserManager.match(/指定对话(?:或标签页)?的原生浏览器工作区不存在/g) || [];
  assert.ok(failures.length >= 7, `expected fail-closed guards for scoped operations, got ${failures.length}`);
  assert.match(browserManager, /"restoreError": error/);
  assert.match(browserManager, /"missing": true/);
  assert.match(main, /\(!st\.running && !st\.restoreError\)/);
  assert.match(browserView, /setError\(st\.restoreError \|\| ''\)/);
});

test('prepare 后置失败只回滚本次新建工作区', () => {
  assert.match(browserManager, /fn rollback_new_native_workspace/);
  assert.match(browserManager, /surface\.close_session\(Some\(app\), session_id\)/);
  assert.doesNotMatch(
    browserManager,
    /if !probe_cdp\([\s\S]{0,300}native_surface\.lock\(\)\.close\(/,
  );
});

test('退出事件不等待浏览器锁且始终清理跨进程协调文件', () => {
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
  assert.match(exitHandler, /mgr\.shutdown_on_exit\(\)/);
  assert.doesNotMatch(
    exitHandler,
    /mgr\.stop\(\)\.await/,
    'exit fallback must not delete restore data after a busy preserving shutdown',
  );
});

test('应用重启只按 URL 清单重建新页面身份并恢复侧栏入口', () => {
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
  assert.match(nativeHost, /atomic_write_private\(path, &encoded\)/);
  const restoreWriter = nativeHost.slice(
    nativeHost.indexOf('fn write_restore_workspace_file'),
    nativeHost.indexOf('fn remove_restore_file'),
  );
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

test('Agent 新标签在 target 发现与宿主首航后才做 lease CAS 发布', () => {
  assert.match(nativeHost, /staged_tabs: HashMap/);
  assert.match(nativeHost, /webview[\s\S]{0,120}\.navigate\(requested_url\)/);
  assert.match(nativeHost, /commit_agent_mutation\([\s\S]{0,1000}workspace\.tabs\.insert/);
  assert.match(nativeHost, /staged_publication\.store\(true/);
  assert.match(nativeHost, /created_at_revision/);
  assert.match(nativeHost, /commit_agent_generation_rollback/);
  assert.doesNotMatch(browserManager, /create_tab_for_agent\([\s\S]{0,900}navigate_tab_after_bind/);
});

test('BrowserCore 标签按 marker、WebDriver bind、首航、发布顺序提交且不读取 CDP 端口', () => {
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
    'BrowserCore 新标签必须按创建 marker、绑定原生 target、提交发布的顺序执行',
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

test('Linux WebDriver 用宿主内部 marker 安全重绑，不向远程页注入身份', () => {
  assert.match(nativePlatform, /register_browser_core_webview_binding/);
  assert.match(
    nativeHost,
    /register_browser_core_webview_binding\(\s*&label,\s*tab_token,\s*&control,?\s*\)/,
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
  assert.match(linuxAutomation, /webview[\s\S]{0,80}\.navigate\(marker_url\)/);
  assert.match(linuxAutomation, /locate_binding_marker_locked/);
  assert.match(linuxAutomation, /request_locked\([\s\S]{0,100}Method::POST,[\s\S]{0,60}"url"/);
  assert.match(linuxAutomation, /active_binding_nonce: Option<String>/);
  assert.match(linuxAutomation, /binding_marker_seen: bool/);
  assert.match(
    linuxAutomation,
    /fn classify_binding_navigation[\s\S]{0,900}strip_prefix\(BINDING_MARKER_PREFIX\)/,
  );
  assert.match(
    linuxAutomation,
    /if binding\.binding_marker_seen \{[\s\S]{0,160}binding\.active_binding_nonce = None;[\s\S]{0,120}binding\.binding_marker_seen = false;/,
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
  assert.match(nativeHost, /browser_initialization_script\(cdp_tab_token\)/);
  assert.doesNotMatch(nativeHost, /BINDING_NONCE_PROPERTY/);
  assert.match(nativeHost, /browser_core_page_script_contains_no_task_or_tab_identity/);
});

test('Linux WebDriver 每个原生 mutation 在 POST 紧前复核 exact tab 和 active lease', () => {
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

test('BrowserCore 首航复用产品化空白页，最后一个工作区停止时关闭共享 driver', () => {
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

test('BrowserCore pageId 稳定且所有页面工具对缺失或畸形身份关闭失败', () => {
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

test('宿主协议 v3 回显身份字段，自动化失联事件按任务发送', () => {
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

test('不可逆页面操作后的持久化失败按任务可见并持续退避重试', () => {
  assert.match(browserManager, /persistence_io: parking_lot::Mutex<\(\)>/);
  assert.match(browserManager, /persistence_warnings: parking_lot::Mutex<HashMap<String, String>>/);
  assert.match(browserManager, /persistence_retries: parking_lot::Mutex<HashSet<String>>/);
  assert.match(browserManager, /let _persistence_guard = (?:self|manager)\.persistence_io\.lock\(\)/);
  assert.match(browserManager, /"browser:persistence-warning"/);
  assert.match(browserManager, /"browser:persistence-restored"/);
  assert.match(browserManager, /delay = delay\.saturating_mul\(2\)\.min\(Duration::from_secs\(30\)\)/);
  assert.match(browserManager, /status\["persistenceWarning"\] = json!\(warning\)/);
  assert.match(browserView, /listenTauri\('browser:persistence-warning'/);
  assert.match(browserView, /listenTauri\('browser:persistence-restored'/);
  assert.match(browserView, /data-testid="browser-persistence-warning"/);
});

test('popup 仅复用已 begin 的完整 lease，其他页面弹窗转为 User', () => {
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

test('恢复取消与孤儿清理不删除仍存在任务的数据', () => {
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

test('宿主瞬态请求不跨进程重放且取消补偿保留到 ACK', () => {
  const spawnWatch = browserManager.slice(
    browserManager.indexOf('pub fn spawn_watch'),
    browserManager.indexOf('// -----------------------------------------------------------------------\n    // 生命周期'),
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

test('一个取消补偿持续失败不会饿死其他对话的宿主请求', () => {
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
  // 所有 artifact 错误都在本轮收集；不能再由 `?` 提前退出整个扫描。
  assert.doesNotMatch(prepareRequests, /\.await\?/);
  assert.doesNotMatch(prepareRequests, /write_hosted_response\([^;]+\)\?/);
});

test('Host Core 取消持久化精确补偿并在 ACK 前撤销操作与 staging', () => {
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

test('原生可见性使用宿主 generation + sequence，旧 renderer 迟到请求无副作用', () => {
  assert.match(browserManager, /surface_visibility: parking_lot::Mutex<SurfaceVisibilityClock>/);
  assert.match(browserManager, /pub fn begin_surface_generation/);
  assert.match(browserManager, /visibility\.claim\(visibility_generation, visibility_sequence\)/);
  assert.match(browserCommands, /browser_begin_surface_generation/);
  assert.match(browserCommands, /visibility_generation: u64/);
  assert.match(browserCommands, /visibility_sequence: u64/);
});

test('User create/activate/close 的显示失败有明确提交边界', () => {
  assert.match(nativeHost, /回滚用户新建标签映射失败/);
  assert.match(nativeHost, /workspace\.tabs\.remove_token\(tab_token\)/);
  assert.match(nativeHost, /回滚用户标签激活映射失败/);
  assert.match(nativeHost, /workspace\.active_tab = previous_active/);
  assert.match(nativeHost, /用户关闭标签后显示回退页失败/);
  assert.match(nativeHost, /回滚新标签显示失败/);
});

test('原生物理表面全局单一，后台 Agent 激活不会抢占前台工作区', () => {
  assert.match(nativeHost, /set_exclusive_workspace_visibility\(&mut self\.workspaces, session_id\)/);
  assert.match(nativeHost, /workspace\.visible = workspace_session_id == session_id/);
  assert.match(
    nativeHost,
    /self\.active_session\.as_deref\(\) == Some\(session_id\)[\s\S]{0,900}workspace_may_present_native_surface\([\s\S]{0,120}session_owns_visible_surface,[\s\S]{0,120}workspace\.visible/,
  );
  assert.match(nativeHost, /session_owns_visible_surface && workspace_visible/);
});

test('远程页面无全局剪贴板授权，下载默认拒绝且事件不泄露本地路径', () => {
  assert.doesNotMatch(nativeHost, /enable_clipboard_access/);
  assert.match(nativeHost, /\.on_download\(/);
  assert.match(nativeHost, /"browser:download-blocked"/);
  assert.match(nativeHost, /"sessionId": download_session_id/);
  assert.match(nativeHost, /"tab": download_tab_token/);
  assert.match(nativeHost, /let source = match \(url\.scheme\(\), url\.host_str\(\)\)/);
  assert.doesNotMatch(nativeHost, /"destination"|"path": destination/);
});
