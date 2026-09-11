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

    // Drops every per-session pending entry. Backend stop_all and disable are
    // global: they wipe all grants/pendings/tokens, so keeping the map made a
    // later re-enable republish dialogs for requests that no longer exist.
    function clearAllPending() {
      for (const key of Object.keys(pendingBySession)) delete pendingBySession[key];
    }

    // Drops requests that belong to `sessionId` from the published slice.
    // pendingBySession is intentionally untouched: switching back to the
    // session must resurface them via refreshStatus. Pure state operation
    // (no command traffic); refreshStatus calls this synchronously when it
    // detects a session switch, and the behavior tests drive it directly.
    function clearSessionRequests(sessionId) {
      const sid = String(sessionId || "");
      const current = state.computerUse;
      if (!sid || !current) return;
      const matches = (request) => !!(request && String(request.sessionId || "") === sid);
      if (!matches(current.grantRequest) && !matches(current.confirmRequest)) return;
      publish(state.activeSessionId, {
        grantRequest: matches(current.grantRequest) ? null : (current.grantRequest || null),
        confirmRequest: matches(current.confirmRequest) ? null : (current.confirmRequest || null),
      });
    }

    // Only the active session owns the public slice: the banner/dialogs in the
    // chat UI describe what the user is looking at, never a background session.
    // A patch that changes nothing publishes nothing (review finding): the 30s
    // reconciler re-reads authoritative status for the app's lifetime, and
    // assigning a fresh slice object + notifying on every tick re-rendered the
    // whole UI every 30s even when nothing changed.
    function publish(sessionId, patch) {
      if (!sessionId || sessionId !== state.activeSessionId) return;
      const current = state.computerUse;
      if (current) {
        const changed = Object.keys(patch).some((key) => {
          const before = current[key];
          const after = patch[key];
          if (key === "grantRequest" || key === "confirmRequest") return !sameRequest(before, after);
          return !Object.is(before, after);
        });
        if (!changed) return;
      }
      state.computerUse = Object.assign({}, current, patch);
      notify();
    }

    // Request objects are re-created on every refreshStatus from the per-session
    // pending map; compare by value so an unchanged dialog does not count as a
    // change.
    function sameRequest(a, b) {
      if (a === b) return true;
      if (!a || !b) return false;
      return (
        a.sessionId === b.sessionId &&
        a.confirmId === b.confirmId &&
        a.action === b.action &&
        a.element === b.element
      );
    }

    function getStatus(sessionId) {
      return invoke("computer_use_get_status", { sessionId });
    }

    async function refreshStatus(sessionId) {
      const sid = sessionId || state.activeSessionId;
      if (!sid) return null;
      // Session switch (ChatView's refresh effect): the live slice may still
      // carry the previous session's requests, and during the IPC round-trip
      // its grant dialog would stay clickable (review finding). Drop them
      // synchronously, before the await; the per-session pending map keeps
      // them, so switching back still resurfaces the requests.
      const current = state.computerUse;
      const liveRequestSession = current && (
        (current.grantRequest && current.grantRequest.sessionId) ||
        (current.confirmRequest && current.confirmRequest.sessionId)
      );
      if (liveRequestSession && String(liveRequestSession) !== String(sid)) {
        clearSessionRequests(liveRequestSession);
      }
      // Same synchronous switch guard for the banner: a stale `granted` left
      // up during the IPC round-trip showed the previous session's control
      // banner over the session the user just opened (review finding).
      if (current && current.sessionId && String(current.sessionId) !== String(sid)) {
        publish(sid, { granted: false });
      }
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
      // Backend stop_all is global: every grant/confirm/token is gone, so the
      // pending map must be dropped too or a later re-enable republished
      // phantom dialogs from stale entries (review finding).
      clearAllPending();
      notify();
    }

    function clearPendingConfirm() {
      const request = state.computerUse && state.computerUse.confirmRequest;
      const sid = request && request.sessionId;
      const pending = sid && pendingBySession[sid];
      if (pending) pending.confirm = null;
      return sid || state.activeSessionId;
    }

    // Targeted clear shared by confirm() and deny() — success AND "unknown or
    // expired" cleanup: only the dialog/pending entry for `confirmId` goes
    // away. A NEWER request that landed during the IPC round-trip keeps its
    // own dialog and pending entry instead of being wiped by this decision
    // (review finding): closing a dead prompt must not close a live
    // replacement that already arrived.
    function clearConfirmIfCurrent(confirmId, fallbackSid) {
      const request = state.computerUse && state.computerUse.confirmRequest;
      const sid = (request && request.sessionId) || fallbackSid || state.activeSessionId;
      if (request && String(request.confirmId) !== String(confirmId)) return;
      const pending = pendingBySession[sid];
      if (pending) pending.confirm = null;
      publish(sid, { confirmRequest: null });
    }

    // Backend TTL: a confirmation older than its five-minute window is
    // rejected as "unknown or expired". Both buttons then keep failing with
    // no way out of the full-screen modal (review finding), so the caller
    // cleans up locally and rethrows — the modal closes and the failure
    // still surfaces through the UI's actionError.
    function isExpiredConfirmError(error) {
      return /unknown or expired/i.test(String((error && error.message) || error));
    }

    async function confirm(confirmId) {
      try {
        await invoke("computer_use_confirm", { confirmId });
      } catch (error) {
        if (!isExpiredConfirmError(error)) throw error;
        // Expired: the backend already dropped the request, so closing
        // locally is the only way out of the dead-end modal. Targeted clear
        // (same rule as below): a newer replacement request must survive.
        clearConfirmIfCurrent(confirmId, state.activeSessionId);
        throw error;
      }
      // Clear only the dialog that was confirmed (review finding): a new
      // request landing during the IPC round-trip keeps its own dialog and
      // pending entry instead of being wiped by this decision.
      clearConfirmIfCurrent(confirmId, state.activeSessionId);
    }

    // Explicit backend deny: clears the pending confirmation, so the model's
    // retry gets a definite "user denied" instead of waiting out the backend
    // TTL (review finding).
    async function deny(confirmId) {
      // Attribute the denial to the session the user is actually looking at,
      // captured BEFORE the IPC round-trip (review finding): a new confirm
      // request landing mid-flight must not be wiped.
      const request = state.computerUse && state.computerUse.confirmRequest;
      const sid = (request && request.sessionId) || state.activeSessionId;
      try {
        await invoke("computer_use_deny", { confirmId });
      } catch (error) {
        if (!isExpiredConfirmError(error)) throw error;
        // Same targeted clear as the success path below: the expiry cleanup
        // must not close a newer replacement request's dialog.
        clearConfirmIfCurrent(confirmId, sid);
        throw error;
      }
      // Same targeted clear as confirm(): only the denied dialog goes away.
      clearConfirmIfCurrent(confirmId, sid);
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
      if (target) {
        // Re-enabling must clear a sticky stop: the toggle command only flips
        // `enabled` on the backend, so without re-reading the authoritative
        // status a latched `stopped` kept every later consent dialog
        // collapsed until the next session switch (review finding). Runs
        // before the macOS permission flow, which can block on an OS dialog.
        await refreshStatus(state.activeSessionId);
      } else {
        // Disable wipes the backend state globally, so the pending map must
        // go too — same phantom-dialog hazard as stop() (review finding).
        clearAllPending();
        notify();
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
        // A live per-action confirmation must survive: a grant that
        // idle-expired mid-run re-arms the grant gate without killing the
        // backend's pending confirm, so wiping it here left no dialog after
        // Allow (review finding). The renderer shows the grant dialog first
        // when both are pending.
        // Feature toggle off: stay inert (no dialog), the record above still
        // lets a later enable + refresh resurface the request.
        if (!state.computerUse.enabled) return;
        // The backend never emits this event while stopped, so a `stopped`
        // flag still latched here is frontend residue (e.g. from a stop that
        // predates a re-enable); clearing it keeps the dialog reachable even
        // if the refresh below has not landed yet (review finding).
        publish(sid, { stopped: false, grantRequest: { sessionId: sid } });
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
        // Optional full typed-text preview (backend contract): present only
        // for non-password Type actions longer than the preview; pass it
        // through untouched and keep old payloads free of the key.
        const typePreviewFull = payload.type_preview_full || payload.typePreviewFull;
        if (typeof typePreviewFull === "string" && typePreviewFull) {
          pending.confirm.typePreviewFull = typePreviewFull;
        }
        if (!state.computerUse.enabled) return;
        publish(sid, { confirmRequest: pending.confirm });
      });
    }

    return {
      getStatus,
      refreshStatus,
      clearSessionRequests,
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
