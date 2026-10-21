import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

/** Load the native bridge/router/terminal scripts without bridge init or the UI. */
export function createNativeToolOutputHarness({ eventSource, shellTasks = () => [], requestAnimationFrame = true } = {}) {
  let now = 0;
  let nextTimer = 1;
  const timers = new Map();
  const listeners = new Map();
  const fixtures = new Map();
  const commands = [];
  const setTimeout = (callback, delay = 0) => {
    const id = nextTimer++;
    timers.set(id, { at: now + Number(delay), callback });
    return id;
  };
  const clearTimeout = id => timers.delete(id);
  class ClockDate extends Date {
    constructor(...args) { super(...(args.length ? args : [Date.UTC(2026, 0, 1) + now])); }
    static now() { return Date.UTC(2026, 0, 1) + now; }
  }
  const localStorage = { getItem: () => null, setItem() {}, removeItem() {} };
  const invoke = async (command, args) => {
    commands.push({ command, args });
    if (command === 'load_session') return structuredClone(fixtures.get(args.id) || { metadata: { id: args.id }, messages: [], artifacts: [] });
    if (command === 'list_sessions') return [...fixtures.values()].map(value => value.metadata);
    if (command === 'get_session_mode' || command === 'get_default_mode') return 'work';
    if (command === 'get_mode_state') return { mode: 'yolo', multi_agent: false };
    if (command === 'get_settings') return {};
    if (command === 'list_shell_tasks') return structuredClone(shellTasks(args, now));
    if (/list|timeline|events|reviews|messages|snapshots/.test(command)) return [];
    return null;
  };
  const window = {
    __TAURI__: { core: { invoke }, event: { listen: async (name, callback) => {
      const callbacks = listeners.get(name) || [];
      callbacks.push(callback);
      listeners.set(name, callbacks);
      return () => {
        const remaining = (listeners.get(name) || []).filter(value => value !== callback);
        if (remaining.length) listeners.set(name, remaining);
        else listeners.delete(name);
      };
    } }, dialog: { open: async () => null } },
    performance: { now: () => now }, addEventListener() {}, localStorage, setTimeout, clearTimeout,
  };
  if (requestAnimationFrame) {
    window.requestAnimationFrame = callback => setTimeout(() => callback(now), 16);
    window.cancelAnimationFrame = clearTimeout;
  }
  const context = vm.createContext({ window, document: { readyState: 'loading', addEventListener() {}, createElement: () => ({ style: {} }), body: { appendChild() {} } }, navigator: {}, console,
    setTimeout, clearTimeout, setInterval: () => 0, clearInterval() {}, structuredClone, localStorage,
    performance: window.performance, Date: ClockDate,
  });
  const html = fs.readFileSync(path.join(appRoot, 'src/index.html'), 'utf8');
  const scripts = [...html.matchAll(/<script src="%BASE_URL%(shared\/bridge-messages\.js|shared\/bridge-shared-helpers\.js|platform\/tauri\/bridge(?:\.js|\/[^"]+))"[^>]*><\/script>/g)].map(match => match[1]);
  for (const relative of scripts) {
    const source = relative === 'platform/tauri/bridge/chat-events.js' && eventSource !== undefined
      ? eventSource : fs.readFileSync(path.join(appRoot, 'src', relative), 'utf8');
    vm.runInContext(source, context, { filename: relative });
  }
  const api = window.TauriBridge;
  const emit = (event, payload) => {
    if (!listeners.has(event)) throw new Error(`No native listener for ${event}`);
    return [...listeners.get(event)].map(callback => callback({ event, payload }));
  };
  const advance = milliseconds => {
    const until = now + milliseconds;
    while (true) {
      const due = [...timers].filter(([, timer]) => timer.at <= until).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!due) break;
      now = due[1].at;
      timers.delete(due[0]);
      due[1].callback();
    }
    now = until;
  };
  const prepare = (id, messages = []) => fixtures.set(id, { metadata: { id, title: id }, messages, artifacts: [] });
  const load = async id => {
    if (!fixtures.has(id)) prepare(id);
    if (!await api.sessions.switchToSession(id)) throw new Error(`Could not load ${id}`);
  };
  return { api, emit, advance, prepare, load, commands, snapshot: () => api.state.get('chat'), now: () => now };
}
