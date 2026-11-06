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

  // invoke 拒绝的 Error 文案会经 bridge addSystemItem 进入界面。文案单一来源是
  // shared/i18n.js 的 uiPlatformMisc.webClientErrors,由 React 入口按当前语言挂到
  // window.PinvouWebClientStrings;此处保留中文兜底(纯脚本无法 import ES module)。
  // 连接状态 message 不进 UI(WebConnectionStatus 统一使用 uiWebConnection 字典),无需本地化。
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

    // 缺凭证的阻断终态:online/visible/看门狗等任何重连路径都不得把它
    // 转成注定失败的 join 重试循环(小转圈覆盖阻断卡),一律停回这张卡。
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
        // Round-16 复审 MAJOR-1:晚到的 close 只许处置它自己的 socket。
        // CLOSING/CLOSED 救援会把 socket 引用先行摘除(this.socket=null),
        // 其 close 事件因此可能在后继周期已经 join 之后才送达;若此时仍
        // 无条件清旗标/再排重连,健康后继会被打回未协商态——下一个入站
        // 事件撞上 !desktopCapabilitiesReady 就被 processRemoteEvent 判成
        // incompatible_desktop 永久关门(probe 端到端复现)。身份不符的
        // close 是纯噪声:旗标属于当前周期,与这颗旧 socket 无关。事件
        // 处理失败主动摘除的 socket 同样落进门内,但那条路径自己排重连
        // (handleRemoteEvent 的 catch),不依赖本处理器的无条件分支。
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
        // Round-16 复审:与 close 处理器同一身份门——被救援摘除的 socket
        // 晚到的 error 同样是噪声,不得把后继周期的旗标/投机计数打脏。
        if (this.socket !== socket) return;
        // 看门狗/网络恢复的投机关闭在半开 socket 上会先触发 error(部分
        // 浏览器把失败的关闭握手报成 error+close):这与自然死亡不同——
        // 验证尚未开始,既不能计入投机失败,也不能提前点亮 connecting,
        // 否则「UI 保持 connected 直到验证失败」的语义随浏览器引擎分叉。
        // 投机源 socket 的 error 交给 close 处理器统一收口(那边的身份门
        // 同样不计数);非投机死亡保持原语义。
        if (this.speculativeReconnect && socket === this.speculativeSource) return;
        this.noteSpeculativeFailure();
        // 异常断线先于 close 触发 error:desktop_offline 的专属文案同样不得
        // 被这里的 connecting 覆盖(close→scheduleReconnect 一侧有同款门),
        // 否则离线窗口内的 TCP reset 会把「桌面端离线」换成误导性转圈。
        if (!this.closedPermanently && this.connectionState.status !== "desktop_offline") {
          this.setConnection("connecting", "连接异常，正在重试…");
        }
      });
    }

    scheduleReconnect(message) {
      if (this.closedPermanently || this.reconnectTimer) return;
      // desktop_offline 的专属文案(「桌面端离线,保持页面打开」)会被紧随
      // 其后的 close→scheduleReconnect 的 connecting 覆盖,用户在最长 60s
      // 的退避窗口里只看得到「连接中」。桌面离线类失败保持该状态度过整个
      // 退避窗口,由重连发起(connect)或 join ack 推进。
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
      // Round-15 复审:CLOSING/CLOSED 态此前被永久忽略——投机 close() 在
      // 静默掉线的链路上(NAT 超时、无 online 事件)可能永远收不到 close
      // 事件,引擎的 closing-handshake 超时差异让「已连接但死链路」楔在
      // 客户端自己的 45s/120s 常量之外。与 CONNECTING 楔口同一救援:超过
      // 同一窗口仍在 CLOSING/CLOSED,丢弃 socket 引用并走普通退避重连。
      // (Round-16 复审订正:原注释的「非投机」与实现不符;「与送达路径
      // 严格同构」也不成立——送达路径对非 speculativeSource 的投机
      // socket 会计一次投机失败(close 处理器的计账分支),救援不记,失
      // 败阶梯晚一个周期补上;joined 后从 OPEN 静默被救的 socket 其实证
      // 明过存活。真实的行为契约是:救援的死不计投机失败、不清
      // speculativeReconnect,后继周期自己的 join 验证决定 UI 是否走出
      // connected。)投机周期内 scheduleReconnect 的文案抑制是投机语义
      // 本身,与送达路径一致;只有后继自己验证失败(close 先于 join ack
      // 送达)才会计数升级。救援只摘 socket/closingStartedAt——joined/
      // leaseId 等旗标保持旧周期值直到后继 join ack,与「瞬态断线不卸载
      // UI」的既有设计一致。(Round-18 复审订正:「救援的死一律不计」只
      // 对投机源 socket 成立;投机周期内被救援的后继(非
      // speculativeSource)尚未通过 join 验证,其死按 close 处理器同一
      // 验证分支计一次投机失败——否则全救援拓扑的阶梯永不前进,UI 永远
      // 停在 connected。下方实现同步。)(Round-21 复审 reconciliation:
      // 「救援只摘 socket/closingStartedAt、其余旗标保持旧值」与事件失败
      // catch 路径(reset joined/desktopOnline/desktopCapabilitiesReady/
      // leaseId 四旗,其注释称残留为跨周期泄漏)是有意分歧,不是疏漏:
      // catch 路径发生在已投递的事件把周期状态污染之后,重置是止损;救
      // 援发生在无投递事件的静默楔口,保留 UI/协商稳定是既有设计。代价
      // 的诚实记录:后继 socket OPEN 到 join ack 之间的窗口里 sendJoined/
      // sendRpc 闸门仍按旧周期开着——桌面在线时 relay 重打 lease_id,流
      // 程不受影响;桌面离线时该窗口的 RPC 会收到 desktop_offline 并落进
      // catch-all error 分支(UI 翻出错误、RPC 等满超时而非排队 flush)
      // ——窗口一周期一次、下一周期 join ack 后闭合,按已接受取舍记录;
      // 两种路径的对齐是行为变更,若做须带各自的楔口回归钉。)
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
        // Round-18 复审:救援替代了永远不会送达的 close 事件,因此必须把
        // close 处理器的验证分支一同带过来——投机周期内被救援摘除的
        // socket 若不是投机源本身(它是投机重连发出的后继,join ack 尚未
        // 送达),它的死就是验证失败,必须计入失败阶梯;否则全救援拓扑
        // (每颗 close 都楔死、永不送达)会让阶梯永远停在 0,UI 永远停留
        // 在 connected,RPC 全部悬到各自的超时(probe 端到端复现)。计数
        // 在 scheduleReconnect 之前:清掉 speculativeReconnect 后,本次
        // 重连的文案不再被投机语义抑制,恢复对用户可见。
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
      // Round-20 复审(披露落树):online/visibilitychange 对 CLOSING
      // socket 的 cancel-then-noop(取消重连定时器,但对不可取消的
      // CLOSING 态本身不做任何事)是有意的看门狗设计——CLOSING/CLOSED
      // 由楔窗口救援路径处理,网络事件不与其竞争(round-15 披露记录)。
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
      // Round-18 复审:与 close/error 事件门同一身份契约。周期状态帧
      // (join ack / desktop_connection_state / desktop_snapshot /
      // endpoint_revoked / endpoint_replaced / error)此前无身份门:被
      // 救援或事件失败摘除的 socket 上在途的帧仍会在后继周期送达——
      // 晚到的旧 join ack 会把已完成协商的后继打回未协商态(下一个入站
      // 事件撞上 !desktopCapabilitiesReady 被 processRemoteEvent 判成
      // incompatible_desktop 永久关门),晚到的 error/endpoint_replaced
      // 会 this.socket.close()/closePermanently 波及健康后继。旗标属于
      // 当前周期,旧 socket 的帧是纯噪声,连同 liveness 证据一起丢弃
      // (事件分派链在 handleRemoteEvent 内另有同一身份门,两道闸口语义
      // 一致)。
      // Round-20 复审例外:非 TTL 的 endpoint_revoked 是链接级事实——
      // relay 只在端点被真正吊销/人工撤销后发送,端点 id 由 relay 单调
      // 铸造,被撤销的端点不可能再 join 成功(只会 endpoint_not_found),
      // 旧 socket 送达的副本同样为真。整帧按噪声丢弃后,唯一副本落在
      // 被摘除 socket 的窗口里时,真实撤销退化为 endpoint_not_found 的
      // 「桌面离线」重试环(该码被客户端有意分类为瞬态),撤销卡片不可
      // 达。非 TTL 撤销帧旁路身份门直接终态;TTL 臂保持受门——过期的
      // desktop_offline_ttl_expired 副本会把健康后继打回 desktop_offline
      // (rejoin 后的新副本自会送达当前 socket)。
      // Round-22 复审:终态已确立后(closedPermanently)旁路必须停步——
      // this.socket 已为 null,其余晚到帧都被下方身份门丢弃,唯独本旁路
      // 会重入 closePermanently:重复派发连接事件(自恢复监听方)并把
      // 已确立的终态卡片(如 endpoint_replaced 的「已在另一台浏览器中
      // 打开」)覆盖成撤销文案。closePermanently 状态上幂等,故仅是
      // 事件噪声/卡片翻转,不是稳定性缺陷;一行守卫关掉。
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
          try { sourceSocket.close(); } catch { /* already closing */ }
          // Delegating the reconnect to the close event instead left a zombie
          // "connected" client with no transport whenever close() delivered
          // normally; scheduleReconnect self-guards, so the gated late
          // close/error events stay harmless.
          // Round-19 复审:与救援同一验证分支(bootstrap.js:463)——投机周期
          // 内被事件失败摘除的 socket 若不是投机源本身,这次已证实的处理
          // 失败必须计入失败阶梯并清掉投机旗标:不清旗标,本次重连文案仍
          // 被投机语义压住一个周期(已证实的失败被宣布成「疑似」),阶梯
          // 也少计一次。投机源自身(从未通过 join 验证)仍按既有契约宽恕。
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
