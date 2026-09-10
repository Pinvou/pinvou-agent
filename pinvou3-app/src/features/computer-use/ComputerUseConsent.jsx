import { useEffect, useRef, useState } from 'react';
// (useEffect/useRef below: the consent dialog moves focus to the safe Deny
// button when it opens — a security-critical prompt must not be silent to
// screen readers, which the role/aria-modal attributes on the dialog elements
// also serve.)
import { bridge } from '../../hooks/useBridge.js';
import { computerUseConsentView } from './computer-use-logic.js';

/**
 * Computer-use consent surfaces for the chat view. All visibility derives from
 * computerUseConsentView: with the feature toggle off every surface is hidden.
 */

// Post-completion grace window (ms) for the synchronous single-flight guard in
// useConsentAction: it swallows the trailing click of a double-click, whose
// second press can land before React has committed the disabled state.
const DOUBLE_CLICK_GUARD_MS = 200;

function useConsentAction(copy) {
  const [pendingAction, setPendingAction] = useState(null);
  const [actionError, setActionError] = useState('');
  // Synchronous single-flight (review finding): the disabled attribute only
  // updates one render after the click, so a double-click could fire two
  // concurrent backend calls. This ref is checked inside the event handler,
  // before React commits anything, and also ignores re-clicks within a short
  // window after an action settles.
  const flightRef = useRef({ busy: false, settledAt: 0 });
  const run = (key, action) => {
    const flight = flightRef.current;
    if (flight.busy || Date.now() - flight.settledAt < DOUBLE_CLICK_GUARD_MS) return;
    flight.busy = true;
    setPendingAction(key);
    setActionError('');
    Promise.resolve()
      .then(action)
      .catch((error) => setActionError(copy.actionFailed(String(error && error.message ? error.message : error))))
      .finally(() => {
        flight.busy = false;
        flight.settledAt = Date.now();
        setPendingAction(null);
      });
  };
  return { pendingAction, actionError, run };
}

const dialogButtonBase = 'text-[13px] px-4 py-2 rounded-full font-medium transition-colors disabled:opacity-50';
const dialogPrimaryButton = `${dialogButtonBase} bg-[#0B57D0] text-white hover:bg-[#1967D2] dark:bg-[#A8C7FA] dark:text-[#041E49] dark:hover:bg-[#C2D7FB]`;
const dialogSecondaryButton = `${dialogButtonBase} bg-[#E1E5EA] hover:bg-[#D3D9E0] dark:bg-[#333537] dark:hover:bg-[#444746]`;

/** Persistent, non-dismissible session banner while the agent holds control. */
export function ComputerUseBanner({ slice, copy }) {
  const view = computerUseConsentView(slice);
  const { pendingAction, actionError, run } = useConsentAction(copy);
  if (!view.showBanner) return null;
  // role="alert": the banner mounts while the agent already controls the
  // machine, so it must be announced assertively instead of appearing silently.
  return (
    <div className="mb-2" role="alert" data-testid="computer-use-banner">
      <div className="flex items-center gap-3 px-3 py-2 rounded-2xl text-[12px] bg-[#FCE8E6] text-[#C5221F] dark:bg-[#3A1F1F] dark:text-[#F28B82]">
        <span className="h-2 w-2 shrink-0 rounded-full bg-[#EA4335] animate-pulse" />
        <span className="min-w-0 flex-1">
          <span className="block font-semibold text-[13px]">{copy.bannerTitle}</span>
          <span className="block opacity-80">{copy.bannerNote}</span>
        </span>
        <button
          type="button"
          data-testid="computer-use-stop"
          disabled={!!pendingAction}
          onClick={() => run('stop', () => bridge.computerUse.stop())}
          className="shrink-0 px-3 py-1.5 rounded-full font-semibold bg-[#C5221F] text-white hover:bg-[#A50E0E] disabled:opacity-50"
        >
          {copy.bannerStop}
        </button>
      </div>
      {actionError && <div className="mt-1 px-3 text-[11px] text-[#C5221F] dark:text-[#F28B82]">{actionError}</div>}
    </div>
  );
}

/** Grant dialog (computer_use:grant_required) + per-action confirm dialog. */
export function ComputerUseDialogs({ slice, copy }) {
  const view = computerUseConsentView(slice);
  const { pendingAction, actionError, run } = useConsentAction(copy);
  const grantRequest = view.grantRequest;
  const confirmRequest = view.confirmRequest;
  // Focus the safe (deny) button of whichever dialog is up; effects may read
  // refs, render may not, so the refs are per-dialog and never spread around.
  const grantDenyRef = useRef(null);
  const confirmDenyRef = useRef(null);
  const grantDialogRef = useRef(null);
  const confirmDialogRef = useRef(null);
  const restoreFocusRef = useRef(null);
  const open = !!(grantRequest || confirmRequest);

  useEffect(() => {
    if (!open) return;
    restoreFocusRef.current = document.activeElement;
    return () => {
      // Hand focus back to where the user was before the modal took it, so
      // closing the prompt never strands keyboard users mid-page.
      const previous = restoreFocusRef.current;
      restoreFocusRef.current = null;
      if (previous && typeof previous.focus === 'function') previous.focus();
    };
  }, [open]);

  useEffect(() => {
    const target = grantRequest ? grantDenyRef.current : confirmDenyRef.current;
    if (open && target && typeof target.focus === 'function') target.focus();
  }, [open, grantRequest, confirmRequest]);

  // Esc maps to Deny: the reflexive way out of a machine-control prompt must
  // never equal approval, so Esc performs the conservative action (revoke or
  // per-action deny) instead of a neutral dismiss.
  const denyCurrentRequest = () => {
    if (pendingAction) return;
    if (grantRequest) run('deny', () => bridge.computerUse.revoke(grantRequest.sessionId));
    else if (confirmRequest) run('deny', () => bridge.computerUse.deny(confirmRequest.confirmId));
  };

  // Modal focus trap: while the dialog is up the rest of the page must stay
  // unreachable to keyboard focus, so Tab cycles inside the dialog.
  const handleDialogKeyDown = (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      denyCurrentRequest();
      return;
    }
    if (event.key !== 'Tab') return;
    const dialog = grantRequest ? grantDialogRef.current : confirmDialogRef.current;
    if (!dialog) return;
    const focusables = dialog.querySelectorAll(
      'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    );
    if (focusables.length === 0) {
      event.preventDefault();
      return;
    }
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || !dialog.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !dialog.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  };

  // Listen at document level while a dialog is up: clicking the backdrop can
  // move focus to <body>, which takes the dialog out of the keydown
  // propagation path — Escape (the conservative deny) and the Tab trap must
  // keep working wherever focus went (review finding: after an overlay
  // click, Escape stopped denying and Tab escaped the trap).
  useEffect(() => {
    if (!open) return;
    document.addEventListener('keydown', handleDialogKeyDown);
    return () => document.removeEventListener('keydown', handleDialogKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- handler closes over the request/pending state that these deps track
  }, [open, grantRequest, confirmRequest, pendingAction]);

  if (!grantRequest && !confirmRequest) return null;

  // The per-action confirmation is the more time-sensitive surface: when both
  // are pending (rare — a confirm arriving before the grant is settled), show
  // the grant first; the confirm request stays pending underneath.
  if (grantRequest) {
    return (
      <div data-testid="computer-use-grant-dialog" className="fixed inset-0 z-[1200] flex items-center justify-center p-4 bg-black/45">
        <div
          role="dialog"
          aria-modal="true"
          aria-labelledby="computer-use-grant-title"
          ref={grantDialogRef}
          className="w-full max-w-[440px] rounded-[20px] shadow-2xl p-6 bg-white text-[#1C1C1E] dark:bg-[#1E1F20] dark:text-[#E3E3E3]"
        >
          <h3 id="computer-use-grant-title" className="text-[16px] font-semibold mb-2">{copy.grantTitle}</h3>
          <p className="text-[13px] leading-relaxed opacity-80 mb-4">{copy.grantDesc}</p>
          {actionError && <div className="text-[13px] text-[#EA4335] mb-3">{actionError}</div>}
          <div className="flex items-center justify-end gap-2">
            <button
              type="button"
              ref={grantDenyRef}
              data-testid="computer-use-grant-deny"
              disabled={!!pendingAction}
              onClick={() => run('deny', () => bridge.computerUse.revoke(grantRequest.sessionId))}
              className={dialogSecondaryButton}
            >
              {copy.grantDeny}
            </button>
            <button
              type="button"
              data-testid="computer-use-grant-allow"
              disabled={!!pendingAction}
              onClick={() => run('grant', () => bridge.computerUse.grant(grantRequest.sessionId))}
              className={dialogPrimaryButton}
            >
              {copy.grantAllow}
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div data-testid="computer-use-confirm-dialog" className="fixed inset-0 z-[1200] flex items-center justify-center p-4 bg-black/45">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="computer-use-confirm-title"
        ref={confirmDialogRef}
        className="w-full max-w-[440px] rounded-[20px] shadow-2xl p-6 bg-white text-[#1C1C1E] dark:bg-[#1E1F20] dark:text-[#E3E3E3]"
      >
        <h3 id="computer-use-confirm-title" className="text-[16px] font-semibold mb-2">{copy.confirmTitle}</h3>
        <div className="text-[13px] leading-relaxed mb-4 space-y-1.5">
          <div className="flex gap-2">
            <span className="shrink-0 opacity-60">{copy.confirmActionLabel}</span>
            <span className="min-w-0 break-words font-medium">{confirmRequest.action}</span>
          </div>
          {confirmRequest.element && (
            <div className="flex gap-2">
              <span className="shrink-0 opacity-60">{copy.confirmElementLabel}</span>
              <span className="min-w-0 break-words">{confirmRequest.element}</span>
            </div>
          )}
        </div>
        {actionError && <div className="text-[13px] text-[#EA4335] mb-3">{actionError}</div>}
        <div className="flex items-center justify-end gap-2">
          <button
            type="button"
            ref={confirmDenyRef}
            data-testid="computer-use-confirm-deny"
            disabled={!!pendingAction}
            onClick={() => run('deny', () => bridge.computerUse.deny(confirmRequest.confirmId))}
            className={dialogSecondaryButton}
          >
            {copy.confirmDeny}
          </button>
          <button
            type="button"
            data-testid="computer-use-confirm-once"
            disabled={!!pendingAction}
            onClick={() => run('confirm', () => bridge.computerUse.confirm(confirmRequest.confirmId))}
            className={dialogPrimaryButton}
          >
            {copy.confirmOnce}
          </button>
        </div>
      </div>
    </div>
  );
}
