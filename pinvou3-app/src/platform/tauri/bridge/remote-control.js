/**
 * Persistent Web access administration for the desktop Tauri bridge.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["remote-control"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var listen = context.listen;
    var bt = context.bt;
    var desktopProxyStarted = false;
    var eventForwarders = {};
    var policyPromise = null;
    var bridgeGeneration = (function () {
      try {
        if (root.crypto && typeof root.crypto.randomUUID === "function") {
          return "webview_" + root.crypto.randomUUID().replace(/-/g, "_");
        }
      } catch (_) {}
      return "webview_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2);
    })();

    function loadAccessPolicy() {
      if (policyPromise) return policyPromise;
      var url = new URL("platform/web/access-policy.json", document.baseURI);
      policyPromise = fetch(url, { cache: "no-store" }).then(function (response) {
        if (!response.ok) throw new Error("Web access policy unavailable (" + response.status + ")");
        return response.json();
      }).then(function (policy) {
        return {
          commands: new Set(policy.allowed_commands || []),
          events: new Set(policy.allowed_events || []),
        };
      });
      return policyPromise;
    }

    function eventPayload(event) {
      return event && Object.prototype.hasOwnProperty.call(event, "payload") ? event.payload : (event || {});
    }

    function respondToWebAccess(requestId, ok, result, error) {
      return invoke("web_access_rpc_respond", {
        requestId: requestId,
        generation: bridgeGeneration,
        ok: !!ok,
        result: result === undefined ? null : result,
        error: error ? String(error) : null,
      }).catch(function (respondError) {
        console.warn("[WebAccess] failed to send RPC response", respondError);
      });
    }

    async function startDesktopProxy() {
      if (desktopProxyStarted || typeof listen !== "function" || typeof fetch !== "function") return;
      desktopProxyStarted = true;
      var policyReady = loadAccessPolicy();

      // Install every allowlisted desktop-side forwarder before the bridge
      // readiness ACK so browser subscriptions cannot miss early events.
      var eventForwardersReady = policyReady.then(function (policy) {
        return Promise.all(Array.from(policy.events).map(function (name) {
          if (eventForwarders[name]) return Promise.resolve();
          return listen(name, function (appEvent) {
            invoke("web_access_publish_event", {
              event: name,
              payload: appEvent ? appEvent.payload : null,
            }).catch(function () {});
          }).then(function (unlisten) {
            eventForwarders[name] = unlisten;
          });
        }));
      });

      var rpcListenerReady = listen("web_access:rpc_request", async function (event) {
        var request = eventPayload(event);
        var requestId = request.request_id || request.requestId || request.id;
        var requestGeneration = request.bridge_generation || request.bridgeGeneration;
        if (!requestId || requestGeneration !== bridgeGeneration) return;

        var mayExecute = false;
        try {
          mayExecute = await invoke("web_access_rpc_begin", {
            requestId: requestId,
            generation: bridgeGeneration,
          });
        } catch (error) {
          console.warn("[WebAccess] RPC begin barrier failed", error);
          return;
        }
        if (!mayExecute) return;

        var policy;
        try {
          policy = await policyReady;
        } catch (error) {
          console.error("[WebAccess] policy load failed", error);
          await respondToWebAccess(requestId, false, null, error);
          return;
        }

        var command = String(request.command || "");
        if (!policy.commands.has(command)) {
          await respondToWebAccess(requestId, false, null, bt("remoteCmdNotAllowed")(command));
          return;
        }
        if (command === "__dialog_open") {
          await respondToWebAccess(requestId, false, null, bt("remoteDialogDesktop"));
          return;
        }

        try {
          var result = await invoke(command, request.args || {});
          await respondToWebAccess(requestId, true, result, null);
        } catch (error) {
          await respondToWebAccess(requestId, false, null, error && error.message ? error.message : error);
        }
      });

      var subscribeListenerReady = listen("web_access:event_subscribe", async function (event) {
        var policy;
        try {
          policy = await policyReady;
        } catch (error) {
          console.error("[WebAccess] policy load failed", error);
          return;
        }
        var name = String(eventPayload(event).event || "");
        if (!name || !policy.events.has(name)) return;
        await eventForwardersReady;
      });

      // Forwarders remain installed for the lifetime of the authoritative main
      // WebView. Rust filters delivery according to the current Web lease.
      var unsubscribeListenerReady = listen("web_access:event_unsubscribe", function () {});
      // Keep the desktop indicator in sync with the actual browser connection.
      // The access endpoint is intentionally persistent, so `active` only means
      // that the QR/link remains valid; it does not mean a phone is connected.
      var statusListenerReady = listen("web_access:status", function (event) {
        state.webAccess = Object.assign({}, state.webAccess, eventPayload(event));
        notify();
      });

      try {
        await Promise.all([
          policyReady,
          eventForwardersReady,
          rpcListenerReady,
          subscribeListenerReady,
          unsubscribeListenerReady,
          statusListenerReady,
        ]);
        await invoke("web_access_bridge_ready", { generation: bridgeGeneration });
      } catch (error) {
        console.error("[WebAccess] desktop bridge readiness failed", error);
        throw error;
      }
    }

    // webAccess 状态写入无意图排序：start/stop/rotate 并发时后完成者会把
    // UI 指示写反（审计 a）。用户操作意图序号——陈旧响应作废，终态由最新
    // 意图的完成写入收敛：web_access:status 事件 payload 不含 starting，
    // 事件无法清理该标志，须由每个意图完成路径（含失败）显式兜底。
    var webAccessIntentSeq = 0;
    var webAccessStatusSeq = 0;

    async function refreshRemoteControlStatus(expectedIntentSeq) {
      var intentSeq = expectedIntentSeq === undefined ? webAccessIntentSeq : expectedIntentSeq;
      var statusSeq = ++webAccessStatusSeq;
      try {
        var status = await invoke("web_access_status");
        if (intentSeq !== webAccessIntentSeq || statusSeq !== webAccessStatusSeq) return status;
        state.webAccess = Object.assign({}, state.webAccess, status || {});
      } catch (error) {
        if (intentSeq !== webAccessIntentSeq || statusSeq !== webAccessStatusSeq) return;
        state.webAccess = Object.assign({}, state.webAccess, { last_error: String(error) });
      }
      notify();
    }

    async function startRemoteControl(options) {
      var seq = ++webAccessIntentSeq;
      var wasActive = !!state.webAccess.active;
      state.webAccess = Object.assign({}, state.webAccess, { starting: true, last_error: null });
      notify();
      try {
        var info = await invoke("web_access_enable", {
          allowHostWorkspace: !!(options && options.allowHostWorkspace),
        });
        // A newer user action owns the state now; discard this stale result.
        if (seq !== webAccessIntentSeq) return info;
        state.webAccess = Object.assign({}, state.webAccess, info || {}, {
          active: true, starting: false, last_error: null,
        });
        notify();
        await refreshRemoteControlStatus(seq);
        return info;
      } catch (error) {
        if (seq !== webAccessIntentSeq) throw error;
        state.webAccess = Object.assign({}, state.webAccess, {
          active: wasActive, web_client_connected: wasActive && !!state.webAccess.web_client_connected, starting: false,
          status: "error", last_error: String(error),
        });
        notify();
        throw error;
      }
    }

    async function stopRemoteControl() {
      var seq = ++webAccessIntentSeq;
      try {
        await invoke("web_access_disable");
      } catch (error) {
        if (seq !== webAccessIntentSeq) return; // 陈旧失败不向调用者抛错：已有更新的用户操作接管状态（审计补充）
        state.webAccess = Object.assign({}, state.webAccess, { status: "error", last_error: String(error), starting: false });
        notify();
        throw error;
      }
      if (seq !== webAccessIntentSeq) return; // 已有更新的用户操作，不写反状态
      state.webAccess = Object.assign({}, state.webAccess, {
        active: false, endpoint_id: null, url: null, qr_data_url: null,
        web_client_connected: false, host_workspace_authorized: false, status: "stopped", starting: false,
      });
      notify();
    }

    async function refreshRemoteControlQr() {
      var seq = ++webAccessIntentSeq;
      try {
        var info = await invoke("web_access_rotate");
        if (seq !== webAccessIntentSeq) return info;
        state.webAccess = Object.assign({}, state.webAccess, info || {}, {
          active: true, web_client_connected: false, last_error: null, starting: false,
        });
        notify();
        await refreshRemoteControlStatus(seq);
        return info;
      } catch (error) {
        if (seq !== webAccessIntentSeq) throw error;
        state.webAccess = Object.assign({}, state.webAccess, { status: "error", last_error: String(error), starting: false });
        notify();
        throw error;
      }
    }

    async function getWebRelaySettings() {
      return invoke("web_access_relay_settings");
    }

    async function setWebRelayAddress(address) {
      var seq = ++webAccessIntentSeq;
      var info = await invoke("web_access_set_relay", { address: address });
      if (seq !== webAccessIntentSeq) return info;
      await refreshRemoteControlStatus(seq);
      return info;
    }

    async function resetWebRelayAddress() {
      var seq = ++webAccessIntentSeq;
      var info = await invoke("web_access_reset_relay");
      if (seq !== webAccessIntentSeq) return info;
      await refreshRemoteControlStatus(seq);
      return info;
    }

    return {
      startDesktopProxy: startDesktopProxy,
      refreshRemoteControlStatus: refreshRemoteControlStatus,
      startRemoteControl: startRemoteControl,
      stopRemoteControl: stopRemoteControl,
      refreshRemoteControlQr: refreshRemoteControlQr,
      getWebRelaySettings: getWebRelaySettings,
      setWebRelayAddress: setWebRelayAddress,
      resetWebRelayAddress: resetWebRelayAddress,
    };
  };
})(window);
