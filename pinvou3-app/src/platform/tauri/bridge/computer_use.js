/**
 * computer_use feature for the Tauri bridge.
 *
 * Wraps the desktop-only computer-use commands (screen capture + mouse/keyboard
 * control consent) and mirrors the backend status into state.computerUse for
 * the active session. These commands drive the local machine and must stay
 * unreachable from the remote web client: they are deliberately absent from
 * platform/web/access-policy.json (allowlist) and the web bridge only exposes
 * rejecting stubs.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["computer_use"] = function (context) {
    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    const listen = context.listen;

    // Pending consent requests tracked per session: computer_use_get_status
    // carries no pending-request bit, so a grant/confirm that arrives while its
    // session is in the background must be re-applied when the user switches
    // back (refreshStatus merges this map into the published slice).
    const pendingBySession = Object.create(null);
    // Latest refresh wins: a slow get_status for a session the user already
    // navigated away from must not overwrite a newer snapshot.
    let statusRequestSeq = 0;
    // macOS permission prompts (screen recording / accessibility) fire once per
    // app run, on the first enable; elsewhere the command is a no-op.
    let permissionsRequested = false;

    function pendingEntry(sessionId) {
      const sid = String(sessionId || "");
      if (!sid) return null;
      if (!pendingBySession[sid]) pendingBySession[sid] = { grant: false, confirm: null };
      return pendingBySession[sid];
    }

    // Only the active session owns the public slice: the banner/dialogs in the
    // chat UI describe what the user is looking at, never a background session.
    function publish(sessionId, patch) {
      if (!sessionId || sessionId !== state.activeSessionId) return;
      state.computerUse = Object.assign({}, state.computerUse, patch);
      notify();
    }

    function getStatus(sessionId) {
      return invoke("computer_use_get_status", { sessionId });
    }

    async function refreshStatus(sessionId) {
      const sid = sessionId || state.activeSessionId;
      if (!sid) return null;
      const seq = ++statusRequestSeq;
      let raw;
      try {
        raw = await invoke("computer_use_get_status", { sessionId: sid });
      } catch {
        return null;
      }
      if (seq !== statusRequestSeq) return raw;
      const pending = pendingBySession[sid] || null;
      publish(sid, {
        sessionId: sid,
        enabled: !!(raw && raw.enabled),
        granted: !!(raw && raw.granted),
        stopped: !!(raw && raw.stopped),
        grantRequest: pending && pending.grant ? { sessionId: sid } : null,
        confirmRequest: pending && pending.confirm ? pending.confirm : null,
      });
      return raw;
    }

    async function grant(sessionId) {
      const sid = sessionId || state.activeSessionId;
      await invoke("computer_use_grant", { sessionId: sid });
      const pending = pendingEntry(sid);
      if (pending) pending.grant = false;
      publish(sid, { granted: true, stopped: false, grantRequest: null });
    }

    async function revoke(sessionId) {
      const sid = sessionId || state.activeSessionId;
      await invoke("computer_use_revoke", { sessionId: sid });
      const pending = pendingEntry(sid);
      if (pending) pending.grant = false;
      publish(sid, { granted: false, grantRequest: null });
    }

    async function stop() {
      const sid = state.activeSessionId;
      // Stop is an escape hatch: the banner must clear immediately, not after
      // the IPC round-trip. A failure re-reads the authoritative status.
      publish(sid, { granted: false, stopped: true });
      try {
        await invoke("computer_use_stop");
      } catch (error) {
        await refreshStatus(sid);
        throw error;
      }
    }

    function clearPendingConfirm() {
      const request = state.computerUse && state.computerUse.confirmRequest;
      const sid = request && request.sessionId;
      const pending = sid && pendingBySession[sid];
      if (pending) pending.confirm = null;
      return sid || state.activeSessionId;
    }

    async function confirm(confirmId) {
      await invoke("computer_use_confirm", { confirmId });
      publish(clearPendingConfirm(), { confirmRequest: null });
    }

    // The contract has no backend deny for confirm_required: denying only
    // dismisses the dialog locally and the backend treats the unanswered
    // confirmation as denied on its own timeout path.
    function dismissConfirm() {
      const sid = clearPendingConfirm();
      if (!sid || sid !== state.activeSessionId) return;
      state.computerUse = Object.assign({}, state.computerUse, { confirmRequest: null });
      notify();
    }

    async function setEnabled(enabled) {
      const target = !!enabled;
      const previous = !!state.computerUse.enabled;
      if (target === previous && !target) return;
      state.computerUse = Object.assign({}, state.computerUse, { enabled: target });
      notify();
      try {
        await invoke("computer_use_set_enabled", { enabled: target });
      } catch (error) {
        state.computerUse = Object.assign({}, state.computerUse, { enabled: previous });
        notify();
        throw error;
      }
      if (target && !permissionsRequested) {
        permissionsRequested = true;
        // macOS triggers the OS permission flows here; a rejection must not
        // roll back the toggle the backend already accepted.
        await invoke("computer_use_request_permissions").catch(function () {});
      }
    }

    function requestPermissions() {
      return invoke("computer_use_request_permissions");
    }

    if (typeof listen === "function") {
      listen("computer_use:grant_required", function (event) {
        const payload = (event && event.payload) || {};
        const sid = payload.session_id || payload.sessionId;
        if (!sid) return;
        const pending = pendingEntry(sid);
        pending.grant = true;
        pending.confirm = null;
        // Feature toggle off: stay inert (no dialog), the record above still
        // lets a later enable + refresh resurface the request.
        if (!state.computerUse.enabled) return;
        publish(sid, { grantRequest: { sessionId: sid }, confirmRequest: null });
      });
      listen("computer_use:confirm_required", function (event) {
        const payload = (event && event.payload) || {};
        const sid = payload.session_id || payload.sessionId;
        const confirmId = payload.confirm_id || payload.confirmId;
        if (!sid || !confirmId) return;
        const pending = pendingEntry(sid);
        pending.confirm = {
          sessionId: sid,
          action: String(payload.action || ""),
          element: String(payload.element || ""),
          confirmId,
        };
        if (!state.computerUse.enabled) return;
        publish(sid, { confirmRequest: pending.confirm });
      });
    }

    return {
      getStatus,
      refreshStatus,
      grant,
      revoke,
      stop,
      confirm,
      dismissConfirm,
      setEnabled,
      requestPermissions,
    };
  };
})(window);
