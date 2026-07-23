/**
 * remote-control feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["remote-control"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
  // ── Remote Control: 当前 session 手机远控 ───────────────────────
  async function refreshRemoteControlStatus() {
    try {
      var status = await invoke("remote_control_status");
      state.remoteControl = Object.assign({}, state.remoteControl, status || {});
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { last_error: String(e) });
    }
    notify();
  }
  async function startRemoteControl(sessionId) {
    state.remoteControl = Object.assign({}, state.remoteControl, { starting: true, last_error: null });
    notify();
    try {
      var info = await invoke("remote_control_start", { sessionId: sessionId || null });
      state.remoteControl = Object.assign({}, state.remoteControl, info || {}, { active: true, pairing: info, starting: false, last_error: null });
      await refreshRemoteControlStatus();
      return info;
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { active: false, starting: false, status: "error", last_error: String(e) });
      notify();
      throw e;
    }
  }
  async function stopRemoteControl() {
    try {
      await invoke("remote_control_stop");
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { status: "error", last_error: String(e) });
      notify();
      throw e;
    }
    state.remoteControl = Object.assign({}, state.remoteControl, { active: false, pairing: null, status: "stopped" });
    notify();
  }
  async function refreshRemoteControlQr(sessionId) {
    try {
      var info = await invoke("remote_control_refresh_qr", { sessionId: sessionId || null });
      state.remoteControl = Object.assign({}, state.remoteControl, info || {}, { active: true, pairing: info, last_error: null });
      await refreshRemoteControlStatus();
      return info;
    } catch (e) {
      state.remoteControl = Object.assign({}, state.remoteControl, { status: "error", last_error: String(e) });
      notify();
      throw e;
    }
  }

    return {
      refreshRemoteControlStatus: refreshRemoteControlStatus,
      startRemoteControl: startRemoteControl,
      stopRemoteControl: stopRemoteControl,
      refreshRemoteControlQr: refreshRemoteControlQr
    };
  };
})(window);
