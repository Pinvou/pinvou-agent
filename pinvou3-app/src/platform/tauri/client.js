/**
 * Narrow access to the Tauri browser global.
 *
 * Feature modules depend on these functions instead of reading `__TAURI__`
 * directly. This keeps browser-preview fallback and native API shape changes
 * inside the platform adapter.
 */
function runtime() {
  return globalThis.__TAURI__ || null;
}

export function isTauriAvailable() {
  return typeof runtime()?.core?.invoke === 'function';
}

export function invokeTauri(command, payload) {
  const invoke = runtime()?.core?.invoke;
  if (typeof invoke !== 'function') {
    return Promise.reject(new Error('Tauri invoke is unavailable'));
  }
  return invoke(command, payload);
}

export function listenTauri(eventName, handler) {
  const listen = runtime()?.event?.listen;
  if (typeof listen !== 'function') {
    return Promise.reject(new Error('Tauri event.listen is unavailable'));
  }
  return listen(eventName, handler);
}

export function emitTauri(eventName, payload) {
  const emit = runtime()?.event?.emit;
  if (typeof emit !== 'function') {
    return Promise.reject(new Error('Tauri event.emit is unavailable'));
  }
  return emit(eventName, payload);
}

export const tauriCommands = Object.freeze({ invoke: invokeTauri });
export const tauriEvents = Object.freeze({ listen: listenTauri, emit: emitTauri });

export function openTauriDialog(options) {
  const open = runtime()?.dialog?.open;
  if (typeof open !== 'function') {
    return Promise.reject(new Error('Tauri dialog.open is unavailable'));
  }
  return open(options);
}

export function saveTauriDialog(options) {
  const save = runtime()?.dialog?.save;
  if (typeof save !== 'function') {
    return Promise.reject(new Error('Tauri dialog.save is unavailable'));
  }
  return save(options);
}

function windowApi() {
  const api = runtime()?.window;
  if (!api) throw new Error('Tauri window API is unavailable');
  return api;
}

export function getCurrentTauriWindow() {
  const api = windowApi();
  if (typeof api.getCurrentWindow !== 'function') {
    throw new Error('Tauri window.getCurrentWindow is unavailable');
  }
  return api.getCurrentWindow();
}

export function tryGetCurrentTauriWindow() {
  try {
    return getCurrentTauriWindow();
  } catch (_) {
    return null;
  }
}

export function currentTauriMonitor() {
  const api = windowApi();
  if (typeof api.currentMonitor !== 'function') {
    throw new Error('Tauri window.currentMonitor is unavailable');
  }
  return api.currentMonitor();
}

export function availableTauriMonitors() {
  const api = windowApi();
  return typeof api.availableMonitors === 'function' ? api.availableMonitors() : Promise.resolve([]);
}

export function createPhysicalPosition(x, y) {
  const api = windowApi();
  if (typeof api.PhysicalPosition !== 'function') {
    throw new Error('Tauri window.PhysicalPosition is unavailable');
  }
  return new api.PhysicalPosition(Math.round(x), Math.round(y));
}
