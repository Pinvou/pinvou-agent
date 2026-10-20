import { useEffect, useState } from 'react';
import { invokeTauri, tauriEvents } from '../platform/tauri/client.js';
import { ensureLanguage, initialSystemLanguage, TAG_TO_LANG } from '../shared/i18n.js';

/**
 * Language of a secondary window that follows the main window's setting:
 * seeded from get_settings (TAG_TO_LANG mapping), then kept in sync with the
 * `ui:language_changed` broadcast. ReaderApp and PetWindow previously inlined
 * the same listener effect (same consolidation pattern as useCopyFlash).
 * The en/ja dictionaries are lazy: a window entry boots on the system language
 * only, so the persisted language must be ensureLanguage()d here — the main
 * window having loaded it does not carry across windows.
 * @param {{ onSettings?: (settings: object) => void, enabled?: boolean }} [opts]
 *   onSettings consumes the main settings further (e.g. the reader's
 *   color-scheme following); enabled=false skips the settings fetch and the
 *   listener entirely (the pet window's defensive non-Tauri guard).
 * @returns {string} the followed UI language
 */
export function useFollowMainWindowLanguage({ onSettings, enabled = true } = {}) {
  const [language, setLanguage] = useState(initialSystemLanguage);
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten = null;
    invokeTauri('get_settings').then((settings) => {
      if (disposed) return;
      const lang = TAG_TO_LANG[settings?.language] || initialSystemLanguage();
      ensureLanguage(lang).then((ok) => { if (ok) setLanguage(lang); }).catch(() => {});
      if (onSettings) onSettings(settings);
    }).catch(() => {});
    tauriEvents.listen('ui:language_changed', (event) => {
      const next = event.payload?.language;
      if (!disposed) ensureLanguage(next).then((ok) => { if (ok) setLanguage(next); }).catch(() => {});
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [enabled, onSettings]);
  return language;
}
