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
    // Per-session timestamp of the last explicit user denial (grant revoke or
    // confirm deny). Repeated blocked attempts re-emit events; re-opening the
    // blocking modal on each one is a consent-fatigue vector, so inside
    // DENY_SUPPRESSION_MS the request stays pending but no dialog is shown.
    const deniedAtBySession = Object.create(null);
    // Mirrors DENY_SUPPRESSION_MS in features/computer-use/computer-use-logic.js
    // (classic scripts cannot import it; the logic test pins both copies).
    const DENY_SUPPRESSION_MS = 30000;

    function markDenied(sessionId) {
      const sid = String(sessionId || "");
      if (sid) deniedAtBySession[sid] = Date.now();
    }

    function clearDenial(sessionId) {
      const sid = String(sessionId || "");
      if (sid) delete deniedAtBySession[sid];
    }

    function suppressedByDenial(sessionId) {
      const at = deniedAtBySession[String(sessionId || "")];
      return typeof at === "number" && Date.now() - at < DENY_SUPPRESSION_MS;
    }

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
        platformSupported: !!(raw && (raw.platform_supported || raw.platformSupported)),
        grantRequest: pending && pending.grant ? { sessionId: sid } : null,
        confirmRequest: pending && pending.confirm ? pending.confirm : null,
      });
      return raw;
    }

    async function grant(sessionId) {
      const sid = sessionId || state.activeSessionId;
      await invoke("computer_use_grant", { sessionId: sid });
      clearDenial(sid);
      const pending = pendingEntry(sid);
      if (pending) pending.grant = false;
      publish(sid, { granted: true, stopped: false, grantRequest: null });
    }

    async function revoke(sessionId) {
      const sid = sessionId || state.activeSessionId;
      await invoke("computer_use_revoke", { sessionId: sid });
      // An explicit deny starts the dialog cooldown for this session.
      markDenied(sid);
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
      const sid = clearPendingConfirm();
      clearDenial(sid);
      publish(sid, { confirmRequest: null });
    }

    // Explicit backend deny: clears the pending confirmation and records the
    // decision, so the model's retry gets a definite "user denied" instead of
    // waiting out the backend TTL (review finding).
    async function deny(confirmId) {
      await invoke("computer_use_deny", { confirmId });
      const request = state.computerUse && state.computerUse.confirmRequest;
      const sid = (request && request.sessionId) || state.activeSessionId;
      markDenied(sid);
      clearPendingConfirm();
      publish(sid, { confirmRequest: null });
    }

    // Kept for compatibility: dismisses locally without telling the backend
    // (the backend then expires the pending on its own TTL).
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
        // Fresh user denial: the request stays pending but the blocking modal
        // must not re-open on every retry (consent-fatigue guard).
        if (suppressedByDenial(sid)) return;
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
        if (suppressedByDenial(sid)) return;
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
      deny,
      dismissConfirm,
      setEnabled,
      requestPermissions,
    };
  };
})(window);
