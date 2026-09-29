/**
 * Pure logic for the app-automations scheduled-task tool cards
 * (docs/builtin-toolset-contract.md §3.2 execution visibility; the create
 * tool is L1, so what the model asked for and what the app answered must be
 * fully visible on the timeline).
 *
 * Kept framework-free (mirroring spawn-aggregation.mjs / session-message-block.js)
 * so tests/ can exercise it directly.
 */

export const SCHEDULED_TASK_CREATE_TOOL = 'mcp_app-automations_create_scheduled_task';
export const SCHEDULED_TASK_LIST_TOOL = 'mcp_app-automations_list_scheduled_tasks';

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
 * Header summary for a list call: the requested limit is the only argument.
 */
export const scheduledTaskListSummary = args => {
  const limit = args && args.limit != null ? ` · limit ${args.limit}` : '';
  return `list${limit}`;
};

/**
 * Parses a create tool's text output into a render shape:
 * - { kind:'created', taskId, taskName } — the watcher confirmed creation;
 * - { kind:'pending', taskName }  — queued, no confirmation within the
 *   server's short wait (still NOT an error);
 * - { kind:'failed', error }      — the creation was rejected/failed;
 * - null                          — anything unparseable (drift defense: the
 *   caller falls back to the default raw output view).
 */
export const parseScheduledTaskCreateOutput = output => {
  if (typeof output !== 'string' || !output.trim()) return null;
  let payload;
  try {
    payload = JSON.parse(output.trim());
  } catch {
    return null;
  }
  if (!payload || typeof payload !== 'object') return null;
  if (payload.ok === true && payload.taskId) {
    return {
      kind: 'created',
      taskId: String(payload.taskId),
      taskName: typeof payload.taskName === 'string' ? payload.taskName : '',
    };
  }
  if (payload.ok === true && payload.delivery === 'pending') {
    return {
      kind: 'pending',
      taskName: typeof payload.taskName === 'string' ? payload.taskName : '',
    };
  }
  if (payload.ok === false && typeof payload.error === 'string') {
    return { kind: 'failed', error: payload.error };
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
