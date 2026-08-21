/** Browser-side Tauri compatibility layer for the shared Full WebUI. */
(function () {
  "use strict";

  const WEB_CAPABILITIES = {
    desktopChrome: false, detachWindows: false, pet: false, oauth: false,
    externalAuth: false, superPermission: false, appUpdate: false,
    dependencyInstall: false, localModelSetup: false, externalSystemOpen: false,
    webAccessAdmin: false, desktopNotifications: false, hostFilePicker: true, artifactDownload: true,
    browserMicrophone: true,
    sessionModelSwitch: true,
    modelManagement: false,
    toolStoreMutations: false,
    deviceFileUpload: true,
  };
  const SEMANTIC_COMMAND_REQUIREMENTS = {
    hostFilePicker: ["web_access_list_host_files", "web_access_ingest_file"],
    artifactDownload: ["web_access_artifact_info", "web_access_read_artifact_chunk"],
    browserMicrophone: ["web_access_transcribe_voice_audio"],
    sessionModelSwitch: ["set_session_model"],
    deviceFileUpload: [
      "web_access_upload_attachment_chunk",
      "web_access_abort_attachment_upload",
      "web_access_discard_attachment",
    ],
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
    const entry = custom !== undefined ? custom : FALLBACK_ERRORS[key];
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
      return `${prefix}_${window.crypto.randomUUID()}`;
    }
    const bytes = new Uint8Array(18);
    if (window.crypto && typeof window.crypto.getRandomValues === "function") {
      window.crypto.getRandomValues(bytes);
    } else {
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
      this.pending = new Map();
      this.listeners = new Map();
      this.subscribed = new Set();
      this.frontendReadyRequested = false;
      this.frontendReady = false;
      this.stateReady = false;
      this.desktopCapabilitiesReady = false;
      this.awaitingCapabilitySnapshot = false;
      this.webAllowedCommands = new Set();
      this.webAllowedEvents = new Set();
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
        try { cursor = JSON.parse(sessionStorage.getItem(this.cursorKey) || "{}"); } catch (_) {}
      }
      this.streamEpoch = typeof cursor.stream_epoch === "string" ? cursor.stream_epoch : "";
      this.lastSeq = Number.isFinite(Number(cursor.after_seq)) ? Number(cursor.after_seq) : 0;
      if (endpointId && accessToken) this.connect();
      else queueMicrotask(() => this.setConnection("credentials_missing", "远程控制链接缺少访问凭证。"));
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
        try { listener(detail); } catch (_) {}
      });
    }

    connect() {
      if (this.closedPermanently || this.socket) return;
      this.setConnection("connecting", "正在连接桌面端…");
      let socket;
      try { socket = new WebSocket(this.url); }
      catch (error) { this.scheduleReconnect(String(error)); return; }
      this.socket = socket;
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
        if (this.socket === socket) this.socket = null;
        this.joined = false;
        this.desktopOnline = false;
        this.desktopCapabilitiesReady = false;
        this.leaseId = null;
        if (!this.closedPermanently) this.scheduleReconnect("连接已断开，正在重试…");
      });
      socket.addEventListener("error", () => this.setConnection("connecting", "连接异常，正在重试…"));
    }

    scheduleReconnect(message) {
      if (this.closedPermanently || this.reconnectTimer) return;
      this.setConnection("connecting", message);
      const base = Math.min(10_000, 500 * (2 ** Math.min(this.reconnectAttempt, 5)));
      const delay = Math.round(base * (0.8 + Math.random() * 0.4));
      this.reconnectAttempt += 1;
      this.reconnectTimer = window.setTimeout(() => {
        this.reconnectTimer = null;
        this.connect();
      }, delay);
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

    handleMessage(raw, sourceSocket) {
      if (typeof raw !== "string") return;
      let message;
      try { message = JSON.parse(raw); } catch (_) { return; }
      switch (message.type) {
        case "web_client_joined":
          // A TCP/WebSocket open only proves that the Relay is reachable. The
          // endpoint and token are known-good only after the authenticated
          // join acknowledgement, so transient endpoint misses must continue
          // to increase the retry delay.
          this.reconnectAttempt = 0;
          this.joined = true;
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
          if (Number.isFinite(Number(message.seq))) this.lastSeq = Number(message.seq);
          this.persistCursor();
          this.applyDesktopCapabilities(message.snapshot && message.snapshot.capabilities);
          break;
        case "endpoint_replaced":
          this.closePermanently("replaced", "此远程控制链接已在另一台浏览器中打开。");
          break;
        case "endpoint_revoked":
          this.closePermanently("revoked", "此远程控制链接已失效。");
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
            this.setConnection("desktop_offline", "等待桌面端重新连接…");
            try { this.socket && this.socket.close(); } catch (_) {}
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
      if (this.reconnectTimer) window.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
      try { this.socket && this.socket.close(); } catch (_) {}
      this.socket = null;
      const error = new Error(message);
      this.pending.forEach((entry) => {
        window.clearTimeout(entry.timeout);
        entry.reject(error);
      });
      this.pending.clear();
      this.setConnection(status, message);
    }

    persistCursor() {
      if (!this.cursorKey) return;
      try {
        sessionStorage.setItem(this.cursorKey, JSON.stringify({
          stream_epoch: this.streamEpoch,
          after_seq: this.lastSeq,
        }));
      } catch (_) {}
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
          this.socket = null;
          try { sourceSocket.close(); } catch (_) {
            this.scheduleReconnect("事件处理失败，正在重新连接…");
          }
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
        Array.from(this.webAllowedCommands).filter((command) => commands.has(command)),
      );
      this.allowedEvents = new Set(
        Array.from(this.webAllowedEvents).filter((eventName) => events.has(eventName)),
      );
      const completesCapabilityHandshake = this.awaitingCapabilitySnapshot;
      this.awaitingCapabilitySnapshot = false;
      this.desktopCapabilitiesReady = true;
      window.dispatchEvent(new CustomEvent("pinvou:web-capabilities", {
        detail: { commands: Array.from(this.allowedCommands), events: Array.from(this.allowedEvents) },
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
      if (!required || !required.length) return true;
      // Command-backed capabilities stay disabled until the installed
      // desktop reports its exact command snapshot. This lets a newer WebUI
      // fail closed when it is opened against an older desktop build.
      if (!this.desktopCapabilitiesReady) return false;
      return required.every((command) => this.allowedCommands.has(command));
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

    flushPending() { this.pending.forEach((entry) => this.sendRpc(entry)); }

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
      this.listeners.forEach((_callbacks, eventName) => this.subscribeEvent(eventName));
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
    dialog: { open(options) { return client.invoke("__dialog_open", { options: options || {} }); } },
  };
})();
