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

    async function refreshRemoteControlStatus() {
      try {
        var status = await invoke("web_access_status");
        state.webAccess = Object.assign({}, state.webAccess, status || {});
      } catch (error) {
        state.webAccess = Object.assign({}, state.webAccess, { last_error: String(error) });
      }
      notify();
    }

    async function startRemoteControl() {
      state.webAccess = Object.assign({}, state.webAccess, { starting: true, last_error: null });
      notify();
      try {
        var info = await invoke("web_access_enable");
        state.webAccess = Object.assign({}, state.webAccess, info || {}, {
          active: true, starting: false, last_error: null,
        });
        await refreshRemoteControlStatus();
        return info;
      } catch (error) {
        state.webAccess = Object.assign({}, state.webAccess, {
          active: false, starting: false, status: "error", last_error: String(error),
        });
        notify();
        throw error;
      }
    }

    async function stopRemoteControl() {
      try {
        await invoke("web_access_disable");
      } catch (error) {
        state.webAccess = Object.assign({}, state.webAccess, { status: "error", last_error: String(error) });
        notify();
        throw error;
      }
      state.webAccess = Object.assign({}, state.webAccess, {
        active: false, endpoint_id: null, url: null, qr_data_url: null, status: "stopped",
      });
      notify();
    }

    async function refreshRemoteControlQr() {
      try {
        var info = await invoke("web_access_rotate");
        state.webAccess = Object.assign({}, state.webAccess, info || {}, { active: true, last_error: null });
        await refreshRemoteControlStatus();
        return info;
      } catch (error) {
        state.webAccess = Object.assign({}, state.webAccess, { status: "error", last_error: String(error) });
        notify();
        throw error;
      }
    }

    async function getWebRelaySettings() {
      return invoke("web_access_relay_settings");
    }

    async function setWebRelayAddress(address) {
      var info = await invoke("web_access_set_relay", { address: address });
      await refreshRemoteControlStatus();
      return info;
    }

    async function resetWebRelayAddress() {
      var info = await invoke("web_access_reset_relay");
      await refreshRemoteControlStatus();
      return info;
    }

    return {
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
