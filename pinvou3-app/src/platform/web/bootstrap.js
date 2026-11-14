/** Browser-side Tauri compatibility layer for the shared Full WebUI. */
(function () {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";

  const WEB_CAPABILITIES = {
    desktopChrome: false, detachWindows: false, pet: false, oauth: false,
    externalAuth: false, superPermission: false, appUpdate: false,
    dependencyInstall: false, localModelSetup: false, externalSystemOpen: false,
    webAccessAdmin: false, desktopNotifications: false, hostFilePicker: true, artifactDownload: true,
    sessionModelSwitch: true,
    modelManagement: false,
    toolStoreMutations: false,
    // Zap-send needs the desktop EnginePool command channel; hide the button on web.
    interruptSend: false,
    deviceFileUpload: true,
    acpCodeMode: true,
    // Mouse/keyboard control is desktop-only by design; the web bridge only
    // exposes rejecting stubs (review finding: state the polarity explicitly).
    computerUse: false,
  };
  const SEMANTIC_COMMAND_REQUIREMENTS = {
    hostFilePicker: ["web_access_list_host_files", "web_access_ingest_file"],
    artifactDownload: ["web_access_artifact_info", "web_access_read_artifact_chunk"],
    sessionModelSwitch: ["set_session_model"],
    deviceFileUpload: [
      "web_access_upload_attachment_chunk",
      "web_access_abort_attachment_upload",
      "web_access_discard_attachment",
    ],
    acpCodeMode: {
      commands: [
        "web_access_list_acp_agents",
        "web_access_get_acp_agent_status",
        "web_access_list_codex_acp_sessions",
        "web_access_create_codex_acp_session",
        "web_access_get_codex_acp_session_info",
        "web_access_set_codex_acp_model",
        "web_access_set_codex_acp_mode",
        "web_access_set_codex_acp_config_option",
        "web_access_codex_acp_prompt",
        "web_access_cancel_codex_acp",
        "web_access_get_codex_acp_timeline",
        "web_access_get_codex_acp_pending_permissions",
        "web_access_respond_codex_acp_permission",
        "web_access_get_codex_acp_pending_elicitations",
        "web_access_respond_codex_acp_elicitation",
        "web_access_list_codex_workspace",
        "web_access_search_codex_workspace",
        "web_access_preview_codex_workspace_file",
        "web_access_get_codex_workspace_changes",
        "web_access_get_codex_workspace_diff",
        "web_access_list_host_files",
        "web_access_ingest_file",
        "web_access_discard_attachment",
      ],
      events: ["acp:event"],
    },
  };

  if (window.__TAURI__) {
    window.PinvouPlatform = Object.freeze({
      kind: "desktop",
      isWeb: false,
    });
    return;
  }

  // Error messages from rejected invoke calls reach the UI through the bridge
  // addSystemItem path. The single source for this copy is
  // uiPlatformMisc.webClientErrors in shared/i18n.js; the React entry attaches
  // it to window.PinvouWebClientStrings in the active language. These Chinese
  // strings remain only as a fallback (a plain script cannot import ES
  // modules). Connection-state messages never reach the UI (WebConnectionStatus
  // uses the uiWebConnection dictionary exclusively), so they need no fallback.
  const FALLBACK_ERRORS = {
    stateNotReady: "远程控制状态尚未就绪，无法处理桌面端事件",
    unnegotiatedEvent: function (event) { return `桌面端发送了未协商的远程控制事件：${event}`; },
    rpcFailed: "远程调用失败",
    incompatibleDesktop: "桌面端版本不支持当前远程控制功能，请先更新桌面端",
    unsupportedCommand: function (command) { return `当前桌面端尚不支持远程控制功能：${command}`; },
    commandNotAllowed: function (command) { return `远程控制不允许调用 ${command}`; },
    invalidRequestId: "远程调用请求 ID 无效",
    requestInFlight: "远程调用请求正在进行中",
    invokeTimeout: function (command) { return `远程调用超时：${command}`; },
  };

  function errorText(key, arg) {
    const custom = (window.PinvouWebClientStrings || {})[key];
    const entry = custom === undefined ? FALLBACK_ERRORS[key] : custom;
    return typeof entry === "function" ? entry(arg) : entry;
  }

  const scriptUrl = document.currentScript && document.currentScript.src
    ? new URL(document.currentScript.src, window.location.href)
    : new URL("./platform/web/bootstrap.js", window.location.href);
  const policyUrl = new URL("access-policy.json", scriptUrl);
  const fragment = new URLSearchParams(String(window.location.hash || "").replace(/^#/, ""));
  const endpointId = fragment.get("endpoint") || "";
  const accessToken = fragment.get("token") || "";
  const relayOverride = fragment.get("relay") || "";
  const protocolVersion = 2;
  // Liveness watchdog. Protocol-level Relay pings are invisible to JS, so a
  // healthy-but-idle socket is silent by design: staleness is only suspicion
  // of a half-open connection, never proof. Suspicion is resolved with a
  // speculative reconnect — the durable cursor makes replay idempotent and
  // RPCs replay by design, so an extra reconnect is cheap. While the rejoin
  // verifies, the UI stays on "connected"; only a failed verification (a
  // connect error, or a close before the join ack) escalates to the ordinary
  // visible reconnecting state.
  const LIVENESS_WATCHDOG_INTERVAL_MS = 30_000;
  const LIVENESS_SILENCE_TIMEOUT_MS = 120_000;
  // A speculative cycle that never became usable is NOT a healthy idle
  // cycle: either its socket never delivered the join ack, or the ack
  // arrived but the desktop-bound frames it promised never did (the
  // capability snapshot stays due — a relay can ack joins on one code path
  // while blackholing the forwarding path, and the ack is generated by the
  // relay itself, so it cannot vouch for the desktop link). The reconnect
  // after the first failed cycle is still speculative (the failure count
  // has only reached this inclusive cap); visibility begins when a further
  // failed cycle pushes the count past the cap, so recovery becomes
  // visible: each watchdog close matches its own speculativeSource, so the
  // close-side "verification failed" branch alone can never fire, and such
  // a relay would otherwise keep the UI on "connected" forever (round-5
  // review; round-11 closed the acked-but-frameless half by counting it and
  // moving the verified-good reset from the join ack to the capability
  // handshake completion — resetting on the ack alone let the relay re-zero
  // the ladder every cycle).
  const LIVENESS_MAX_SPECULATIVE_FAILURES = 1;
  // A connect wedged past this window (browser TCP timeouts can run minutes,
  // and background tabs never fire the error event for a silent drop) gets
  // one watchdog kick so recovery reroutes through the close handler.
  const LIVENESS_CONNECT_WEDGE_TIMEOUT_MS = 45_000;
  // Desktop-gone failures (endpoint_not_found, desktop_offline_ttl_expired)
  // keep the historical fast retry pace for the first attempts so a desktop
  // recreating its endpoint is rejoined within seconds, then double per
  // attempt up to the cap so an absent desktop is not retried ~10s forever.
  const DESKTOP_GONE_FAST_RETRIES = 5;
  const DESKTOP_GONE_MAX_DELAY_MS = 60_000;

  function relayWebSocketUrl() {
    if (relayOverride) return relayOverride;
    // Resolve beside this bootstrap script, not beside the current SPA route.
    // This keeps extensionless deep links on the same `/pinvou3/remote/ws`
    // endpoint instead of accidentally appending `ws` to the route.
    const url = new URL("../../ws", scriptUrl);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    url.search = "";
    url.hash = "";
    return url.toString();
  }

  function randomId(prefix) {
    if (window.crypto && typeof window.crypto.randomUUID === "function") {
      return `${prefix}_${window.crypto.randomUUID()}`; // safari14-ok: guarded above
    }
    const bytes = new Uint8Array(18);
    if (window.crypto && typeof window.crypto.getRandomValues === "function") {
      window.crypto.getRandomValues(bytes);
    } else {
      // eslint-disable-next-line sonarjs/pseudo-random -- not security-sensitive: request ID fallback when neither randomUUID nor getRandomValues is available
      for (let i = 0; i < bytes.length; i += 1) bytes[i] = Math.floor(Math.random() * 256);
    }
    return `${prefix}_${Array.from(bytes, (value) => value.toString(16).padStart(2, "0")).join("")}`;
  }

  class WebTauriClient {
    constructor() {
      this.endpointId = endpointId;
      this.accessToken = accessToken;
      this.url = relayWebSocketUrl();
      this.socket = null;
      this.joined = false;
      this.desktopOnline = false;
      this.leaseId = null;
      this.closedPermanently = false;
      this.reconnectAttempt = 0;
      this.reconnectTimer = null;
      // Speculative-reconnect state: set only by the liveness watchdog, whose
      // staleness close is a suspicion to verify, not a proven failure.
      // speculativeFailures counts finished cycles whose socket never got its
      // join ack; past the cap the watchdog reconnects visibly instead.
      this.speculativeReconnect = false;
      this.speculativeSource = null;
      this.speculativeFailures = 0;
      // Consecutive desktop-gone reconnects (endpoint_not_found / TTL expiry).
      this.desktopGoneRetries = 0;
      this.reconnectForDesktopGone = false;
      this.lastInboundAt = 0;
      // Set on every connect(): wedge-window start for the liveness watchdog.
      this.connectStartedAt = 0;
      // First CLOSING observation by the watchdog (same wedge rescue);
      // cleared on every connect(), speculative close, and CLOSING/CLOSED
      // rescue.
      this.closingStartedAt = null;
      this.livenessTimer = null;
      this.pending = new Map();
      this.listeners = new Map();
      this.subscribed = new Set();
      this.frontendReadyRequested = false;
      this.frontendReady = false;
      this.stateReady = false;
      this.desktopCapabilitiesReady = false;
      this.awaitingCapabilitySnapshot = false;
      // Capability compatibility and transport availability are deliberately
      // separate. A transient disconnect must pause RPC without unmounting
      // feature UI (and losing local drafts) after this endpoint has already
      // completed a successful negotiation.
      this.negotiatedCapabilitiesKnown = false;
      this.negotiatedCommands = new Set();
      this.negotiatedEvents = new Set();
      this.webAllowedCommands = new Set();
      this.webAllowedEvents = new Set();
      // allowedCommands/allowedEvents are only assigned after policy/capability negotiation; declared null first
      // (same falsy as the previously dynamic undefined; the consumer existence checks are unchanged).
      this.allowedCommands = null;
      this.allowedEvents = null;
      this.pendingListenerRegistrations = 0;
      this.connectionListeners = new Set();
      this.eventDispatch = Promise.resolve();
      this.connectionState = {
        status: "idle",
        message: "",
        endpoint_id: this.endpointId,
        desktop_online: false,
      };
      this.policyPromise = fetch(policyUrl, { cache: "no-store" })
        .then((response) => {
          if (!response.ok) throw new Error(`Web access policy unavailable (${response.status})`);
          return response.json();
        })
        .then((policy) => {
          this.webAllowedCommands = new Set(policy.allowed_commands || []);
          this.webAllowedEvents = new Set(policy.allowed_events || []);
          this.allowedCommands = new Set(this.webAllowedCommands);
          this.allowedEvents = new Set(this.webAllowedEvents);
          return policy;
        });
      this.cursorKey = endpointId ? `pinvou.web.cursor.${endpointId}` : "";
      let cursor = {};
      if (this.cursorKey) {
        try { cursor = JSON.parse(sessionStorage.getItem(this.cursorKey) || "{}"); } catch { /* treat a corrupted cursor as empty */ }
      }
      this.streamEpoch = typeof cursor.stream_epoch === "string" ? cursor.stream_epoch : "";
      this.lastSeq = Number.isFinite(Number(cursor.after_seq)) ? Number(cursor.after_seq) : 0;
      this.startLivenessWatchdog();
      this.bindNetworkListeners();
      if (endpointId && accessToken) this.connect();
      else this.parkCredentialsMissing();
    }

    // Blocking terminal state for missing credentials: no reconnect path
    // (online/visibilitychange/watchdog) may turn this into a join retry loop
    // that is doomed to fail (the spinner would cover the blocking card).
    // Every path parks back on this card.
    parkCredentialsMissing() {
      queueMicrotask(() => this.setConnection("credentials_missing", "远程控制链接缺少访问凭证。"));
    }

    setConnection(status, message) {
      const detail = {
        status,
        message: message || "",
        endpoint_id: this.endpointId,
        desktop_online: this.desktopOnline,
      };
      this.connectionState = detail;
      window.dispatchEvent(new CustomEvent("pinvou:web-connection", { detail }));
      this.connectionListeners.forEach((listener) => {
        try { listener(detail); } catch { /* a failing listener must not block the broadcast */ }
      });
    }

    connect() {
      if (this.closedPermanently || this.socket) return;
      if (!this.endpointId || !this.accessToken) {
        this.parkCredentialsMissing();
        return;
      }
      // A speculative reconnect keeps the UI on "connected" until verification
      // fails; see the LIVENESS_SILENCE_TIMEOUT_MS note above.
      if (!this.speculativeReconnect) this.setConnection("connecting", "正在连接桌面端…");
      let socket;
      try { socket = new WebSocket(this.url); }
      catch (error) {
        // No socket exists, so the close path can never clear a speculative
        // state; count the failed cycle so the cap escalates (round-5 review).
        this.noteSpeculativeFailure();
        this.scheduleReconnect(String(error));
        return;
      }
      this.socket = socket;
      // Every new socket gets a full silence window to deliver its join ack
      // before the liveness watchdog may judge it, and a wedge window before
      // a CONNECTING socket may be abandoned.
      this.lastInboundAt = Date.now();
      this.connectStartedAt = Date.now();
      this.closingStartedAt = null;
      socket.addEventListener("open", () => {
        // Round-23 review hardening: open is the only one of the four socket
        // events without an identity gate — close/error/message all verify
        // event ownership before touching cycle state. In a spec-compliant
        // browser a CONNECTING-phase close() means open is never delivered,
        // so this gate is unreachable in the current topology; adding it keeps
        // the gate symmetric with the other three, and a stale late open can
        // no longer send the join frame through sendRaw onto a successor
        // socket (the transport fixture pins that direction).
        if (this.socket !== socket) return;
        this.sendRaw({
          v: protocolVersion,
          type: "web_client_join",
          endpoint_id: this.endpointId,
          access_token: this.accessToken,
          protocol_version: protocolVersion,
          stream_epoch: this.streamEpoch || null,
          after_seq: this.lastSeq,
        });
      });
      socket.addEventListener("message", (event) => this.handleMessage(event.data, socket));
      socket.addEventListener("close", () => {
        // Round-16 review MAJOR-1: a late close may only dispose of its own
        // socket. The CLOSING/CLOSED rescue detaches the socket reference
        // first (this.socket = null), so its close event can arrive after the
        // successor cycle has already joined; if this handler still cleared
        // flags or scheduled a reconnect unconditionally, the healthy successor
        // would be knocked back to the unnegotiated state — the next inbound
        // event then hits !desktopCapabilitiesReady and processRemoteEvent
        // permanently shuts the door as incompatible_desktop (probe
        // end-to-end reproduction). An identity-mismatched close is pure
        // noise: the flags belong to the current cycle and have nothing to do
        // with this old socket. A socket detached deliberately after event
        // processing failed also falls inside this gate, but that path
        // schedules its own reconnect (handleRemoteEvent's catch) and does not
        // rely on this handler's unconditional branch.
        if (this.socket !== socket) return;
        this.socket = null;
        this.joined = false;
        this.desktopOnline = false;
        this.desktopCapabilitiesReady = false;
        this.leaseId = null;
        if (this.speculativeReconnect && socket !== this.speculativeSource) {
          // A socket issued by the speculative reconnect died before it could
          // rejoin: verification failed, so stop hiding the recovery.
          this.noteSpeculativeFailure();
        }
        if (!this.closedPermanently) this.scheduleReconnect("连接已断开，正在重试…");
      });
      socket.addEventListener("error", () => {
        // Round-16 review: same identity gate as the close handler — a late
        // error from a rescue-detached socket is likewise noise and must not
        // smear the successor cycle's flags or speculative count.
        if (this.socket !== socket) return;
        // The watchdog/network-recovery speculative close on a half-open
        // socket surfaces as error first (some browsers report a failed
        // closing handshake as error+close): unlike a natural death,
        // verification has not started yet, so this must not count as a
        // speculative failure nor light up connecting early — otherwise the
        // "UI stays connected until verification fails" semantics fork across
        // browser engines. The speculative source socket's error is left to
        // the close handler for unified accounting (that identity gate does
        // not count either); non-speculative deaths keep the original
        // semantics.
        if (this.speculativeReconnect && socket === this.speculativeSource) return;
        this.noteSpeculativeFailure();
        // An abnormal disconnect fires error before close: the dedicated
        // desktop_offline copy must not be overwritten by the connecting state
        // here either (the close→scheduleReconnect side has the same guard),
        // or a TCP reset inside the offline window would swap "desktop
        // offline" for a misleading spinner.
        if (!this.closedPermanently && this.connectionState.status !== "desktop_offline") {
          this.setConnection("connecting", "连接异常，正在重试…");
        }
      });
    }

    scheduleReconnect(message) {
      if (this.closedPermanently || this.reconnectTimer) return;
      // The dedicated desktop_offline copy ("desktop offline, keep this page
      // open") would otherwise be overwritten by the connecting state from the
      // immediately following close→scheduleReconnect, leaving the user with
      // nothing but "connecting" for the entire up-to-60s backoff window.
      // Desktop-offline failures keep that status through the whole backoff
      // window; only a reconnect attempt (connect) or the join ack advances it.
      if (!this.speculativeReconnect && this.connectionState.status !== "desktop_offline") {
        this.setConnection("connecting", message);
      }
      // Desktop-gone failures keep the fast pace only for the first attempts;
      // any other failure class resets the streak (a successful join resets it
      // in the join handler).
      this.desktopGoneRetries = this.reconnectForDesktopGone ? this.desktopGoneRetries + 1 : 0;
      this.reconnectForDesktopGone = false;
      let base = Math.min(10_000, 500 * (2 ** Math.min(this.reconnectAttempt, 5)));
      if (this.desktopGoneRetries > DESKTOP_GONE_FAST_RETRIES) {
        base = Math.min(
          DESKTOP_GONE_MAX_DELAY_MS,
          10_000 * 2 ** (this.desktopGoneRetries - DESKTOP_GONE_FAST_RETRIES),
        );
      }
      // eslint-disable-next-line sonarjs/pseudo-random -- not security-sensitive: reconnect backoff jitter (±20%); only affects retry pacing
      const delay = Math.round(base * (0.8 + Math.random() * 0.4));
      this.reconnectAttempt += 1;
      this.reconnectTimer = window.setTimeout(() => {
        this.reconnectTimer = null;
        this.connect();
      }, delay);
    }

    clearSpeculativeReconnect() {
      this.speculativeReconnect = false;
      this.speculativeSource = null;
    }

    // A speculative cycle failed verification (its socket died — or was
    // constructed, in the connect()-throw case — before the join ack). Count
    // it and stop hiding recovery; the watchdog re-enables speculation only
    // while the failure count is under the cap. Idempotent per cycle: the
    // flag is cleared on the first call, so an error event followed by the
    // close event counts once.
    noteSpeculativeFailure() {
      if (!this.speculativeReconnect) return;
      this.speculativeFailures += 1;
      this.clearSpeculativeReconnect();
    }

    startLivenessWatchdog() {
      if (this.livenessTimer) return;
      if (typeof window.setInterval !== "function" || typeof window.clearInterval !== "function") return;
      this.livenessTimer = window.setInterval(() => this._livenessCheck(), LIVENESS_WATCHDOG_INTERVAL_MS);
      // Node-based embedders return a Timeout object; never hold their process open.
      if (typeof this.livenessTimer.unref === "function") this.livenessTimer.unref();
    }

    // Watchdog body (also the test hook). An OPEN socket that has received no
    // inbound frame for LIVENESS_SILENCE_TIMEOUT_MS is probably half-open
    // (phone sleep, network switch): RPCs would hang until their timeout and
    // the close event would never fire. Closing routes recovery through the
    // ordinary close -> backoff -> rejoin path, marked speculative so the UI
    // stays on "connected" unless the rejoin fails to verify. See the
    // LIVENESS_SILENCE_TIMEOUT_MS note above.
    _livenessCheck() {
      if (this.closedPermanently || !this.socket) return;
      if (this.socket.readyState === WebSocket.CONNECTING) {
        // The watchdog is the only timer that notices a wedged connect (the
        // browser may never deliver an error event); give it the wedge window
        // first. The UI is already in the visible connecting state, so this
        // abort stays non-speculative.
        if (Date.now() - this.connectStartedAt <= LIVENESS_CONNECT_WEDGE_TIMEOUT_MS) return;
        try { this.socket.close(); } catch { /* already closing */ }
        return;
      }
      // Round-15 review: CLOSING/CLOSED used to be ignored forever — a
      // speculative close() on a silently dropped link (NAT timeout, no online
      // event) may never receive a close event, and engine differences in
      // closing-handshake timeouts leave "connected but dead" wedged beyond
      // the client's own 45s/120s constants. Same rescue as the CONNECTING
      // wedge: still CLOSING/CLOSED past the same window, drop the socket
      // reference and fall through to the normal backoff reconnect.
      // (Round-16 review correction: the original comment's "non-speculative"
      // did not match the implementation, and "strictly isomorphic to the
      // delivery path" did not hold either — the delivery path does count one
      // speculative failure for a speculative socket that is not the
      // speculativeSource (the close handler's accounting branch) while the
      // rescue does not, so the failure ladder catches up a cycle later; a
      // socket silently rescued from OPEN after joining had in fact proven
      // liveness. The real behavioral contract: a rescue death does not count
      // a speculative failure and does not clear speculativeReconnect; the
      // successor cycle's own join verification decides whether the UI leaves
      // connected.) The scheduleReconnect copy suppression inside a
      // speculative cycle is the speculative semantics itself, consistent
      // with the delivery path; only the successor's own verification failure
      // (close arrives before the join ack) escalates the count. The rescue
      // only detaches socket/closingStartedAt — joined/leaseId and the other
      // flags keep their old-cycle values until the successor's join ack,
      // matching the existing "a transient drop must not tear down the UI"
      // design. (Round-18 review correction: "rescue deaths never count" only
      // holds for the speculative source socket; a successor rescued inside a
      // speculative cycle (not the speculativeSource) has not yet passed join
      // verification, and its death counts one speculative failure via the
      // same verification branch as the close handler — otherwise the ladder
      // in an all-rescue topology never advances and the UI stays on
      // connected forever. The implementation below matches.)(Round-21 review
      // reconciliation: "the rescue only detaches socket/closingStartedAt and
      // the other flags keep their old values" diverges deliberately from the
      // event-failure catch path (which resets the four flags
      // joined/desktopOnline/desktopCapabilitiesReady/leaseId and whose
      // comment calls the residue a cross-cycle leak) — this is intentional,
      // not an oversight: the catch path runs after a delivered event already
      // polluted cycle state, so the reset is damage control; the rescue runs
      // in a silent wedge with no delivered event, where preserving UI/
      // negotiation stability is the established design. An honest record of
      // the cost: in the window between the successor socket's OPEN and its
      // join ack, the sendJoined/sendRpc gates are still open on the old
      // cycle — with the desktop online, the relay re-mints lease_id and the
      // flow is unaffected; with the desktop offline, RPCs in that window get
      // desktop_offline and fall into the catch-all error branch (the UI
      // surfaces the error and the RPC waits out its full timeout instead of
      // queueing a flush) — the window occurs once per cycle and closes after
      // the next cycle's join ack, recorded as an accepted trade-off;
      // aligning the two paths would be a behavior change and, if ever done,
      // must bring its own wedge regression pins.)
      if (
        this.socket.readyState === WebSocket.CLOSING
        || this.socket.readyState === WebSocket.CLOSED
      ) {
        if (!this.closingStartedAt) {
          this.closingStartedAt = Date.now();
          return;
        }
        if (Date.now() - this.closingStartedAt <= LIVENESS_CONNECT_WEDGE_TIMEOUT_MS) return;
        this.closingStartedAt = null;
        const rescued = this.socket;
        this.socket = null;
        // Round-18 review: the rescue stands in for a close event that will
        // never be delivered, so it must carry the close handler's
        // verification branch along — if a socket detached by the rescue
        // inside a speculative cycle is not the speculative source itself (it
        // is a successor issued by the speculative reconnect whose join ack
        // has not arrived), its death is a verification failure and must count
        // toward the failure ladder; otherwise an all-rescue topology (every
        // close wedged and never delivered) pins the ladder at 0 forever, the
        // UI stays on connected forever, and every RPC hangs until its own
        // timeout (probe end-to-end reproduction). The count happens before
        // scheduleReconnect: once speculativeReconnect is cleared, this
        // reconnect's copy is no longer suppressed by the speculative
        // semantics and becomes visible to the user again.
        if (this.speculativeReconnect && rescued !== this.speculativeSource) {
          this.noteSpeculativeFailure();
        }
        this.scheduleReconnect("连接已断开，正在重试…");
        return;
      }
      if (this.socket.readyState !== WebSocket.OPEN) return;
      if (Date.now() - this.lastInboundAt <= LIVENESS_SILENCE_TIMEOUT_MS) return;
      // A cycle that never became usable — no join ack, or an ack whose
      // capability snapshot never arrived (relay acks but blackholes the
      // desktop-bound forwarding path) — is not a healthy idle cycle:
      // past the failure cap the reconnect becomes visible instead of
      // speculatively repeating forever (see LIVENESS_MAX_SPECULATIVE_FAILURES).
      this.speculativeClose(this.socket);
    }

    // Shared tail of the two speculative-close sites (liveness watchdog and
    // the `online` handler). Round-11: extracted from two byte-identical
    // copies that relied on an edit-together comment — one method removes the
    // drift hazard instead of pinning it.
    speculativeClose(socket) {
      this.closingStartedAt = null;
      if (!this.joined || (this.awaitingCapabilitySnapshot && !this.desktopCapabilitiesReady)) {
        this.speculativeFailures += 1;
      }
      this.speculativeReconnect =
        this.speculativeFailures <= LIVENESS_MAX_SPECULATIVE_FAILURES;
      this.speculativeSource = socket;
      try { socket.close(); } catch { this.clearSpeculativeReconnect(); }
    }

    // OS-level connectivity change: neither the old socket nor the old
    // backoff is trustworthy across it.
    handleNetworkBecomeOnline() {
      if (this.closedPermanently) return;
      if (this.socket && this.socket.readyState === WebSocket.OPEN) {
        // An OPEN socket looks alive, but the OS just said connectivity
        // changed: treat it as the same suspicion as the liveness watchdog.
        // Like the watchdog, the close is speculative — the UI stays on
        // "connected" unless the cycle never became usable, and such a
        // cycle counts toward the same failure cap.
        this.speculativeClose(this.socket);
        return;
      }
      if (this.socket && this.socket.readyState === WebSocket.CONNECTING) {
        // A CONNECTING socket is wedged on the browser TCP timeout; abort it
        // so recovery reroutes through the close handler immediately. The
        // UI is already in the visible connecting state.
        try { this.socket.close(); } catch { /* socket already closed */ }
        return;
      }
      this.reconnectAttempt = 0;
      if (this.reconnectTimer) {
        window.clearTimeout(this.reconnectTimer);
        this.reconnectTimer = null;
      }
      this.connect();
    }

    // Waking tab: reconnect immediately when no socket is live. Do NOT
    // speculatively drop an OPEN socket here — backgrounded tabs throttle
    // timers, so liveness state is stale on wake; the watchdog and the online
    // event cover that case. A CONNECTING socket gets the same abort as in
    // handleNetworkBecomeOnline: recovery must not wait out the TCP timeout.
    handlePageVisible() {
      if (this.closedPermanently) return;
      if (this.socket && this.socket.readyState === WebSocket.OPEN) return;
      if (this.socket && this.socket.readyState === WebSocket.CONNECTING) {
        try { this.socket.close(); } catch { /* socket already closed */ }
        return;
      }
      this.reconnectAttempt = 0;
      if (this.reconnectTimer) {
        window.clearTimeout(this.reconnectTimer);
        this.reconnectTimer = null;
      }
      this.connect();
    }

    bindNetworkListeners() {
      if (typeof window.addEventListener !== "function") return;
      // Round-20 review (disclosure recorded in-tree): online/visibilitychange
      // handling a CLOSING socket with cancel-then-noop (canceling the
      // reconnect timer but otherwise doing nothing for the
      // un-cancelable CLOSING state) is intentional watchdog design —
      // CLOSING/CLOSED is owned by the wedge-window rescue path, and network
      // events must not compete with it (round-15 disclosure).
      window.addEventListener("online", () => this.handleNetworkBecomeOnline());
      window.addEventListener("visibilitychange", () => {
        if (document.visibilityState !== "visible") return;
        this.handlePageVisible();
      });
    }

    sendRaw(value) {
      if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return false;
      this.socket.send(JSON.stringify(value));
      return true;
    }

    sendJoined(value) {
      if (!this.joined || !this.desktopOnline) return false;
      return this.sendRaw({ ...value, v: protocolVersion, lease_id: this.leaseId });
    }

    // eslint-disable-next-line sonarjs/cognitive-complexity -- legacy bridge; refactor tracked separately
    handleMessage(raw, sourceSocket) {
      if (typeof raw !== "string") return;
      let message;
      try { message = JSON.parse(raw); } catch { return; }
      // Round-18 review: the same identity contract as the close/error event
      // gates. Cycle-state frames (join ack / desktop_connection_state /
      // desktop_snapshot /
      // endpoint_revoked / endpoint_replaced / error) previously had no
      // identity gate: frames still in flight on a socket detached by the
      // rescue or by an event-failure would keep arriving into the successor
      // cycle — a late old join ack would knock an already-negotiated
      // successor back to the unnegotiated state (the next inbound event then
      // hits !desktopCapabilitiesReady and processRemoteEvent permanently
      // shuts the door as incompatible_desktop), and a late
      // error/endpoint_replaced would this.socket.close()/closePermanently a
      // healthy successor. The flags belong to the current cycle; frames from
      // an old socket are pure noise and are dropped together with their
      // liveness evidence (the event dispatch chain has its own identical
      // identity gate inside handleRemoteEvent; the two gates share
      // semantics).
      // Round-20 review exception: a non-TTL endpoint_revoked is a link-level
      // fact — the relay only sends it after the endpoint was truly revoked
      // (manually or otherwise), endpoint ids are minted monotonically by the
      // relay, and a revoked endpoint can never join successfully again (only
      // endpoint_not_found), so a copy delivered by an old socket is equally
      // true. If the frame is dropped wholesale as noise and its only copy
      // lands in a detached socket's window, a real revocation degenerates
      // into the endpoint_not_found "desktop offline" retry loop (the client
      // deliberately classifies that code as transient) and the revocation
      // card becomes unreachable. Non-TTL revocation frames bypass the
      // identity gate straight to the terminal state; the TTL arm stays
      // gated — an expired desktop_offline_ttl_expired copy would knock a
      // healthy successor back to desktop_offline (a fresh copy after the
      // rejoin will reach the current socket anyway).
      // Round-22 review: once the terminal state is established
      // (closedPermanently) the bypass must stop — this.socket is already
      // null, every other late frame is dropped by the identity gate below,
      // and only this bypass would re-enter closePermanently: re-dispatching
      // connection events (to self-recovering listeners) and overwriting the
      // established terminal card (e.g. endpoint_replaced's "opened in
      // another browser") with the revocation copy. closePermanently is
      // idempotent state-wise, so this is mere event noise / card flapping,
      // not a stability defect; one guard line closes it.
      if (this.closedPermanently) return;
      if (
        sourceSocket !== this.socket
        && message.type === "endpoint_revoked"
        && message.reason !== "desktop_offline_ttl_expired"
      ) {
        this.closePermanently("revoked", "此远程控制链接已失效。");
        return;
      }
      if (sourceSocket !== this.socket) return;
      // Any parseable inbound frame counts as liveness evidence.
      this.lastInboundAt = Date.now();
      switch (message.type) {
        case "web_client_joined":
          // A TCP/WebSocket open only proves that the Relay is reachable. The
          // endpoint and token are known-good only after the authenticated
          // join acknowledgement, so transient endpoint misses must continue
          // to increase the retry delay.
          // Round-11: the speculative-failure ladder deliberately does NOT
          // reset here — the ack is generated by the relay on its own code
          // path and cannot vouch for the desktop-bound forwarding path. A
          // relay that acks every join while blackholing desktop frames
          // would re-zero the ladder every cycle and wedge the UI on
          // "connected" forever. The verified-good reset lives at the
          // capability-handshake completion, which only a real desktop frame
          // can reach.
          // Accepted trade-off (documented): the OTHER two ladders
          // (reconnectAttempt / desktopGoneRetries) still reset on the ack, so
          // a relay alternating ack+close can keep the phone reconnecting at
          // fast pace forever — self-DoS against the user's own relay only;
          // rate-limiting the reset is a follow-up if it ever matters.
          this.reconnectAttempt = 0;
          this.desktopGoneRetries = 0;
          this.clearSpeculativeReconnect();
          this.joined = true;
          this.lastInboundAt = Date.now();
          this.desktopOnline = message.desktop_connected !== false;
          this.desktopCapabilitiesReady = false;
          this.awaitingCapabilitySnapshot = this.desktopOnline;
          this.allowedCommands = new Set(this.webAllowedCommands);
          this.allowedEvents = new Set(this.webAllowedEvents);
          this.leaseId = message.lease_id || null;
          if (message.stream_epoch && !this.streamEpoch) this.streamEpoch = message.stream_epoch;
          this.setConnection(this.desktopOnline ? "connected" : "desktop_offline",
            this.desktopOnline ? "" : "桌面端离线，等待重新连接…");
          if (this.frontendReady) {
            this.flushSubscriptions();
            this.sendReady(false);
          }
          break;
        case "desktop_connection_state":
          this.desktopOnline = message.status === "connected";
          this.desktopCapabilitiesReady = false;
          this.awaitingCapabilitySnapshot = this.desktopOnline;
          this.setConnection(this.desktopOnline ? "connected" : "desktop_offline",
            this.desktopOnline ? "" : "桌面端离线，等待重新连接…");
          if (this.desktopOnline && this.frontendReady) {
            this.flushSubscriptions();
            this.sendReady(false);
          }
          break;
        case "rpc_response":
          this.handleRpcResponse(message);
          break;
        case "event":
          this.handleRemoteEvent(message, sourceSocket || this.socket);
          break;
        case "stream_reset":
          this.streamEpoch = message.stream_epoch || "";
          this.lastSeq = 0;
          this.persistCursor();
          window.location.reload();
          break;
        case "desktop_snapshot":
          if (message.stream_epoch) this.streamEpoch = String(message.stream_epoch);
          // Number(null) and Number("") are 0: a snapshot missing seq must not
          // reset the cursor to 0 (that would trigger a full replay next join).
          if (message.seq !== null && message.seq !== undefined && message.seq !== ""
            && Number.isFinite(Number(message.seq))) this.lastSeq = Number(message.seq);
          this.persistCursor();
          this.applyDesktopCapabilities(message.snapshot && message.snapshot.capabilities);
          break;
        case "endpoint_replaced":
          this.closePermanently("replaced", "此远程控制链接已在另一台浏览器中打开。");
          break;
        case "endpoint_revoked":
          // TTL expiry shares this message type but is NOT a revoke
          // (PROTOCOL.md): the desktop may recreate the endpoint with its
          // persistent credentials, so the tab must wait and rejoin instead of
          // showing a permanent dead end. Only any other reason (or none) is a
          // real revoke.
          if (message.reason === "desktop_offline_ttl_expired") {
            this.desktopOnline = false;
            this.reconnectForDesktopGone = true;
            this.setConnection("desktop_offline", "桌面端离线，等待重新连接…");
            try { this.socket && this.socket.close(); } catch { /* socket already closed */ }
          } else {
            this.closePermanently("revoked", "此远程控制链接已失效。");
          }
          break;
        case "error":
          if (message.code === "invalid_token") {
            this.closePermanently("denied", message.message || "远程控制访问被拒绝。");
          } else if (message.code === "endpoint_not_found") {
            // Relay endpoint state is deliberately ephemeral. After a Relay
            // restart the browser often reconnects before the persistent
            // desktop endpoint, so keep retrying instead of invalidating the
            // user's stable link.
            this.desktopOnline = false;
            this.reconnectForDesktopGone = true;
            this.setConnection("desktop_offline", "等待桌面端重新连接…");
            try { this.socket && this.socket.close(); } catch { /* socket already closed */ }
          } else {
            this.setConnection("error", message.message || "远程控制连接异常。");
          }
          break;
        default:
          break;
      }
    }

    closePermanently(status, message) {
      this.closedPermanently = true;
      this.joined = false;
      this.desktopOnline = false;
      this.desktopCapabilitiesReady = false;
      this.clearSpeculativeReconnect();
      this.negotiatedCapabilitiesKnown = false;
      this.negotiatedCommands.clear();
      this.negotiatedEvents.clear();
      if (this.reconnectTimer) window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
      if (this.livenessTimer) {
        window.clearInterval(this.livenessTimer);
        this.livenessTimer = null;
      }
      try { this.socket && this.socket.close(); } catch { /* socket already closed */ }
      this.socket = null;
      const error = new Error(message);
      this.pending.forEach((entry) => {
        window.clearTimeout(entry.timeout);
        entry.reject(error);
      });
      this.pending.clear();
      window.dispatchEvent(new CustomEvent("pinvou:web-capabilities", {
        detail: { commands: [], events: [] },
      }));
      this.setConnection(status, message);
    }

    persistCursor() {
      if (!this.cursorKey) return;
      try {
        sessionStorage.setItem(this.cursorKey, JSON.stringify({
          stream_epoch: this.streamEpoch,
          after_seq: this.lastSeq,
        }));
      } catch { /* keep the cursor in memory only when sessionStorage is unavailable */ }
    }

    handleRemoteEvent(message, sourceSocket = this.socket) {
      this.eventDispatch = this.eventDispatch
        .then(async () => {
          if (this.closedPermanently || !sourceSocket || this.socket !== sourceSocket) return;
          await this.processRemoteEvent(message);
        })
        .catch((error) => {
          console.error("[WebBridge] event processing failed; reconnecting for replay", error);
          if (this.closedPermanently || this.socket !== sourceSocket) return;
          // Invalidate this socket immediately so already-queued messages from
          // the same connection cannot overtake the failed event. The durable
          // cursor is unchanged, so the desktop replays it after reconnect.
          // The invalidation detaches the socket, so its close event is noise
          // under the close handler's identity gate: this path must reset the
          // cycle flags and schedule the replay reconnect itself (a round-16
          // probe asserted the reconnect timer and found none — the designed
          // stateNotReady / sequence-gap recovery stalled until an OS
          // online/visibilitychange event).
          this.socket = null;
          this.joined = false;
          this.desktopOnline = false;
          this.desktopCapabilitiesReady = false;
          this.leaseId = null;
          // Round-25 review NIT: closedPermanently/connection semantics also
          // invalidate inside the catch — clear the wedge clock here too, so
          // the documented "every detach path clears this field" contract
          // holds literally (rescue/speculativeClose/connect all clear it).
          this.closingStartedAt = null;
          try { sourceSocket.close(); } catch { /* already closing */ }
          // Delegating the reconnect to the close event instead left a zombie
          // "connected" client with no transport whenever close() delivered
          // normally; scheduleReconnect self-guards, so the gated late
          // close/error events stay harmless.
          // Round-19 review: same verification branch as the rescue (the
          // speculative-count leg of the liveness section) — if a socket
          // detached by an event failure inside a speculative cycle is not the
          // speculative source itself, this proven processing failure must
          // count toward the failure ladder and clear the speculative flag:
          // without clearing the flag, this reconnect's copy stays suppressed
          // by the speculative semantics for one more cycle (a proven failure
          // announced as "suspected"), and the ladder undercounts by one. The
          // speculative source itself is still forgiven under the existing
          // contract — its death is attributed by our own speculative close,
          // and its successor cycle brings its own join verification
          // (round-25 correction: the old note "the speculative source never
          // passed join verification" does not hold — the speculative source
          // may be fully joined; that rationale is load-bearing, and changing
          // the count on its basis would regress the ladder).
          if (this.speculativeReconnect && sourceSocket !== this.speculativeSource) {
            this.noteSpeculativeFailure();
          }
          this.scheduleReconnect("事件处理失败，正在重新连接…");
        });
      return this.eventDispatch;
    }

    async processRemoteEvent(message) {
      if (!this.frontendReady || !this.stateReady) {
        // Never acknowledge replay data before the shared bridge has attached
        // its listeners and loaded its durable Session index. Closing forces
        // the desktop journal to replay from the unchanged cursor after both
        // readiness barriers, including when an older/buggy desktop ignores
        // the state_ready=false phase.
        throw new Error(errorText("stateNotReady"));
      }
      if (!this.desktopCapabilitiesReady || !this.allowedEvents.has(message.event)) {
        this.closePermanently(
          "incompatible_desktop",
          errorText("unnegotiatedEvent", message.event || "unknown"),
        );
        return;
      }
      const epoch = String(message.stream_epoch || "");
      const seq = Number(message.seq || 0);
      if (this.streamEpoch && epoch && epoch !== this.streamEpoch) {
        this.streamEpoch = epoch;
        this.lastSeq = 0;
      }
      if (epoch) this.streamEpoch = epoch;
      if (seq && seq <= this.lastSeq) return;
      if (seq && this.lastSeq && seq !== this.lastSeq + 1) {
        throw new Error(`remote event sequence gap: expected ${this.lastSeq + 1}, got ${seq}`);
      }
      const callbacks = this.listeners.get(message.event);
      const event = { event: message.event, id: seq, payload: message.payload };
      if (callbacks) {
        for (const callback of callbacks) await callback(event);
      }
      if (seq) this.lastSeq = seq;
      this.persistCursor();
    }

    handleRpcResponse(message) {
      const entry = this.pending.get(message.id);
      if (!entry) return;
      this.pending.delete(message.id);
      window.clearTimeout(entry.timeout);
      if (message.ok === false) {
        const error = new Error(message.error || errorText("rpcFailed"));
        error.code = message.error_code || "rpc_failed";
        if (message.error_category) error.category = message.error_category;
        error.requestId = entry.id;
        entry.reject(error);
      } else entry.resolve(message.result);
    }

    applyDesktopCapabilities(capabilities) {
      const commands = capabilities && Array.isArray(capabilities.commands)
        ? new Set(capabilities.commands)
        : null;
      const events = capabilities && Array.isArray(capabilities.events)
        ? new Set(capabilities.events)
        : null;
      if (!commands || !events || Number(capabilities.protocol_version) !== protocolVersion) {
        const error = new Error(errorText("incompatibleDesktop"));
        this.pending.forEach((entry) => {
          window.clearTimeout(entry.timeout);
          entry.reject(error);
        });
        this.pending.clear();
        this.closePermanently("incompatible_desktop", error.message);
        return;
      }
      this.allowedCommands = new Set(
        [...this.webAllowedCommands].filter((command) => commands.has(command)),
      );
      this.allowedEvents = new Set(
        [...this.webAllowedEvents].filter((eventName) => events.has(eventName)),
      );
      this.negotiatedCommands = new Set(this.allowedCommands);
      this.negotiatedEvents = new Set(this.allowedEvents);
      this.negotiatedCapabilitiesKnown = true;
      const completesCapabilityHandshake = this.awaitingCapabilitySnapshot;
      this.awaitingCapabilitySnapshot = false;
      this.desktopCapabilitiesReady = true;
      // A cycle that reached the capability handshake is verified good end
      // to end — only a real desktop-bound frame can land here — so this is
      // where the speculative-failure ladder resets (round-11; previously at
      // the join ack, which a frameless relay could mint every cycle).
      this.speculativeFailures = 0;
      window.dispatchEvent(new CustomEvent("pinvou:web-capabilities", {
        detail: { commands: [...this.allowedCommands], events: [...this.allowedEvents] },
      }));
      this.pending.forEach((entry, id) => {
        if (this.allowedCommands.has(entry.command)) return;
        this.pending.delete(id);
        window.clearTimeout(entry.timeout);
        entry.reject(new Error(errorText("unsupportedCommand", entry.command)));
      });
      this.flushSubscriptions();
      this.flushPending();
      if (completesCapabilityHandshake && this.frontendReady && this.stateReady
          && this.joined && this.desktopOnline) {
        this.sendReady(true);
      }
    }

    async invoke(command, args) {
      return this.invokeWithRequestId(command, args, randomId("rpc"));
    }

    async invokeWithRequestId(command, args, requestId) {
      await this.policyPromise;
      if (!this.allowedCommands.has(command)) throw new Error(errorText("commandNotAllowed", command));
      const id = String(requestId || "").trim();
      if (!/^[A-Za-z0-9_-]{8,256}$/.test(id)) {
        throw new Error(errorText("invalidRequestId"));
      }
      if (this.pending.has(id)) {
        throw new Error(errorText("requestInFlight"));
      }
      return new Promise((resolve, reject) => {
        const entry = {
          id, command, args: args || {}, resolve, reject,
          timeout: window.setTimeout(() => {
            if (!this.pending.delete(id)) return;
            const error = new Error(errorText("invokeTimeout", command));
            error.code = "rpc_timeout";
            error.requestId = id;
            reject(error);
          }, 180_000),
        };
        this.pending.set(id, entry);
        this.sendRpc(entry);
      });
    }

    supportsCommand(command) {
      return this.desktopCapabilitiesReady && this.allowedCommands.has(command);
    }

    supportsCapability(capability) {
      if (WEB_CAPABILITIES[capability] !== true) return false;
      const required = SEMANTIC_COMMAND_REQUIREMENTS[capability];
      if (!required) return true;
      const requiredCommands = Array.isArray(required) ? required : (required.commands || []);
      const requiredEvents = Array.isArray(required) ? [] : (required.events || []);
      if (!requiredCommands.length && !requiredEvents.length) return true;
      // Fail closed until the first authoritative snapshot. Afterwards retain
      // compatibility across transient transport loss so React keeps feature
      // state mounted; supportsCommand/sendRpc remain online-only.
      if (!this.negotiatedCapabilitiesKnown) return false;
      return requiredCommands.every((command) => this.negotiatedCommands.has(command))
        && requiredEvents.every((eventName) => this.negotiatedEvents.has(eventName));
    }

    sendRpc(entry) {
      if (!this.frontendReady || !this.desktopCapabilitiesReady) return false;
      this.sendJoined({
        v: protocolVersion,
        type: "rpc_request",
        id: entry.id,
        client_request_id: entry.id,
        command: entry.command,
        args: entry.args,
      });
      return true;
    }

    sendReady(stateReady = this.stateReady) {
      this.sendJoined({
        v: protocolVersion,
        type: "client_ready",
        stream_epoch: this.streamEpoch || null,
        after_seq: this.lastSeq,
        state_ready: stateReady,
      });
    }

    flushPending() { this.pending.forEach((entry) => { this.sendRpc(entry); }); }

    async listen(eventName, callback) {
      this.pendingListenerRegistrations += 1;
      try {
        await this.policyPromise;
        if (!this.webAllowedEvents.has(eventName)) return function () {};
        let callbacks = this.listeners.get(eventName);
        if (!callbacks) {
          callbacks = new Set();
          this.listeners.set(eventName, callbacks);
        }
        callbacks.add(callback);
        this.subscribeEvent(eventName);
        return () => {
          const current = this.listeners.get(eventName);
          if (!current) return;
          current.delete(callback);
          if (current.size) return;
          this.listeners.delete(eventName);
          this.subscribed.delete(eventName);
          this.sendJoined({ type: "event_unsubscribe", event: eventName });
        };
      } finally {
        this.pendingListenerRegistrations = Math.max(0, this.pendingListenerRegistrations - 1);
        this.tryCompleteFrontendReady();
      }
    }

    subscribeEvent(eventName) {
      if (!this.frontendReady) return;
      if (!this.desktopCapabilitiesReady || !this.allowedEvents.has(eventName)) return;
      if (this.subscribed.has(eventName)) return;
      if (!this.sendJoined({ type: "event_subscribe", event: eventName })) return;
      this.subscribed.add(eventName);
    }

    flushSubscriptions() {
      this.subscribed.clear();
      this.listeners.forEach((_callbacks, eventName) => { this.subscribeEvent(eventName); });
    }

    markFrontendReady() {
      this.frontendReadyRequested = true;
      this.policyPromise.then(() => this.tryCompleteFrontendReady()).catch((error) => {
        this.setConnection("error", String(error && error.message ? error.message : error));
      });
    }

    markStateReady() {
      if (this.stateReady) return;
      this.stateReady = true;
      if (this.frontendReady && this.joined && this.desktopOnline
          && this.desktopCapabilitiesReady && !this.awaitingCapabilitySnapshot) {
        this.sendReady(true);
      }
    }

    tryCompleteFrontendReady() {
      if (!this.frontendReadyRequested || this.frontendReady || this.pendingListenerRegistrations > 0) return;
      if (!this.allowedCommands || !this.allowedEvents) return;
      this.frontendReady = true;
      if (this.joined && this.desktopOnline) {
        this.flushSubscriptions();
        this.awaitingCapabilitySnapshot = true;
        this.sendReady(false);
      }
    }
  }

  const client = new WebTauriClient();
  window.PinvouWebClient = client;
  window.PinvouPlatform = Object.freeze({
    kind: "web",
    isWeb: true,
    capabilities: Object.freeze(WEB_CAPABILITIES),
    can(capability) { return client.supportsCapability(capability); },
    canInvoke(command) { return client.supportsCommand(command); },
    areInvokeCapabilitiesReady() { return client.desktopCapabilitiesReady === true; },
    getConnectionState() { return client.connectionState; },
    onConnectionChange(listener) {
      client.connectionListeners.add(listener);
      queueMicrotask(() => listener(client.connectionState));
      return () => client.connectionListeners.delete(listener);
    },
  });
  window.__TAURI__ = {
    core: {
      invoke(command, args) { return client.invoke(command, args); },
      invokeWithRequestId(command, args, requestId) {
        return client.invokeWithRequestId(command, args, requestId);
      },
    },
    event: {
      listen(eventName, callback) { return client.listen(eventName, callback); },
      emit() { return Promise.resolve(); },
      emitTo() { return Promise.resolve(); },
    },
    // Kept as an empty shell: host-file-picker.js installs the remote host
    // picker onto __TAURI__.dialog.open right after this script, and the
    // bridge captures it from there. The previous "__dialog_open" RPC stub
    // was unreachable: host-file-picker overwrites it before any reader, and
    // the desktop relay handler explicitly denies the "__dialog_open"
    // command anyway (tauri/bridge/remote-control.js).
    dialog: {},
  };
})();
