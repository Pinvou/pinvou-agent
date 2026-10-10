/**
 * Pure logic for the app-automations scheduled-task timeline headers
 * (docs/builtin-toolset-contract.md §3.2 execution visibility; the
 * state-changing tools are L1, so what the model asked for and what the app
 * answered must be fully visible on the timeline).
 *
 * Startup-entry module: the summaries render synchronously in the
 * timeline's tool header. The result-card parser and the prompt excerpt
 * live in scheduled-task-card-logic.js, loaded only inside the lazy
 * scheduled-task card chunk (the startup-budget policy in
 * vite.config.mjs).
 *
 * Kept framework-free (mirroring spawn-aggregation.mjs / session-message-block.js)
 * so tests/ can exercise it directly.
 */

export const SCHEDULED_TASK_CREATE_TOOL = 'mcp_app-automations_create_scheduled_task';
export const SCHEDULED_TASK_READ_TOOL = 'mcp_app-automations_read_scheduled_task';
export const SCHEDULED_TASK_LIST_TOOL = 'mcp_app-automations_list_scheduled_tasks';
export const SCHEDULED_TASK_UPDATE_TOOL = 'mcp_app-automations_update_scheduled_task';
export const SCHEDULED_TASK_DELETE_TOOL = 'mcp_app-automations_delete_scheduled_task';

/**
 * Header summary for a create call: task name + rrule (the two fields that
 * identify the request at a glance); empty strings degrade to '' so the
 * fallback summary rendering stays honest.
 */
export const scheduledTaskCreateSummary = (args, nameQuote) => {
  if (!args || typeof args !== 'object') return '';
  const name = typeof args.name === 'string' ? args.name.trim() : '';
  const rrule = typeof args.rrule === 'string' ? args.rrule.trim() : '';
  // The quoting is localized copy (round-6 minor 9 — hardcoded CJK brackets
  // gave English users 「…」); the formatter receives the bare name.
  const quote = typeof nameQuote === 'function' ? nameQuote : (n => n);
  if (name && rrule) return `${quote(name)} · ${rrule}`;
  return name || rrule;
};

/**
 * Header summary for an update call: target id + the changed field names, so
 * the timeline shows what the model asked to touch without opening the card.
 */
const UPDATE_FIELD_KEYS = ['name', 'prompt', 'rrule', 'model_id', 'paused', 'target_session'];
export const scheduledTaskUpdateSummary = args => {
  if (!args || typeof args !== 'object') return '';
  const target = typeof args.task_id === 'string' ? args.task_id.trim() : '';
  const fields = UPDATE_FIELD_KEYS.filter(key => {
    const value = args[key];
    return value !== undefined && value !== null && String(value).trim() !== '';
  });
  return [target, fields.length ? fields.join('/') : ''].filter(Boolean).join(' · ');
};

/**
 * Header summary for a delete call: the target id is the whole story.
 */
export const scheduledTaskDeleteSummary = args => {
  if (!args || typeof args !== 'object') return '';
  return typeof args.task_id === 'string' ? args.task_id.trim() : '';
};

/**
 * Header summary for a list call: the localized verb plus the requested
 * limit (the only argument).
 */
export const scheduledTaskListSummary = (args, listLabel, limitSuffix) => {
  let limit = '';
  if (args?.limit != null && typeof limitSuffix === 'function') {
    const value = args.limit;
    const numeric = typeof value === 'number' || typeof value === 'boolean'
      || (typeof value === 'string' && /^[+-]?\d+$/.test(value.trim()));
    const requested = numeric ? Number(value) : NaN;
    const count = Number.isFinite(requested) ? Math.trunc(requested) : 20;
    limit = limitSuffix(Math.max(1, Math.min(100, count)));
  }
  return `${listLabel}${limit}`;
};
