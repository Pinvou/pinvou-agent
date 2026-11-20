// Window-level Escape-to-cancel for the shared dialog shells (ModalShell's
// confirm dialogs and CodexAcpView's branch-switch dialog). The guard is
// load-bearing twice: IME composition (the Escape that cancels candidate words
// must not close anything — shared/ime-guard.mjs has the macOS WKWebView
// details) and busy (an in-flight confirmation cannot be dismissed from the
// keyboard). Stacked dialogs each run their own listener by design; handlers
// target their own layer and must stay idempotent (see ProviderFormModal's
// layered handler — the confirm layer closes first, so a co-open stack always
// loses exactly one layer per press). Behaviorally pinned in
// tests/use_dialog_escape_key.test.mjs; the shell wiring in
// tests/settings_window_confirm.test.mjs.
import { useEffect } from 'react';
import { isImeComposing } from '../shared/ime-guard.mjs';

export function useDialogEscapeKey(busy, onCancel) {
  useEffect(() => {
    const onKey = (event) => {
      if (event.key === 'Escape' && !busy && !isImeComposing(event)) {
        event.preventDefault();
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);
}
