import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(path.join(root, 'src/platform/tauri/bridge/memory.js'), 'utf8');
const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
vm.runInNewContext(source, { window: windowObject, setTimeout, clearTimeout, console });

const state = {
  activeSessionId: 'session-1',
  memory: { pending: [{ id: 'pending-1' }] },
  chatItems: [],
};
let response;
let rejectOverview = false;
const invoke = async command => {
  if (command === 'get_memory_overview' && rejectOverview) throw new Error('overview unavailable');
  return typeof response === 'function' ? response(command) : response;
};
const api = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.memory({
  state,
  notify() {},
  invoke,
  bt: key => key,
  addSystemItem() {},
  runSyncOnSession() {},
  patchItemById() {},
  runOnSession(_sid, action) { action(); },
  addChatItem(item) { state.chatItems.push(item); },
  timeStr() { return ''; },
});

const availableSources = {
  profile: { available: true }, preferences: { available: true }, work_context: { available: true },
  current_focus: { available: true }, recent_activity: { available: true }, recent_work: { available: true },
  pending: { available: true }, never: { available: true },
};
const overview = (overrides = {}) => ({
  profile: { identity: { call_name: 'Ada' } },
  preferences: [{ id: 'pref-1', text: 'concise' }],
  work_context: [], current_focus: [], recent_activity: [], recent_work: [],
  pending: [{ id: 'pending-1' }], never: [], runtime: null, snapshot_path: '', warnings: [],
  sources: availableSources,
  ...overrides,
});

response = overview();
await api.loadMemoryOverview();
assert.equal(state.memory.preferences[0].text, 'concise');

response = overview({
  preferences: [],
  warnings: [{ code: 'memory_source_unavailable', source: 'preferences', detail: 'locked' }],
  sources: { ...availableSources, preferences: { available: false, code: 'memory_source_unavailable' } },
});
await api.loadMemoryOverview();
assert.equal(state.memory.preferences[0].text, 'concise', 'unavailable source must preserve last success');
assert.equal(state.memory.sources.preferences.available, false);

state.memory.snapshot_path = '/snap/last-success.json';
response = overview({
  snapshot_path: '',
  warnings: [{ code: 'snapshot_refresh_failed', source: 'snapshot', detail: 'locked' }],
  sources: { ...availableSources, snapshot: { available: false, code: 'snapshot_refresh_failed' } },
});
await api.loadMemoryOverview();
assert.equal(state.memory.snapshot_path, '/snap/last-success.json', 'unavailable snapshot source must preserve last path');
assert.equal(state.memory.sources.snapshot.available, false);

response = overview({ preferences: [] });
await api.loadMemoryOverview();
assert.deepEqual(state.memory.preferences, [], 'successful empty source must replace stale content');

state.memory.preferences = [{ id: 'pref-old', text: 'concise' }];
const cleanupWarning = { code: 'memory_topic_cleanup_required', source: 'preferences', detail: 'occupied' };
response = command => command === 'update_memory_preference'
  ? { value: { id: 'pref-new', text: 'detailed' }, runtime: null, warnings: [{ code: 'runtime_refresh_failed' }, cleanupWarning] }
  : overview({ preferences: [{ id: 'pref-new', text: 'detailed' }], warnings: [{ code: 'snapshot_refresh_failed' }, cleanupWarning] });
rejectOverview = true;
await api.updateMemoryItem('preference', 'pref-old', { topic: 'workflow_preference', text: 'detailed' });
rejectOverview = false;
assert.deepEqual(state.memory.preferences.map(item => item.id), ['pref-new'], 'topic update must remove both old and returned ids');
assert.equal(state.memory.preferences[0].text, 'detailed', 'committed update must apply before overview refresh');
assert.equal(state.memory.warnings[0].code, 'memory_topic_cleanup_required');

state.memory.work_context = [{ id: 'ctx-old', text: 'old context' }];
const contextCleanupWarning = { code: 'memory_topic_cleanup_required', source: 'work_context', detail: 'occupied' };
response = command => command === 'update_work_context_memory'
  ? { value: { id: 'ctx-new', text: 'new context' }, runtime: null, warnings: [contextCleanupWarning] }
  : overview({
      preferences: [{ id: 'pref-new', text: 'detailed' }],
      work_context: [{ id: 'ctx-new', text: 'new context' }],
      warnings: [contextCleanupWarning],
    });
await api.updateMemoryItem('work_context', 'ctx-old', { topic: 'project_context', text: 'new context' });
assert.deepEqual(state.memory.work_context.map(item => item.id), ['ctx-new']);
assert.equal(state.memory.warnings[0].code, 'memory_topic_cleanup_required');

response = command => command === 'delete_memory_preference'
  ? { value: true, runtime: null, warnings: [{ code: 'runtime_refresh_failed' }] }
  : overview();
rejectOverview = true;
await api.deleteMemoryPreference('pref-new');
assert.deepEqual(state.memory.preferences, [], 'committed delete must survive overview failure');
rejectOverview = false;

response = command => command === 'confirm_pending_memory'
  ? { value: { id: 'pending-1', action: 'confirmed' }, runtime: null, warnings: [{ code: 'runtime_refresh_failed' }] }
  : overview();
rejectOverview = true;
await api.confirmMemoryCandidate('pending-1');
assert.deepEqual(state.memory.pending, [], 'committed confirmation must survive overview failure');
rejectOverview = false;
