import { useMediaQuery } from './useMediaQuery.js';

/**
 * Track the OS light/dark preference, including runtime changes (not just a
 * mount-time snapshot). While the color-scheme preference is `system`, OS flips
 * must map to the UI theme live, so long-lived windows stay subscribed.
 * Returns false when detection is unavailable (light fallback), matching
 * shared/color-scheme.js.
 */
export function useSystemDarkMode() {
  return useMediaQuery('(prefers-color-scheme: dark)');
}
