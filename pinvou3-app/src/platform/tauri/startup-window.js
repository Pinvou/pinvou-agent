import { invokeTauri } from './client.js';

const defaultWarn = (message, error) => console.warn(message, error);

/**
 * Reveal a main window that the Linux config created hidden. The URL marker
 * avoids touching other platform startup paths.
 */
export async function revealStartupWindow({
  search = globalThis.location?.search || '',
  invoke = invokeTauri,
  warn = defaultWarn,
} = {}) {
  if (new URLSearchParams(search).get('startupWindow') !== 'hidden') return false;

  try {
    return await invoke('reveal_startup_window') === true;
  } catch (error) {
    warn('[startup] failed to reveal main window', error);
    return false;
  }
}
