/**
 * Pure logic for the color-scheme preference, shared by the main window,
 * detached windows, and the reader.
 *
 * Product rules:
 * - Fresh installs (no saved preference at all) follow the OS light/dark setting;
 * - When the system preference cannot be determined (matchMedia unavailable or
 *   failing), fall back to light;
 * - Once the user picks explicitly, all of system/light/dark persist, and
 *   light/dark stop following the system.
 *
 * Live system-change subscription lives in hooks/useSystemDarkMode.js.
 * Persistence goes through the `color_scheme` field of settings.json on
 * desktop (prefs.rs) and browser localStorage on web.
 */

/** Web-only localStorage key; desktop does not use it (settings.json instead). */
export const COLOR_SCHEME_STORAGE_KEY = 'pinvou.web.theme';

/**
 * Normalize any persisted/stored value to 'system' | 'light' | 'dark';
 * unknown/missing values always fall back to following the system.
 * @param {string | null | undefined} value Any value from a persisted color_scheme, localStorage, or a bridge fallback object.
 * @returns {'system' | 'light' | 'dark'} The normalized color-scheme preference.
 */
export function normalizeColorScheme(value) {
  return value === 'light' || value === 'dark' ? value : 'system';
}

/** Whether the system is currently dark. Returns false when matchMedia is unavailable — undeterminable means light, a product rule rather than a degradation. */
export function systemPrefersDark() {
  return typeof window !== 'undefined'
    && typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

/**
 * Normalized preference → the theme to actually render.
 * @param {string} scheme 'system' | 'light' | 'dark'
 * @param {boolean} [systemDark] Whether the system is dark; when omitted, detected once at call time.
 * @returns {'light' | 'dark'} The theme to render; `system` follows the OS, light when undeterminable.
 */
export function resolveTheme(scheme, systemDark = systemPrefersDark()) {
  if (scheme === 'light') return 'light';
  if (scheme === 'dark') return 'dark';
  return systemDark ? 'dark' : 'light';
}
