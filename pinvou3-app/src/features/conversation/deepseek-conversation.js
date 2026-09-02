import {
  countsAsFailedOperation,
  presentConversationItems,
} from './conversation-model.js';
const SHELL_TOOLS = new Set([
  'exec_shell',
  'exec_shell_wait',
  'exec_wait',
  'task_shell_start',
  'task_shell_wait',
  'shell',
  'Bash',
]);

export function conversationItemsForMode(chatItems = [], unified = true) {
  const items = Array.isArray(chatItems) ? chatItems : [];
  if (!unified) return items;
  return items.filter(item => !(item && item.legacyConversationOnly));
}

function stableItemId(item, index) {
  return `deepseek-${item && item.id != null ? item.id : index}`;
}

function toolStatus(item) {
  if (item && item.state === 'running') return 'in_progress';
  if (item && (item.success === false || item.state === 'failed')) return 'failed';
  if (item && (item.state === 'done' || item.success === true)) return 'completed';
  return 'pending';
}

function projectItem(item, index, copyOptions) {
  const id = stableItemId(item, index);
  if (item.type === 'assistant') {
    return {
      id,
      type: 'agent_message',
      text: item.text || '',
      copyOptions,
      status: item.streaming ? 'in_progress' : 'completed',
      legacyItem: item,
    };
  }
  if (item.type === 'reasoning') {
    return {
      id,
      type: 'reasoning',
      text: item.text || '',
      status: item.streaming ? 'in_progress' : 'completed',
      startedAt: item.startedAt,
      completedAt: item.completedAt,
      legacyItem: item,
    };
  }
  if (item.type === 'tool') {
    const status = toolStatus(item);
    return {
      id,
      type: SHELL_TOOLS.has(item.name) ? 'command_execution' : 'tool',
      status,
      tool: {
        name: item.name || '',
        title: item.name || '工具',
        kind: SHELL_TOOLS.has(item.name) ? 'execute' : 'tool',
        rawInput: item.args,
        rawOutput: item.output,
        content: item.output,
        status,
      },
      legacyItem: item,
    };
  }

  const semanticType = {
    plan_card: 'plan',
    plan_stuck: 'plan',
    careful_blocked: 'permission',
    user_input: 'user_input',
    artifact_card: 'artifact',
    memory_notice: 'memory',
    memory_candidate: 'memory',
    system: 'system_notice',
  }[item.type] || 'extension';

  return {
    id,
    type: semanticType,
    extensionType: item.type,
    status: item.resolved === false ? 'waiting' : 'completed',
    legacyItem: item,
  };
}

function emptyTurn(id) {
  return {
    id,
    userItem: null,
    userText: '',
    items: [],
    presentation: [],
    permissions: [],
    waitingPermission: false,
    waitingInput: false,
    usage: null,
    status: 'completed',
    error: null,
    startedAt: null,
    completedAt: null,
    lifecycleKnown: false,
    operationCount: 0,
    failedOperationCount: 0,
  };
}

function normalizeTurnStatus(status, completed) {
  const normalized = String(status || '').toLowerCase();
  if (normalized === 'completed') return 'Completed';
  if (['failed', 'send_error', 'refused'].includes(normalized)) return 'Failed';
  if (['interrupted', 'cancelled', 'canceled'].includes(normalized)) return 'Interrupted';
  if (normalized === 'limitreached' || normalized === 'limit_reached') return 'LimitReached';
  return completed ? String(status || 'Completed') : 'incomplete';
}

function timelineUsage(usage) {
  if (!usage || typeof usage !== 'object') return null;
  return {
    inputTokens: Number(usage.input_tokens || 0),
    outputTokens: Number(usage.output_tokens || 0),
    cacheHitTokens: Number(usage.cache_hit_tokens || 0),
    cacheMissTokens: Number(usage.cache_miss_tokens || 0),
    cacheWriteTokens: Number(usage.cache_write_tokens || 0),
    reasoningTokens: Number(usage.reasoning_tokens || 0),
  };
}

// Exported for the sibling subagent transcript adapter
// (subagent-conversation.mjs) so every lane shares one gate / redaction /
// trilingual copy implementation instead of reimplementing it.
export function timelineUserError(event, options = {}) {
  const existing = event && (event.user_error || event.userError);
  if (existing && typeof existing === 'object') return existing;
  const error = event && event.error;
  const helper = globalThis.PinvouModelServiceErrors;
  if (!error || !helper || typeof helper.build !== 'function') return null;
  if (typeof helper.isModelServiceError === 'function' && !helper.isModelServiceError(error)) return null;
  const providerLabel = options.providerLabel
    || (typeof helper.providerLabelFromState === 'function'
      ? helper.providerLabelFromState(options.modelServiceState, null, options.language)
      : '');
  return helper.build(error, {
    language: options.language,
    providerLabel,
  });
}

/// 时间线裸 error 是给红字兜底展示的:权威回读/历史会话里可能带网关或
/// provider 原始报文,门控没接管(不建 user_error 卡)的也要先脱敏再展示
/// ——分类允许漏判,凭证不允许漏。userError 卡走 build(),自身已脱敏;
/// provider 信号提取(如 URL 里的 api.deepseek.com)在 build 内用原文,不受影响。
export function timelineDisplayError(error, options = {}) {
  if (!error) return error || null;
  const helper = globalThis.PinvouModelServiceErrors;
  if (!helper || typeof helper.redactTechnicalDetail !== 'function') return error;
  return helper.redactTechnicalDetail(String(error), options.language);
}

/**
 * timing_events.jsonl 是 DeepSeek 回合生命周期的事实源。这里把
 * user_start / assistant_done 配成只读 Turn 元数据，不改写消息历史。
 */
export function pairDeepSeekTimeline(events = [], options = {}) {
  const ordered = [...events]
    .filter(event => event && event.turn_id && ['user_start', 'assistant_done'].includes(event.event))
    .sort((left, right) => Number(left.timestamp || 0) - Number(right.timestamp || 0));
  const records = [];
  const byId = new Map();
  for (const event of ordered) {
    const id = String(event.turn_id);
    let record = byId.get(id);
    if (!record) {
      record = {
        id,
        turnIndex: Number.isSafeInteger(event.ui_turn_index) ? event.ui_turn_index : null,
        startedAt: null,
        completedAt: null,
        status: 'incomplete',
        rawStatus: '',
        error: null,
        userError: null,
        usage: null,
      };
      byId.set(id, record);
      records.push(record);
    }
    if (event.event === 'user_start') {
      record.startedAt = Number(event.timestamp || 0) || event.ts || null;
      if (Number.isSafeInteger(event.ui_turn_index)) record.turnIndex = event.ui_turn_index;
    } else {
      record.completedAt = Number(event.timestamp || 0) || event.ts || null;
      record.rawStatus = String(event.status || '');
      record.status = normalizeTurnStatus(event.status, true);
      record.error = timelineDisplayError(event.error, options);
      record.userError = timelineUserError(event, options);
      record.usage = timelineUsage(event.usage);
    }
  }
  // send_error 发生在消息被 engine 接纳之前，不对应可见的用户 Turn。
  return records.filter(record => record.startedAt && record.rawStatus.toLowerCase() !== 'send_error');
}

// Attach timing-record lifecycles to the projected turns. Kept standalone so
// the projection itself stays under the cognitive-complexity budget.
//   - Primary pairing matches record.turnIndex against admitted user turns
//     (steered mid-turn injections are excluded upstream: they logged no
//     user_start of their own and must not consume a record). While the
//     session is busy, a record without a terminal is merely in flight — skip
//     it AND consume it so the trailing fallback cannot re-attach it (live
//     repro: a steered continuation badged the admitted turn "interrupted"
//     although its own response segment had already finished).
//   - The trailing fallback pairs leftovers positionally; with more records
//     than turns the surplus is record noise (a parked-steer resume
//     continuation whose assistant_done attached to the original turn id), so
//     prefer records that actually carry a terminal.
function assignDeepSeekTimelines(turns, userTurns, timelineEvents, busy, options) {
  const timeline = pairDeepSeekTimeline(timelineEvents, options);
  const assigned = new Set();
  for (const record of timeline) {
    if (!Number.isSafeInteger(record.turnIndex) || !userTurns[record.turnIndex]) continue;
    if (busy && record.status === 'incomplete') {
      assigned.add(record.id);
      continue;
    }
    Object.assign(userTurns[record.turnIndex], {
      status: record.status,
      error: record.error,
      userError: record.userError,
      startedAt: record.startedAt,
      completedAt: record.completedAt,
      usage: record.usage,
      lifecycleKnown: true,
    });
    assigned.add(record.id);
  }
  const unassignedRecords = timeline.filter(record => !assigned.has(record.id));
  const unassignedTurns = userTurns.filter(turn => !turn.lifecycleKnown);
  const trailingSource = unassignedRecords.length > unassignedTurns.length
    ? unassignedRecords.filter(record => record.status !== 'incomplete')
    : unassignedRecords;
  const trailingPairCount = Math.min(trailingSource.length, unassignedTurns.length);
  if (trailingPairCount <= 0) return;
  const trailingRecords = trailingSource.slice(-trailingPairCount);
  const trailingTurns = unassignedTurns.slice(-trailingPairCount);
  trailingRecords.forEach((record, index) => {
    Object.assign(trailingTurns[index], {
      status: record.status,
      error: record.error,
      userError: record.userError,
      startedAt: record.startedAt,
      completedAt: record.completedAt,
      usage: record.usage,
      lifecycleKnown: true,
    });
  });
}

// A steered turn belongs to the same engine turn as the admitted message
// before it (it consumed no timing record of its own). That engine turn's
// terminal arrives on the admitted turn's record — but visually the task
// ended at the run's TAIL (the last steered continuation), and only the tail
// should carry the badge (live feedback: stamping every turn before an
// insertion point duplicated the terminal down the whole run). While busy
// nothing has a terminal, so nothing moves and the run stays badge-less
// except the active indicator on the trailing turn.
function transferSteeredRunTerminals(turns) {
  let start = 0;
  while (start < turns.length) {
    let end = start;
    while (end + 1 < turns.length
      && turns[end + 1].userItem
      && turns[end + 1].userItem.steeredMidTurn) end += 1;
    if (end > start) {
      const head = turns[start];
      const tail = turns[end];
      if (head.lifecycleKnown && head.completedAt) {
        tail.status = head.status;
        tail.error = head.error;
        tail.userError = head.userError;
        tail.startedAt = head.startedAt;
        tail.completedAt = head.completedAt;
        tail.usage = head.usage;
        tail.lifecycleKnown = true;
        for (let clear = start; clear < end; clear += 1) {
          turns[clear].status = 'completed';
          turns[clear].error = null;
          turns[clear].userError = null;
          turns[clear].completedAt = null;
          turns[clear].usage = null;
          turns[clear].lifecycleKnown = false;
        }
      }
    }
    start = end + 1;
  }
}

/**
 * DeepSeek-TUI 仍以原 chatItems / SavedSession 为事实源；这里只做只读投影，
 * 不改变底座消息、工具、记忆、产物或计划卡的存储格式。
 */
export function projectDeepSeekConversation({
  chatItems = [],
  busy = false,
  thinking = null,
  tokens = null,
  sessionId = null,
  timelineEvents = [],
  allowScheduledTaskDraft = false,
  language = 'zh-Hans',
  providerLabel = '',
  modelServiceState = null,
} = {}) {
  const turns = [];
  const userTurns = [];
  let current = null;

  function ensureTurn(index) {
    if (!current) {
      current = emptyTurn(`deepseek-${sessionId || 'session'}-preamble-${index}`);
      turns.push(current);
    }
    return current;
  }

  for (let index = 0; index < chatItems.length; index += 1) {
    const item = chatItems[index];
    if (!item) continue;
    if (item.type === 'user') {
      current = emptyTurn(`deepseek-${sessionId || 'session'}-turn-${item.id == null ? index : item.id}`);
      current.userItem = item;
      current.userText = String(item.text || '');
      turns.push(current);
      // A steered mid-turn injection is NOT a turn admission: it has no
      // user_start timing record of its own, so it must not consume one —
      // otherwise the record lands on the wrong turn (a natural queue showed
      // a phantom "interrupted" badge) and the admitted turn loses its
      // lifecycle. It still renders as its own visual turn.
      if (!item.steeredMidTurn) userTurns.push(current);
      continue;
    }
    ensureTurn(index).items.push(projectItem(item, index, { allowScheduledTaskDraft }));
  }

  for (const turn of turns) {
    turn.presentation = presentConversationItems(turn.items);
    const operations = turn.items.filter(item => (
      ['command_execution', 'file_change', 'tool'].includes(item.type)
    ));
    turn.operationCount = operations.length;
    turn.failedOperationCount = operations.filter(countsAsFailedOperation).length;
    turn.waitingPermission = turn.items.some(item => (
      item.type === 'permission'
      && item.legacyItem
      && item.legacyItem.resolved === false
    ));
    turn.waitingInput = turn.items.some(item => (
      item.type === 'user_input'
      && item.legacyItem
      && item.legacyItem.resolved === false
    ));
  }

  assignDeepSeekTimelines(turns, userTurns, timelineEvents, busy, { language, providerLabel, modelServiceState });
  if (!busy) transferSteeredRunTerminals(turns);

  const activeTurn = turns[turns.length - 1];
  if (activeTurn && busy) {
    activeTurn.status = 'running';
    activeTurn.startedAt = thinking && thinking.startedAt || Date.now();
    activeTurn.completedAt = null;
    activeTurn.error = null;
    activeTurn.lifecycleKnown = true;
    activeTurn.activityToolName = thinking && thinking.phase === 'tool' && thinking.toolName
      ? thinking.toolName
      : null;
  }
  if (activeTurn && busy && tokens && tokens.max > 0) {
    activeTurn.usage = {
      used: Number(tokens.input || 0),
      size: Number(tokens.max || 0),
    };
  }

  return {
    thread: {
      id: sessionId,
      turns,
    },
    turns,
  };
}
