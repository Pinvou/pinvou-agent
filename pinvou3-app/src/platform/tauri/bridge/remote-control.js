/**
 * Persistent Web access administration for the desktop Tauri bridge.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statement would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["remote-control"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const listen = context.listen;
    const bt = context.bt;
    let desktopProxyStarted = false;
    const eventForwarders = {};
    let policyPromise = null;
    const bridgeGeneration = (function () {
      try {
        if (root.crypto && typeof root.crypto.randomUUID === "function") {
          return "webview_" + root.crypto.randomUUID().replaceAll('-', "_"); // safari14-ok: guarded above
        }
      } catch { /* fall through to the fallback below when UUID generation throws */ }
      // eslint-disable-next-line sonarjs/pseudo-random -- non-security use: webview dedup ID; the timestamp prefix already guarantees basic uniqueness
      return "webview_" + Date.now().toString(36) + "_" + Math.random().toString(36).slice(2);
    })();

    function loadAccessPolicy() {
      if (policyPromise) return policyPromise;
      const url = new URL("platform/web/access-policy.json", document.baseURI);
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
      // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor; Object.hasOwn is unavailable, and this call is already the safe form
      return event && Object.prototype.hasOwnProperty.call(event, "payload") ? event.payload : (event || {});
    }

    function respondToWebAccess(requestId, ok, result, error, errorCode, errorCategory) {
      return invoke("web_access_rpc_respond", {
        requestId,
        generation: bridgeGeneration,
        ok: !!ok,
        result: result === undefined ? null : result,
        error: error ? String(error) : null,
        // Structured identity of desktop command errors (e.g. VoiceCommandError
        // {code, category, message}): the message text alone would leave the
        // browser lane's error-code → trilingual copy mapping unreachable.
        errorCode: errorCode || null,
        errorCategory: errorCategory || null,
      }).catch(function (respondError) {
        console.warn("[WebAccess] failed to send RPC response", respondError);
      });
    }

    async function startDesktopProxy() {
      if (desktopProxyStarted || typeof listen !== "function" || typeof fetch !== "function") return;
      desktopProxyStarted = true;
      const policyReady = loadAccessPolicy();

      // Install every allowlisted desktop-side forwarder before the bridge
      // readiness ACK so browser subscriptions cannot miss early events.
      const eventForwardersReady = policyReady.then(function (policy) {
        return Promise.all([...policy.events].map(function (name) {
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

      const rpcListenerReady = listen("web_access:rpc_request", async function (event) {
        const request = eventPayload(event);
        const requestId = request.request_id || request.requestId || request.id;
        const requestGeneration = request.bridge_generation || request.bridgeGeneration;
        if (!requestId || requestGeneration !== bridgeGeneration) return;

        let mayExecute;
        try {
          mayExecute = await invoke("web_access_rpc_begin", {
            requestId,
            generation: bridgeGeneration,
          });
        } catch (error) {
          console.warn("[WebAccess] RPC begin barrier failed", error);
          return;
        }
        if (!mayExecute) return;

        let policy;
        try {
          policy = await policyReady;
        } catch (error) {
          console.error("[WebAccess] policy load failed", error);
          await respondToWebAccess(requestId, false, null, error);
          return;
        }

        const command = String(request.command || "");
        if (!policy.commands.has(command)) {
          await respondToWebAccess(requestId, false, null, bt("remoteCmdNotAllowed")(command));
          return;
        }
        if (command === "__dialog_open") {
          await respondToWebAccess(requestId, false, null, bt("remoteDialogDesktop"));
          return;
        }

        try {
          const result = await invoke(command, request.args || {});
          await respondToWebAccess(requestId, true, result, null);
        } catch (error) {
          // Desktop command errors arrive as structured objects (e.g.
          // VoiceCommandError); keep their stable code/category for the
          // browser's trilingual error mapping instead of only the message.
          const structured = error && typeof error.code === "string";
          const errorCategory = error && typeof error.category === "string" ? error.category : null;
          await respondToWebAccess(
            requestId,
            false,
            null,
            error && error.message ? error.message : error,
            structured ? error.code : null,
            errorCategory
          );
        }
      });

      const subscribeListenerReady = listen("web_access:event_subscribe", async function (event) {
        let policy;
        try {
          policy = await policyReady;
        } catch (error) {
          console.error("[WebAccess] policy load failed", error);
          return;
        }
        const name = String(eventPayload(event).event || "");
        if (!name || !policy.events.has(name)) return;
        await eventForwardersReady;
      });

      // Forwarders remain installed for the lifetime of the authoritative main
      // WebView. Rust filters delivery according to the current Web lease.
      const unsubscribeListenerReady = listen("web_access:event_unsubscribe", function () {});
      // Keep the desktop indicator in sync with the actual browser connection.
      // The access endpoint is intentionally persistent, so `active` only means
      // that the QR/link remains valid; it does not mean a phone is connected.
      const statusListenerReady = listen("web_access:status", function (event) {
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
    let webAccessIntentSeq = 0;
    let webAccessStatusSeq = 0;

    async function refreshRemoteControlStatus(expectedIntentSeq) {
      const intentSeq = expectedIntentSeq === undefined ? webAccessIntentSeq : expectedIntentSeq;
      const statusSeq = ++webAccessStatusSeq;
      try {
        const status = await invoke("web_access_status");
        if (intentSeq !== webAccessIntentSeq || statusSeq !== webAccessStatusSeq) return status;
        state.webAccess = Object.assign({}, state.webAccess, status || {});
      } catch (error) {
        if (intentSeq !== webAccessIntentSeq || statusSeq !== webAccessStatusSeq) return;
        state.webAccess = Object.assign({}, state.webAccess, { last_error: String(error) });
      }
      notify();
    }

    async function startRemoteControl(options) {
      const seq = ++webAccessIntentSeq;
      const wasActive = !!state.webAccess.active;
      state.webAccess = Object.assign({}, state.webAccess, { starting: true, last_error: null });
      notify();
      try {
        const info = await invoke("web_access_enable", {
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
      const seq = ++webAccessIntentSeq;
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
      const seq = ++webAccessIntentSeq;
      try {
        const info = await invoke("web_access_rotate");
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

    // eslint-disable-next-line sonarjs/no-invariant-returns -- echoing info from both branches is an intentional API contract
    async function setWebRelayAddress(address) {
      const seq = ++webAccessIntentSeq;
      const info = await invoke("web_access_set_relay", { address });
      if (seq !== webAccessIntentSeq) return info;
      await refreshRemoteControlStatus(seq);
      return info;
    }

    // eslint-disable-next-line sonarjs/no-invariant-returns -- echoing info from both branches is an intentional API contract
    async function resetWebRelayAddress() {
      const seq = ++webAccessIntentSeq;
      const info = await invoke("web_access_reset_relay");
      if (seq !== webAccessIntentSeq) return info;
      await refreshRemoteControlStatus(seq);
      return info;
    }

    return {
      startDesktopProxy,
      refreshRemoteControlStatus,
      startRemoteControl,
      stopRemoteControl,
      refreshRemoteControlQr,
      getWebRelaySettings,
      setWebRelayAddress,
      resetWebRelayAddress,
    };
  };
})(window);
