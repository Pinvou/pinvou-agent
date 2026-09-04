import { useEffect, useState } from 'react';
import { systemPrefersDark } from '../shared/color-scheme.js';

/**
 * Track the OS light/dark preference, including runtime changes (not just a
 * mount-time snapshot). While the color-scheme preference is `system`, OS flips
 * must map to the UI theme live, so long-lived windows stay subscribed.
 * Returns false when detection is unavailable (light fallback), matching
 * shared/color-scheme.js.
 */
export function useSystemDarkMode() {
  const [dark, setDark] = useState(systemPrefersDark);

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => setDark(media.matches);
    media.addEventListener?.('change', onChange);
    return () => media.removeEventListener?.('change', onChange);
  }, []);

  return dark;
}
