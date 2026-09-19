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
      if (!pendingBySession[sid]) {
        pendingBySession[sid] = { grant: false, confirm: null, grantAt: 0, confirmAt: 0 };
      }
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
    // A patch that changes nothing publishes nothing: the 30s
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
    // change. detailKey digests the full request content (English summary,
    // structured action fields, typed-text preview), so any visible change
    // counts and identical re-sends stay no-ops.
    function sameRequest(a, b) {
      if (a === b) return true;
      if (!a || !b) return false;
      return (
        a.sessionId === b.sessionId &&
        a.confirmId === b.confirmId &&
        (a.detailKey || null) === (b.detailKey || null)
      );
    }

    function getStatus(sessionId) {
      return invoke("computer_use_get_status", { sessionId });
    }

    // Server-truth reconciliation: `pending_grant`/`pending_confirm` carry
    // the backend's consent state, so a request the user resolved in ANOTHER
    // window collapses here immediately (its pending is gone server-side),
    // and a request minted anywhere (or a window reloaded mid-request) is
    // reconstructed from the served payload instead of waiting for an event
    // this window may never have received. A status WITHOUT these keys
    // predates the fields: keep the local cache as the source of truth there
    // (background-session resurfacing). Entries stamped AFTER `snapshotAt`
    // arrived while the status round-trip was in flight; their events are
    // newer than this snapshot, so the snapshot never collapses them.
    function reconcileServerTruth(sid, raw, snapshotAt) {
      const serverTruthKnown =
        raw && typeof raw === "object" &&
        ("pending_confirm" in raw || typeof raw.pending_grant === "boolean");
      if (!serverTruthKnown) return;
      const pending = pendingEntry(sid);
      if ("pending_confirm" in raw) {
        const serverConfirm =
          raw.pending_confirm && typeof raw.pending_confirm === "object"
            ? buildConfirmRequest(
                sid,
                String(raw.pending_confirm.confirm_id || ""),
                raw.pending_confirm,
              )
            : null;
        if (serverConfirm && !serverConfirm.confirmId) {
          // Payload without a usable id: leave reconciliation to the events.
        } else if (serverConfirm) {
          if (!pending.confirm || pending.confirm.confirmId !== serverConfirm.confirmId) {
            pending.confirm = serverConfirm;
            pending.confirmAt = Date.now();
          }
        } else if (pending.confirm && (pending.confirmAt || 0) <= snapshotAt) {
          pending.confirm = null;
        }
      }
      if (typeof raw.pending_grant === "boolean") {
        if (raw.pending_grant && !pending.grant) {
          pending.grant = true;
          pending.grantAt = Date.now();
        } else if (
          !raw.pending_grant &&
          pending.grant &&
          (pending.grantAt || 0) <= snapshotAt
        ) {
          pending.grant = false;
        }
      }
    }

    async function refreshStatus(sessionId) {
      // No early return on an empty sid: the settings page polls status at
      // cold start before any session exists, and the backend answers a
      // session-less get_status with just enabled + platform_supported.
      const sid = String(sessionId || state.activeSessionId || "");
      // Session switch (ChatView's refresh effect): the live slice may still
      // carry the previous session's requests, and during the IPC round-trip
      // its grant dialog would stay clickable. Drop them synchronously,
      // before the await; the per-session pending map keeps them, so
      // switching back still resurfaces the requests. Only when the refresh
      // is FOR the active session: state_changed events carry their own
      // session_id, so a background refresh for another session must not
      // collapse the active session's live dialog (the switch case always
      // has sid === activeSessionId because ChatView passes the id it just
      // set).
      const current = state.computerUse;
      const liveRequestSession = current && (
        (current.grantRequest && current.grantRequest.sessionId) ||
        (current.confirmRequest && current.confirmRequest.sessionId)
      );
      if (
        sid && sid === String(state.activeSessionId || "") &&
        liveRequestSession && String(liveRequestSession) !== sid
      ) {
        clearSessionRequests(liveRequestSession);
      }
      // Same synchronous switch guard for the banner: a stale `granted` left
      // up during the IPC round-trip showed the previous session's control
      // banner over the session the user just opened.
      if (sid && current && current.sessionId && String(current.sessionId) !== sid) {
        publish(sid, { granted: false });
      }
      const seq = ++statusRequestSeq;
      // Snapshot wall-clock taken BEFORE the IPC round-trip: a dialog that
      // arrives while the request is in flight (its event still processing)
      // is newer than this snapshot and must not be collapsed by it.
      const snapshotAt = Date.now();
      let raw;
      try {
        raw = await invoke("computer_use_get_status", { sessionId: sid });
      } catch {
        return null;
      }
      if (seq !== statusRequestSeq) return raw;
      if (!sid) {
        // Session-less answer: no per-session fields exist, but the
        // enabled/platform_supported bits still drive the settings toggle
        // (its platformSupported grey-out). A null raw means the backend
        // predates the session-less form — leave the slice untouched.
        if (raw) {
          state.computerUse = Object.assign({}, state.computerUse, {
            enabled: !!raw.enabled,
            platformSupported: !!(raw.platform_supported || raw.platformSupported),
          });
          notify();
        }
        return raw;
      }
      reconcileServerTruth(sid, raw, snapshotAt);
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
      // Backend revoke wipes the session's grant AND its pending confirmations
      // AND tokens, so a same-session confirm dialog must collapse too — it
      // would otherwise linger as a dead modal after the explicit revoke.
      const pending = pendingEntry(sid);
      if (pending) {
        pending.grant = false;
        pending.confirm = null;
      }
      publish(sid, { granted: false, grantRequest: null, confirmRequest: null });
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
      // phantom dialogs from stale entries.
      clearAllPending();
      notify();
    }

    // Targeted clear shared by confirm() and deny() — success AND "unknown or
    // expired" cleanup: only the dialog/pending entry for `confirmId` goes
    // away. Two independent concerns:
    // (a) The pending-map cleanup for the captured `sid` runs whenever the
    // map's own entry still holds THIS confirmId — even when a different
    // session's dialog was published mid-IPC. The old published-slice
    // early-return skipped it, so switching back resurfaced a phantom
    // dialog. A same-session newer replacement still survives: the map only
    // clears on an id match.
    // (b) The published-slice clearing stays gated on the published request
    // for THAT sid matching the confirmId, so a newer replacement's dialog
    // is never closed by this decision.
    function clearConfirmIfCurrent(confirmId, sid) {
      const target = String(sid || state.activeSessionId);
      const pending = pendingBySession[target];
      if (
        pending && pending.confirm &&
        String(pending.confirm.confirmId) === String(confirmId)
      ) {
        pending.confirm = null;
      }
      const request = state.computerUse && state.computerUse.confirmRequest;
      const published = !!(
        request &&
        String(request.sessionId || "") === target &&
        String(request.confirmId) === String(confirmId)
      );
      if (published) publish(target, { confirmRequest: null });
    }

    // Backend TTL: a confirmation older than its five-minute window is
    // rejected as "unknown or expired". Both buttons then keep failing with
    // no way out of the full-screen modal, so the caller cleans up locally
    // and rethrows — the modal closes and the failure still surfaces through
    // the UI's actionError.
    function isExpiredConfirmError(error) {
      return /unknown or expired/i.test(String((error && error.message) || error));
    }

    // Backend error strings the commands surface verbatim. Map the stable
    // known ones onto the settings copy (trilingual, keyed off the persisted
    // UI language) so the settings page never shows raw backend English.
    // The copy table lives in shared/bridge-messages.js (the sanctioned
    // bridge-layer i18n mechanism, loaded before this feature); exact
    // equality only — unrecognized messages resolve to null and pass
    // through untouched, and so does every error when the shared module is
    // absent (partial loads, the behavior-test VM).
    function localizeKnownError(error) {
      const message = String((error && error.message) || error);
      const messages = window.PinvouBridgeMessages;
      if (!messages || typeof messages.computerUseKnownErrorText !== "function") return error;
      const localized = messages.computerUseKnownErrorText(message, state.settings && state.settings.language);
      return localized == null ? error : new Error(localized);
    }

    async function confirm(confirmId) {
      // Attribute the decision to the session the dialog belongs to, captured
      // BEFORE the IPC round-trip (same rule as deny()): a session switch
      // mid-flight must not redirect the cleanup.
      const request = state.computerUse && state.computerUse.confirmRequest;
      const sid = (request && request.sessionId) || state.activeSessionId;
      try {
        await invoke("computer_use_confirm", { confirmId });
      } catch (error) {
        if (!isExpiredConfirmError(error)) throw error;
        // Expired: the backend already dropped the request, so closing
        // locally is the only way out of the dead-end modal. Targeted clear
        // (same rule as below): a newer replacement request must survive.
        clearConfirmIfCurrent(confirmId, sid);
        throw error;
      }
      // Clear only the dialog that was confirmed: a new request landing
      // during the IPC round-trip keeps its own dialog and pending entry
      // instead of being wiped by this decision.
      clearConfirmIfCurrent(confirmId, sid);
    }

    // Explicit backend deny: clears the pending confirmation so the model's
    // retry re-screens and raises a fresh confirmation dialog instead of
    // waiting out the backend TTL.
    async function deny(confirmId) {
      // Attribute the denial to the session the user is actually looking at,
      // captured BEFORE the IPC round-trip: a new confirm request landing
      // mid-flight must not be wiped.
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
        throw localizeKnownError(error);
      }
      if (target) {
        // Re-enabling must clear a sticky stop: the toggle command only flips
        // `enabled` on the backend, so without re-reading the authoritative
        // status a latched `stopped` kept every later consent dialog
        // collapsed until the next session switch. Runs before the macOS
        // permission flow, which can block on an OS dialog.
        await refreshStatus(state.activeSessionId);
      } else {
        // Disable wipes the backend state globally, so the pending map must
        // go too — same phantom-dialog hazard as stop().
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

    // Structured confirm payload (backend contract): the event carries the
    // action name plus optional button/click_count/point/end_point/
    // text_length/text_preview/text_preview_truncated (and chord/
    // chord_masked_chars/hold_ms for the key/hold shapes — on secure targets
    // the backend strips the character keys from `chord` and sends their
    // count in `chord_masked_chars` instead; scroll never blocks and has no
    // structured fields), while the original English summary string is kept
    // in `summary` as the renderer's fallback. Legacy payloads carried the
    // summary itself in `action` and no `summary` key — the presence of
    // `summary` distinguishes the two generations.
    function buildConfirmRequest(sid, confirmId, payload) {
      const structured = typeof payload.summary === "string";
      const num = (value) => (typeof value === "number" && Number.isFinite(value) ? value : null);
      const point = (value) => (
        value && typeof value.x === "number" && typeof value.y === "number"
          ? { x: value.x, y: value.y }
          : null
      );
      // snake_case wins over camelCase (the backend emits snake_case; the
      // camel spelling accepts hand-rolled/test payloads).
      const pickField = (snake, camel) => {
        const primary = payload[snake];
        return primary === undefined ? payload[camel] : primary;
      };
      const request = {
        sessionId: sid,
        confirmId,
        summary: structured ? payload.summary : String(payload.action || ""),
        element: String(payload.element || ""),
        actionName: structured ? String(payload.action || "") : null,
        button: structured && typeof payload.button === "string" ? payload.button : null,
        clickCount: structured ? num(pickField("click_count", "clickCount")) : null,
        point: structured ? point(payload.point) : null,
        endPoint: structured ? point(pickField("end_point", "endPoint")) : null,
        textLength: structured ? num(pickField("text_length", "textLength")) : null,
        textPreview: structured && typeof payload.text_preview === "string" ? payload.text_preview : null,
        textPreviewTruncated: structured && !!payload.text_preview_truncated,
        chord: structured && typeof payload.chord === "string" ? payload.chord : null,
        chordMaskedChars: structured ? num(pickField("chord_masked_chars", "chordMaskedChars")) : null,
        holdMs: structured ? num(pickField("hold_ms", "holdMs")) : null,
      };
      // Full typed-text preview (backend contract): rides along with every
      // non-password Type action up to 4096 chars, short texts included;
      // pass it through untouched and keep old payloads free of the key.
      const typePreviewFull = payload.type_preview_full || payload.typePreviewFull;
      if (typeof typePreviewFull === "string" && typePreviewFull) {
        request.typePreviewFull = typePreviewFull;
      }
      // Value digest for sameRequest: any visible content change must
      // re-render the dialog, identical re-sends must stay no-ops.
      request.detailKey = JSON.stringify([
        request.summary, request.actionName, request.button, request.clickCount,
        request.point, request.endPoint, request.textLength, request.textPreview,
        request.textPreviewTruncated, request.chord, request.holdMs,
        request.typePreviewFull || null,
        request.element,
      ]);
      return request;
    }

    if (typeof listen === "function") {
      listen("computer_use:grant_required", function (event) {
        const payload = (event && event.payload) || {};
        const sid = payload.session_id || payload.sessionId;
        if (!sid) return;
        const pending = pendingEntry(sid);
        pending.grant = true;
        pending.grantAt = Date.now();
        // A live per-action confirmation must survive a grant request: the
        // renderer shows the grant dialog first when both are pending, and
        // wiping the confirm here left no dialog after Allow.
        // Feature toggle off: stay inert (no dialog), the record above still
        // lets a later enable + refresh resurface the request. The "later
        // enable" may happen in ANOTHER window (settings live in the main
        // window; detached windows keep their own slice), and nothing tells
        // this window — the reconciler skips polling while the slice says
        // disabled, so a detached session's grant dialog would never surface.
        // One authoritative re-read bridges the gap: if the feature is on
        // now, refreshStatus republishes the recorded pending; if not, it is
        // a cheap no-op. Optional chain: the slice can be undefined in a
        // freshly opened detached window whose mount refresh has not landed
        // yet — a TypeError here would skip both the re-read and publish.
        if (!state.computerUse?.enabled) {
          void refreshStatus(sid).catch(() => {});
          return;
        }
        // The backend never emits this event while stopped, so a `stopped`
        // flag still latched here is frontend residue (e.g. from a stop that
        // predates a re-enable); clearing it keeps the dialog reachable even
        // if the refresh below has not landed yet.
        publish(sid, { stopped: false, grantRequest: { sessionId: sid } });
      });
      listen("computer_use:confirm_required", function (event) {
        const payload = (event && event.payload) || {};
        const sid = payload.session_id || payload.sessionId;
        const confirmId = payload.confirm_id || payload.confirmId;
        if (!sid || !confirmId) return;
        const pending = pendingEntry(sid);
        pending.confirm = buildConfirmRequest(sid, confirmId, payload);
        pending.confirmAt = Date.now();
        if (!state.computerUse?.enabled) {
          // Same other-window-enable gap as the grant branch above: re-read
          // once so a detached window's confirm dialog can resurface. Same
          // optional-chain reason as the grant branch: the slice may not
          // exist yet.
          void refreshStatus(sid).catch(() => {});
          return;
        }
        publish(sid, { confirmRequest: pending.confirm });
      });
      listen("computer_use:state_changed", function (event) {
        // Consent state changed somewhere (grant/revoke/stop/confirm/deny/
        // master toggle): re-read the authoritative status so a request the
        // user resolved in ANOTHER window collapses here immediately instead
        // of surviving as a phantom dialog until the next reconciler tick.
        const payload = (event && event.payload) || {};
        const sid = payload.session_id || payload.sessionId || state.activeSessionId;
        if (!sid) return;
        void refreshStatus(sid).catch(() => {});
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
      setEnabled,
      requestPermissions,
    };
  };
})(window);
