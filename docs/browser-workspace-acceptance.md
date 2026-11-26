# Browser workspace acceptance

Domain terms used by this contract are defined in [`docs/browser-workspace-context.md`](browser-workspace-context.md).

## Product boundary

- The browser is a right-side work surface for the current task conversation, not a global feature page in the left sidebar.
- Pages, tabs, and the active tab belong to a task conversation. The default browsing identity and website sign-in state belong to the application.
- The user and Agent operate the same system-native WebView page, and the user can take control directly.
- A continuous screenshot stream, JPEG/base64 frame transport, and external Chrome are not display or failure-fallback paths.
- A task may own multiple tabs. Ordinary observation and interaction apply only to that task's current visible tab.
- A background task may continue operating its own current tab without changing the task or Dock that the user is viewing.

## Tool-alignment invariants

Before any page-level tool executes, it must complete these steps in order:

1. Verify that `pageId` belongs to the current task.
2. Activate the corresponding native tab.
3. Select the same page in the automation backend.
4. Read the host workspace and page list again, then verify that the visible tab is the selected page.
5. Execute the real tool and verify its result.

If any step fails, the real tool must not execute. A foreground `new_page` may reuse the initial placeholder only when the workspace has exactly one tab, that tab's host token equals the task session token, and the page remains on the controlled `about:blank`. Reuse navigates that placeholder under the normal visible-page lease and must not leave an extra blank tab. Every other `new_page` first discovers a new target in a hidden staging WebView, lets the host commit the requested URL as the first navigation, and publishes it through one final compare-and-swap using the complete lease captured before creation. A bind, first-navigation, or compare-and-swap failure destroys only that request generation. `new_page(background: true)` never reuses the placeholder; it may only precreate and preload a tab, which must be activated before later observation or interaction.

A tool that has passed `begin_agent_operation` completes as one atomic dispatch. User takeover immediately invalidates the lease and blocks the next tool, but it does not promise to interrupt the current call after it has been submitted to the platform backend. The `finally` path immediately revokes active-operation state. Only a call that actually dispatched native input retains a post-dispatch callback grace period of at most 100 ms. Explicit UI takeover clears that window immediately; 750 ms is only a fail-safe upper bound for an abnormal dispatch exit.

Cancellation, timeout, and platform-backend exit share one atomic settlement path. After dispatch begins, cancellation is cooperative and the wrapper waits for the real response. If the backend does not answer within the post-timeout grace period, the wrapper must terminate it and confirm that its process has stopped. `end_agent_operation` may run only after the backend can no longer dispatch input. If terminal state cannot prove that the action did not occur, return a structured result with `actionMayHaveCommitted=true` and `retryable=false`; the Agent must not replay it. Acceptance coverage includes both a backend that ignores cancellation and later succeeds, and a backend that crashes during execution, with completion always preceding `end_agent_operation`.

A successful native mutation is committed. An optional `includeSnapshot` is a later observation step: observation failure must preserve success and set `structuredContent.observationWarning` with `actionCommitted=true` and `retryable=false`, instructing the Agent to call `take_snapshot` separately instead of replaying the mutation. `fill_form` validates the complete elements array before its first write. A mid-batch failure returns an `isError=true` structured partial result with `completedCount`, zero-based `failedIndex`, `totalCount`, and `retryable=false`; the host must not retry the complete form.

A direct user click, input, or scroll takes short-lived control. Every real interaction advances the revision and starts a new three-second idle window. Only a timer matching the latest interaction revision may return ownership to the Agent; an older timer must not overwrite a later interaction or an explicit hand-back. The immediate Hand back to Agent action remains a shortcut, not the only release path.

A `window.open` or `target=_blank` popup is published as Agent-owned through hidden staging, target binding, first navigation, and final compare-and-swap only when it carries the complete, still-valid host lease from an already-started dispatch. Without valid dispatch authorization it becomes user-owned. A short-lived boolean must never infer Agent ownership, and a user takeover that commits first must reject the late popup.

## Delivered by this PR

| Capability or runtime | Windows | macOS | Linux |
|---|---|---|---|
| `browser_native_display` | `true` (WebView2) | `true` (WKWebView) | `true` (WebKitGTK) |
| `browser_agent_automation` | `true` | `true` (BrowserCore) | `true` (BrowserCore + WebKitWebDriver) |
| `browser_cdp` | `true` only for the application-owned loopback endpoint | `false` | `false` |
| Agent browser tools | Core tools plus Chrome diagnostic extensions; screenshots are hidden from the Agent | BrowserCore DOM structure and interactions; screenshots are hidden, there is no standalone scroll tool, and `hover`, `drag`, `resize_page`, and `handle_dialog` are not exposed | Supported BrowserCore DOM and interaction tools, including W3C `handle_dialog`; screenshots are hidden, there is no standalone scroll tool, and `hover`, `drag`, and `resize_page` are not exposed |
| `chrome-devtools-mcp` build/package | Windows adapter | None | None |
| Page file download | OS save dialog over the WebView2 download | OS save dialog over `WKDownload` | OS save dialog over the WebKitGTK download |
| External Chrome or screenshot-stream fallback | Forbidden | Forbidden | Forbidden |

Linux provides a reachable native-page and Agent-automation loop: the system WebKitGTK displays the real page, WebKitWebDriver element endpoints generate trusted clicks and keyboard input, and W3C alert endpoints handle `alert`, `confirm`, and `prompt` inside the same operation gate without packaging or starting Chrome MCP. The `.deb` package requires `webkit2gtk-driver`. A development environment without the driver must not register a fake tool catalog that fails only after invocation. Because WebKitGTK pointer actions block on the currently tested hardware, `hover` and `drag` remain hidden and direct calls fail quickly until a trusted implementation passes acceptance. `resize_page` must neither call W3C Set Window Rect on the Tauri top-level window nor bypass Dock layout by temporarily changing child-WebView bounds; it remains hidden and fails quickly until an independently recoverable viewport simulation exists.

Neither Linux nor macOS currently exposes screenshots to the Agent, and neither provides a standalone scroll tool. Their current verification contract covers DOM or accessibility structure and supported interactions only. It does not let the model inspect colors, spacing, animation quality, sharpness, or other visual styling. The visible native page is for the user; a successful DOM snapshot is not visual confirmation. A platform without a released browser, and any lane where the browser gate is closed, must not treat a display-only surface, external Chrome, screenshot stream, or synthetic JavaScript interaction as completion.

BrowserCore element resolution runs its uid registry per frame but resolves interaction coordinates from the main frame only, so elements inside any iframe — including same-origin ones — fail with `browser/stale-ref` on Linux and macOS. This is fail-closed by design; the Windows CDP backend can act inside frames. Acceptance must not expect cross-frame interaction parity until the BrowserCore backends gain per-frame resolution.

After a Linux WebDriver session crash, the host may briefly navigate the page to a random internal marker while safely rebuilding the handle mapping, then reload the original URL. Acceptance must not expect form values, Canvas contents, or JavaScript memory to survive. It must verify that the remote page's main world never receives a stable binding identity and that the recovered page still belongs to the original task and tab.

## Page downloads

- A page download streams into private staging (`~/.pinvou3/browser/downloads/<id>/`) and is placed only where the user chooses in the OS save dialog. The system WebView writes the transfer itself: the app applies no size limit and streams the content through a small copy buffer at most. At most 16 downloads are live at once; a page that asks for more is refused with the `browser:download-blocked` notice (`reason: limit`) until existing ones finish, as is a download whose staging directory cannot be created (`reason: staging`). When the Linux download surface itself cannot be created, downloads are put out of service for the rest of the process and refused with `reason: unavailable`: attaching per-tab handlers instead would mis-attribute every download and, once the first tab closed, cancel all of them without any notice, so the failure is fail-closed.
- The dialog is non-blocking and is never opened from the WebView download callback, which wry invokes on the native UI thread while holding its handler borrow. Only one dialog is open at a time; every other transfer streams meanwhile and is prompted in turn.
- wry registers one context-level download handler per webview build with no dedupe or disconnect, and tauri resolves each handler's `webview` argument from its build-time label rather than from the requesting download. On Linux all tabs share one WebKit context that the runtime keeps alive for the whole process, so the first-connected handler would answer every download forever and, once its tab closed, cancel every download before any app code runs. Linux therefore routes all downloads through one always-live hidden download surface that owns the single handler and is never closed. Its notices are attributed to the workspace that is currently shown; the requesting tab is not identifiable through the context-level signal, so the notice's `tab` value is best-effort there — the mirror follows the shown session's active tab and is refreshed at every tab activation and close — and a download that arrives while no workspace is shown is refused. macOS and Windows fire each event in the downloading webview's own handler, so the requesting tab is attributed exactly.
- A completion without a written path (macOS always, Windows on failure) can only be matched by URL. Such reports are attributed through one per-URL batch that counts the URL's in-flight transfers down; a transfer resolved through its written path votes in the batch too, so a mixed generation resolves as soon as its last member reports and leaves nothing behind. Uniform outcomes attribute cleanly; disagreeing ones all fail closed as `failed` instead of placing an unattributable file as `saved`. A download the app refuses (live cap, unusable staging, unavailable service, or no shown workspace) never enters a batch: the engine still cancels it and reports a pathless failure, and the coordinator spends that report as noise so it cannot fail a live transfer of the same URL.
- The dialog opens in the folder of the last file saved in this app session (the folder is remembered per process, not per browser session), else the system Downloads folder, else the home folder. Without a starting folder GTK opens the process working directory.
- The file is placed once both the user's choice and the finished transfer exist, and the staged bytes are flushed to disk before the destination name can appear. A destination that would land inside the staging root — spelled directly or resolved into it through a symlink or a case-insensitive spelling — fails the placement instead (`unsaved`), so no saved file can be swept away later. A placement that fails keeps the staged file and reports `unsaved`, so the user can choose another location without downloading again. `unsaved` is the only state without an automatic exit, so its notice carries both a Save as… and a Discard download action: discarding deletes the staged file and reports `cancelled`. Declining, a failed transfer, and a successful placement discard the staging directory — declining a reopened save-as dialog keeps the staged file for another attempt — and startup deletes the per-download staging directories an earlier process left behind (entries the coordinator does not own are left untouched). Declining does not abort a transfer the engine has already started: the download callback exposes no cancel, so the transfer runs to its end in staging and is deleted then.
- A cross-volume save streams through a temporary sibling named `.pinvou-<download id>.part` in the destination folder; a process killed mid-copy leaves exactly that one attributable file behind, and no sweep covers it.
- `browser:download-started` carries the task, tab, an opaque download id, and the file name; `browser:download-finished` additionally carries an outcome (`saved`, `cancelled`, `failed`, or `unsaved`). They are not forwarded to the remote Web UI.
- Known limitation on Linux: WebKitGTK 2.52 reports a response that ends before its `Content-Length` (connection closed, reset, or a truncated chunked body) as a finished download, so a truncated transfer can be placed and reported as `saved`. The download callback exposes no expected size, so the app cannot detect this. An HTTP error status is reported as a failure; both are engine behavior the app trusts, not something it enforces.
- Known engine-lifetime note: on macOS and Windows the download delegate dies with its webview, so a tab closed mid-download never delivers its completion. The app settles such records when the tab closes (reported as `failed`, staging discarded): the live slot is freed, and the record's seat in its URL's completion batch is released without a vote, so the batch resolves on its real reports alone. Linux completions arrive through the never-closed download surface and survive tab close.
- Acceptance has exercised this flow end to end on Linux only (before the download-surface redesign; the redesign must be re-exercised there). Windows (WebView2 reports the written path; a failure reports none) and macOS (`WKDownload` reports none, so a completion is attributed only once every same-URL transfer has reported, failing closed when the reports disagree) must be repeated there before release.

## Future complete three-platform automation target (not delivered by this PR)

| Capability | Windows | macOS | Linux |
|---|---|---|---|
| Native page | WebView2 | WKWebView | WebKitGTK |
| Task isolation and multiple tabs | Required | Required | Required |
| Navigation, DOM/accessibility structure, click, input, keys, scroll, wait, script, model-visible screenshot, and dialog | Required | Required | Required |
| Trusted native Agent input | WebView2/CDP | Native event adapter | WebKitWebDriver without taking over the global mouse or keyboard |
| Chrome DevTools advanced diagnostics | Capability-gated | Not promised | Not promised |
| External Chrome dependency | None | None | None |

This table is a future product target, not a statement of what the current PR exposes. A platform that is display-only or supports only `isTrusted=false` synthetic JavaScript interaction must not claim that core automation is complete and must not enable `browser_agent_automation`. A missing platform tool must be hidden from the catalog and reject direct calls quickly rather than timing out as if it were supported.

## Performance gates

- Continuous screenshot frames in native mode: `0`.
- JPEG/base64 bytes on the display path: `0`.
- Continuous bounds IPC while hidden or idle: `0`.
- Bounds updates: at most once per display frame; identical session, tab, and bounds values must not call the native layer.
- Application-added latency for a warm Dock expansion, restore, or tab switch: P95 `< 100 ms`.
- Application-added latency to align the Agent target page and begin dispatch: P95 `< 200 ms`.
- Visible feedback for native user input: no more than one display frame.
- After minimize, DPI change, or shrink-and-restore: no more than `1` physical pixel of bounds error.
- Before a blocking dialog, full-screen preview, non-browser Dock panel, or hidden main window appears, the native page must hide. Restoration must not flicker, black-screen, or paint through the overlay.
- Closing a task must leave no orphan WebView, CDP endpoint, task MCP configuration, or restoration manifest. A normal application restart preserves only the minimal URL/order/active-index manifest with user-private permissions; old tab tokens, target IDs, leases, and runtime mappings must be deleted.
- An unreadable durable Prepare journal with a canonical lowercase session-token filename must be isolated together with that exact token's process-local runtime mapping and restore manifest, moving runtime then restore and the journal last into a private, per-token generation slot. Directory roots and children must remain pinned for enumeration, move, and cleanup; symlinks, junctions, reparse points, and unstable regular-file identities must fail closed. A partial move must keep the active journal visible, block browser admission globally, and retry the same slot with bounded backoff. Once the journal marker moves successfully, no quarantined state may be restored or treated as active, unrelated tasks and a fresh generation for the same task must remain usable, and repeated corruption must use a new slot. A regular `.json` journal with an untrusted filename must be isolated in an unassigned journal-only slot without mutating any task state. Task cleanup and startup orphan reconciliation must remove eligible completed slots only through anchored handles and must not derive authority from unexpected names. A committed WAL whose exact token-scoped host runtime has a higher revision must be durably changed to non-compensating before that revision witness is removed; after any crash point, the newer restore manifest must survive and a late old cancellation must be an acknowledged no-op.

### Performance collection and evaluation

- Windows hardware acceptance collects at least `30` warm samples per metric. Fewer samples may be displayed but must not be marked as passing.
- In frontend DevTools, `window.__PINVOU_BROWSER_PERF__.snapshot()` returns counts, P50, P95, and maximum values for Dock native-surface commit, restoration status query, and tab switching. Call `reset()` to start an independent sample run.
- Set `PINVOU3_BROWSER_PERF_LOG=1` before launching the application. The wrapper writes Agent target-alignment samples as `[browser-perf]` JSONL on stderr. Run `node pinvou3-app/scripts/browser-performance-report.mjs <log-file> --min-samples=30` to aggregate the samples, apply these thresholds, and return an appropriate exit code.
- Node and static contract tests verify sampling, deduplication, and threshold calculations only. They do not replace Windows WebView2 hardware measurements for P95 latency, input-frame latency, or DPI behavior.

A fixed memory budget must be based on measurements from real hardware on all three platforms. Until then, record the incremental working set for each task and tab, idle reclamation, and abnormal peaks; do not turn an estimated number of megabytes into a pass condition.

## Responsive layout and restoration

- When the desktop is wide enough, the conversation and the single right Dock appear side by side at the user's saved ratio.
- When space is insufficient, switch to a conversation-or-Dock single-pane mode instead of compressing the conversation into a narrow strip or saving the temporary width as a preference.
- Expanding the window restores the pre-shrink ratio. Cancelled drag, window blur, minimize, and DPI changes must not leave a temporary pixel width behind.
- After an application restart, restore tab URLs, order, and the active tab for each task while retaining the default browser identity. Ownership starts neutral and unclaimed; the next real user action or Agent lease claims it atomically. Restart must not masquerade as user takeover or silent authorization. A persistence failure is visible per task and retried with continuing backoff. In-page form state, Canvas contents, and JavaScript memory are not restored.

## Security gates

- Page navigation, new windows, and redirects allow only `http`, `https`, and controlled `about:blank`. Reject `file`, `data`, `javascript`, `chrome`, and other schemes.
- The packaged macOS app enables ATS exceptions only for user-directed WKWebView content and local networking (`NSAllowsArbitraryLoadsInWebContent` and `NSAllowsLocalNetworking`). It must not set the process-wide `NSAllowsArbitraryLoads` exception.
- CDP binds only to loopback, and coordination files use user-private permissions. Platform protections such as SmartScreen must remain enabled.
- Remote pages do not receive global clipboard-read access. An embedded download never leaves private staging until the user chooses a place in the OS save dialog (see [Page downloads](#page-downloads)). No URL and no local download path is written to restore state or exposed through events; a download that cannot start reports only its origin (scheme, host, and explicit port). This release has no file-selection approval broker, so `mcp_browser_upload_file` is absent and direct calls fail before starting upstream code or touching a file; no other tool may bypass that boundary. Camera, microphone, location, and notification requests keep the engine's default deny-or-prompt behavior until an explicit request flow exists and must never be granted silently.
- The reserved `pinvou-user-takeover://interaction/...` navigation signal is unauthenticated and low privilege. A remote page may trigger it to pause Agent control, creating only a temporary Agent-control denial of service until idle or explicit hand-back. It carries no page data and cannot grant browser operations, filesystem or clipboard access, credentials, or any other capability.
- Business-semantic risks such as payment, deletion, and final submission use the general Agent approval framework, not the browser protocol allowlist.
