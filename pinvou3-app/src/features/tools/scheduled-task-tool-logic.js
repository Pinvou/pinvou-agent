/**
 * Pure logic for the app-automations scheduled-task tool cards
 * (docs/builtin-toolset-contract.md §3.2 execution visibility; the
 * state-changing tools are L1, so what the model asked for and what the app
 * answered must be fully visible on the timeline).
 *
 * Kept framework-free (mirroring spawn-aggregation.mjs / session-message-block.js)
 * so tests/ can exercise it directly.
 */

export const SCHEDULED_TASK_CREATE_TOOL = 'mcp_app-automations_create_scheduled_task';
export const SCHEDULED_TASK_READ_TOOL = 'mcp_app-automations_read_scheduled_task';
export const SCHEDULED_TASK_LIST_TOOL = 'mcp_app-automations_list_scheduled_tasks';
export const SCHEDULED_TASK_UPDATE_TOOL = 'mcp_app-automations_update_scheduled_task';
export const SCHEDULED_TASK_DELETE_TOOL = 'mcp_app-automations_delete_scheduled_task';

const PROMPT_EXCERPT_CHARS = 120;

/**
 * Header summary for a create call: task name + rrule (the two fields that
 * identify the request at a glance); empty strings degrade to '' so the
 * fallback summary rendering stays honest.
 */
export const scheduledTaskCreateSummary = args => {
  if (!args || typeof args !== 'object') return '';
  const name = typeof args.name === 'string' ? args.name.trim() : '';
  const rrule = typeof args.rrule === 'string' ? args.rrule.trim() : '';
  if (name && rrule) return `「${name}」 · ${rrule}`;
  return name || rrule;
};

/**
 * Header summary for an update call: target id + the changed field names, so
 * the timeline shows what the model asked to touch without opening the card.
 */
const UPDATE_FIELD_KEYS = ['name', 'prompt', 'rrule', 'model_id', 'paused'];
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
export const scheduledTaskListSummary = (args, listLabel) => {
  const limit = args && args.limit != null ? ` · limit ${args.limit}` : '';
  return `${listLabel}${limit}`;
};

/**
 * Parses a create/update/delete tool's text output into a render shape:
 * - { kind:'created'|'updated'|'deleted', taskId, taskName, duplicate? } —
 *   the watcher confirmed the operation (`duplicate` marks the recorded
 *   result of an idempotent replay);
 * - { kind:'pending', taskName }  — queued, no confirmation within the
 *   server's short wait (still NOT an error);
 * - null                          — failures and anything unparseable (drift
 *   defense: error outputs render through the is_error path, and unknown
 *   shapes fall back to the default raw output view).
 */
export const parseScheduledTaskToolOutput = output => {
  if (typeof output !== 'string' || !output.trim()) return null;
  let payload;
  try {
    payload = JSON.parse(output.trim());
  } catch {
    return null;
  }
  if (!payload || typeof payload !== 'object') return null;
  if (payload.ok === true && payload.delivery === 'pending') {
    return {
      kind: 'pending',
      taskName: typeof payload.taskName === 'string' ? payload.taskName : '',
    };
  }
  if (payload.ok === true && payload.taskId) {
    const opByKind = { create: 'created', update: 'updated', delete: 'deleted' };
    const kind = opByKind[payload.kind];
    // An unknown kind is drift: fall back to the raw view instead of
    // asserting a specific affirmative outcome.
    if (!kind) return null;
    return {
      kind,
      taskId: String(payload.taskId),
      taskName: typeof payload.taskName === 'string' ? payload.taskName : '',
      duplicate: payload.duplicate === true,
    };
  }
  return null;
};

/**
 * Prompt excerpt for the card body: first N chars + ellipsis marker, so a
 * 32k prompt cannot flood the timeline.
 */
export const scheduledTaskPromptExcerpt = args => {
  const prompt = args && typeof args.prompt === 'string' ? args.prompt.trim() : '';
  if (!prompt) return '';
  if (prompt.length <= PROMPT_EXCERPT_CHARS) return prompt;
  return `${prompt.slice(0, PROMPT_EXCERPT_CHARS)}…`;
};
