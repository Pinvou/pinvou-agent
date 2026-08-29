import {
  commandExecutionDetails,
  presentConversationItems,
} from '../conversation/conversation-model.js';

/// ACP 泳道错误文本(红字兜底)展示前的无条件脱敏:agent CLI 的原始报文
/// 可能带网关自定义 body 或凭证,门控没接管的不建友好卡,但仍不得带密上屏。
function redactDisplayError(error) {
  if (!error) return error || null;
  const helper = typeof globalThis !== 'undefined' && globalThis.PinvouModelServiceErrors;
  if (!helper || typeof helper.redactTechnicalDetail !== 'function') return error;
  return helper.redactTechnicalDetail(String(error));
}

export function unifiedConversationUiEnabled() {
  try {
    return localStorage.getItem('pinvou_conversation_ui_v2') !== 'false';
  } catch {
    return true;
  }
}

export function updateAcpAttachmentDraft(drafts, attachmentId, update) {
  for (const [owner, attachments] of Object.entries(drafts || {})) {
    if (attachments.every(attachment => attachment.id !== attachmentId)) continue;
    return {
      ...drafts,
      [owner]: attachments.map(attachment => (
        attachment.id === attachmentId ? update(attachment) : attachment
      )),
    };
  }
  return drafts;
}

function contentText(content) {
  if (!content) return '';
  if (typeof content === 'string') return content;
  if (content.type === 'text') return String(content.text || '');
  if (content.text != null) return String(content.text);
  return '';
}

function updatePayload(envelope) {
  const data = envelope && envelope.event && envelope.event.data;
  return data && data.update != null ? data.update : (data || {});
}

function mergeTool(current, update) {
  if (!current) return { ...update };
  return {
    ...current,
    ...update,
    content: update.content === undefined ? current.content : update.content,
    locations: update.locations === undefined ? current.locations : update.locations,
    rawInput: update.rawInput === undefined ? current.rawInput : update.rawInput,
    rawOutput: update.rawOutput === undefined ? current.rawOutput : update.rawOutput,
  };
}

function emptyTurn(id) {
  return {
    id,
    userText: '',
    userAttachments: [],
    assistantText: '',
    thoughtText: '',
    blocks: [],
    items: [],
    presentation: [],
    tools: [],
    toolIndex: {},
    toolBlockIndex: {},
    plan: null,
    planBlockIndex: null,
    permissions: [],
    permissionBlockIndex: {},
    elicitations: [],
    elicitationBlockIndex: {},
    waitingInput: false,
    usage: null,
    status: 'idle',
    error: null,
    startedAt: null,
    completedAt: null,
    operationCount: 0,
    failedOperationCount: 0,
  };
}

function isTerminalToolStatus(status) {
  return ['completed', 'failed', 'cancelled', 'canceled'].includes(String(status || '').toLowerCase());
}

function toolItemType(tool) {
  const kind = String(tool && tool.kind || '').toLowerCase();
  if (kind === 'execute') return 'command_execution';
  if (['edit', 'delete', 'move', 'write'].includes(kind)) return 'file_change';
  return 'tool';
}

function appendTextBlock(turn, type, text, envelope, phase = null) {
  if (!text) return;
  const last = turn.blocks[turn.blocks.length - 1];
  if (last && last.type === type && last.phase === phase) {
    last.text += text;
    last.updatedAt = envelope.timestamp;
    return;
  }
  turn.blocks.push({
    id: `${type}-${envelope.seq}`,
    type,
    text,
    phase,
    seq: envelope.seq,
    startedAt: envelope.timestamp,
    updatedAt: envelope.timestamp,
  });
}

function normalizeTurnItems(turn) {
  return turn.blocks.map((block, index) => {
    const next = turn.blocks[index + 1];
    const inferredEnd = next && next.startedAt || turn.completedAt || null;
    if (block.type === 'thought') {
      const completedAt = inferredEnd;
      return {
        ...block,
        type: 'reasoning',
        status: completedAt ? 'completed' : 'in_progress',
        completedAt,
      };
    }
    if (block.type === 'message') {
      const completedAt = inferredEnd;
      return {
        ...block,
        type: 'agent_message',
        status: completedAt ? 'completed' : 'in_progress',
        completedAt,
      };
    }
    if (block.type === 'tool') {
      const status = block.tool && block.tool.status || 'pending';
      return {
        ...block,
        type: toolItemType(block.tool),
        status: turn.completedAt && !isTerminalToolStatus(status) ? 'cancelled' : status,
        completedAt: block.completedAt || turn.completedAt || null,
      };
    }
    if (block.type === 'permission') {
      return {
        ...block,
        status: block.permission.resolved
          ? 'completed'
          : turn.completedAt ? 'cancelled' : 'waiting',
        completedAt: block.permission.resolvedAt || turn.completedAt || null,
      };
    }
    if (block.type === 'elicitation') {
      return {
        ...block,
        status: block.elicitation.resolved
          ? 'completed'
          : turn.completedAt ? 'cancelled' : 'waiting',
        completedAt: block.elicitation.resolvedAt || turn.completedAt || null,
      };
    }
    if (block.type === 'plan') {
      return { ...block, status: turn.completedAt ? 'completed' : 'in_progress', completedAt: turn.completedAt };
    }
    return block;
  });
}

/**
 * Item 是事实语义，presentation 只控制视觉聚合。工具组不会改写、合并或丢弃
 * 任何 Item；展开后仍按原始时序逐项展示。
 */
export function presentTurnItems(items) {
  return presentConversationItems(items);
}

/**
 * 把不可变 ACP event log 投影成 Codex 的 Thread → Turn → Item 模型。
 * 原始 event log 仍是事实源；tool update 只更新同一个 tool_call_id。
 */
// eslint-disable-next-line sonarjs/cognitive-complexity -- event log to Turn/Item projection: single-pass merge of many ACP event shapes; splitting would repeat the traversal
export function projectAcpTimeline(input) {
  const seen = new Set();
  const events = [...(input || [])]
    .filter(event => {
      const key = `${event && event.sessionId}:${event && event.seq}`;
      if (!event || seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .sort((a, b) => Number(a.seq || 0) - Number(b.seq || 0));

  const turns = [];
  const turnIndex = {};
  const global = [];

  function getTurn(event) {
    const id = event.turnId;
    if (!id) return null;
    if (turnIndex[id] == null) {
      turnIndex[id] = turns.length;
      turns.push(emptyTurn(id));
    }
    return turns[turnIndex[id]];
  }

  for (const envelope of events) {
    const type = envelope.event && envelope.event.type;
    const data = envelope.event && envelope.event.data || {};
    const update = updatePayload(envelope);
    const turn = getTurn(envelope);
    if (!turn) {
      global.push(envelope);
      continue;
    }
    if (type === 'user_message') {
      const blocks = Array.isArray(data.content) ? data.content : [];
      turn.userText += blocks.map(contentText).join('');
      turn.userAttachments = Array.isArray(data.attachments) ? data.attachments : [];
    } else if (type === 'user_message_chunk') {
      turn.userText += contentText(update.content);
    } else if (type === 'agent_message_chunk') {
      const text = contentText(update.content);
      const phase = update && update._meta && update._meta.codex && update._meta.codex.phase || 'message';
      turn.assistantText += text;
      appendTextBlock(turn, 'message', text, envelope, phase);
    } else if (type === 'agent_thought_chunk') {
      const text = contentText(update.content);
      turn.thoughtText += text;
      appendTextBlock(turn, 'thought', text, envelope);
    } else if (type === 'tool_call' || type === 'tool_call_update') {
      const id = String(update.toolCallId || '');
      if (!id) continue;
      const existingAt = turn.toolIndex[id];
      if (existingAt == null) {
        turn.toolIndex[id] = turn.tools.length;
        const tool = mergeTool(null, update);
        turn.tools.push(tool);
        turn.toolBlockIndex[id] = turn.blocks.length;
        turn.blocks.push({
          id: `tool-${id}`,
          type: 'tool',
          tool,
          seq: envelope.seq,
          startedAt: envelope.timestamp,
          updatedAt: envelope.timestamp,
          completedAt: isTerminalToolStatus(tool.status) ? envelope.timestamp : null,
        });
      } else {
        const tool = mergeTool(turn.tools[existingAt], update);
        turn.tools[existingAt] = tool;
        const block = turn.blocks[turn.toolBlockIndex[id]];
        block.tool = tool;
        block.updatedAt = envelope.timestamp;
        if (isTerminalToolStatus(tool.status)) block.completedAt = envelope.timestamp;
      }
    } else if (type === 'plan') {
      turn.plan = update;
      if (turn.planBlockIndex == null) {
        turn.planBlockIndex = turn.blocks.length;
        turn.blocks.push({
          id: `plan-${envelope.seq}`,
          type: 'plan',
          plan: update,
          seq: envelope.seq,
          startedAt: envelope.timestamp,
          updatedAt: envelope.timestamp,
        });
      } else {
        const block = turn.blocks[turn.planBlockIndex];
        block.plan = update;
        block.updatedAt = envelope.timestamp;
      }
    } else if (type === 'permission_requested') {
      const request = data.request || {};
      const permission = {
        toolCallId: String(data.toolCallId || (request.toolCall && request.toolCall.toolCallId) || ''),
        request,
        resolved: false,
        requestedAt: envelope.timestamp,
        resolvedAt: null,
      };
      turn.permissions.push(permission);
      turn.permissionBlockIndex[permission.toolCallId] = turn.blocks.length;
      turn.blocks.push({
        id: `permission-${permission.toolCallId || envelope.seq}`,
        type: 'permission',
        permission,
        seq: envelope.seq,
        startedAt: envelope.timestamp,
        updatedAt: envelope.timestamp,
      });
    } else if (type === 'permission_resolved') {
      const item = [...turn.permissions].reverse().find(p => p.toolCallId === String(data.toolCallId || '') && !p.resolved);
      if (item) {
        Object.assign(item, {
          resolved: true,
          resolvedAt: envelope.timestamp,
          optionId: data.optionId,
          outcome: data.outcome,
        });
        const block = turn.blocks[turn.permissionBlockIndex[item.toolCallId]];
        if (block) block.updatedAt = envelope.timestamp;
      }
    } else if (type === 'elicitation_requested') {
      const request = data.request || {};
      const elicitation = {
        elicitationId: String(data.elicitationId || ''),
        request,
        resolved: false,
        requestedAt: envelope.timestamp,
        resolvedAt: null,
      };
      turn.elicitations.push(elicitation);
      turn.elicitationBlockIndex[elicitation.elicitationId] = turn.blocks.length;
      turn.blocks.push({
        id: `elicitation-${elicitation.elicitationId || envelope.seq}`,
        type: 'elicitation',
        elicitation,
        seq: envelope.seq,
        startedAt: envelope.timestamp,
        updatedAt: envelope.timestamp,
      });
    } else if (type === 'elicitation_resolved') {
      const item = [...turn.elicitations].reverse().find(
        elicitation => elicitation.elicitationId === String(data.elicitationId || '')
          && !elicitation.resolved,
      );
      if (item) {
        Object.assign(item, {
          resolved: true,
          resolvedAt: envelope.timestamp,
          action: data.action,
        });
        const block = turn.blocks[turn.elicitationBlockIndex[item.elicitationId]];
        if (block) block.updatedAt = envelope.timestamp;
      }
    } else if (type === 'usage') {
      turn.usage = update;
    } else if (type === 'turn_started') {
      turn.status = 'running';
      turn.startedAt = envelope.timestamp;
    } else if (type === 'turn_completed') {
      turn.status = data.status || 'completed';
      // ACP 泳道的错误文本来自 agent CLI(codex/claude/gemini),可能原样携带
      // 网关报文或凭证;红字展示前无条件脱敏(分类可以漏,凭证不能漏)。
      // helper(classic script)缺失时原样保留,降级为既有行为。
      turn.error = redactDisplayError(data.error || null);
      turn.completedAt = envelope.timestamp;
    }
  }

  for (const turn of turns) {
    turn.waitingInput = !turn.completedAt
      && turn.elicitations.some(elicitation => !elicitation.resolved);
    turn.items = normalizeTurnItems(turn);
    turn.presentation = presentTurnItems(turn.items);
    const operations = turn.items.filter(item => (
      ['command_execution', 'file_change', 'tool'].includes(item.type)
    ));
    turn.operationCount = operations.length;
    turn.failedOperationCount = operations.filter(item => {
      if (String(item.status || '').toLowerCase() === 'failed') return true;
      if (item.type !== 'command_execution') return false;
      const exitCode = commandExecutionDetails(item.tool).exitCode;
      return exitCode != null && exitCode !== 0;
    }).length;
  }
  return {
    thread: {
      id: events[0] && events[0].sessionId || null,
      turns,
    },
    turns,
    global,
    events,
  };
}

export function appendAcpEvent(events, incoming) {
  if (!incoming) return events || [];
  if ((events || []).some(event => event.sessionId === incoming.sessionId && event.seq === incoming.seq)) {
    return events;
  }
  return [...(events || []), incoming].sort((a, b) => Number(a.seq || 0) - Number(b.seq || 0));
}

// The server-side OrderedWebDelivery watchdog skips a hole after a stalled
// predecessor times out, so the web live stream can show envelope-seq jumps
// (the desktop native path persists and broadcasts within the same ordered
// unit and is unaffected). The tracker records the max live seq per session
// and reports 'gap' on a jump so the view layer can refetch the authoritative
// timeline and restore the missing permission/terminal events; reconnect
// replays and duplicate deliveries return 'duplicate' and are ignored.
export function createAcpEventSeqTracker() {
  const lastSeqBySession = new Map();
  return {
    note(sessionId, seq) {
      const value = Number(seq) || 0;
      if (!sessionId || value <= 0) return 'ignored';
      const last = lastSeqBySession.get(sessionId) || 0;
      if (value <= last) return 'duplicate';
      lastSeqBySession.set(sessionId, value);
      return last > 0 && value > last + 1 ? 'gap' : 'ok';
    },
    // After a snapshot merge, baseline on the known max seq; a higher seq the
    // live stream already advanced to never regresses.
    rebase(sessionId, seq) {
      const value = Number(seq) || 0;
      if (!sessionId || value <= 0) return;
      if (value > (lastSeqBySession.get(sessionId) || 0)) lastSeqBySession.set(sessionId, value);
    },
  };
}

// Bounded backoff driver for gap resyncs. note() advances its baseline before
// reporting 'gap', so after a failed resync the following live envelopes look
// continuous and never retrigger on their own; without retries a single
// transient failure would permanently disable healing for that gap. schedule()
// debounces a burst of gap reports into one attempt; each failure reschedules
// with exponential backoff until an attempt succeeds or the attempt budget is
// exhausted. Retry state is scoped to the cycle: schedule() starts a fresh
// cycle for its session (a burst of gap reports still debounces into one
// attempt), and cancel() drops any pending attempt (unmount). Both also start
// a new generation, invalidating any attempt still in flight: its late
// completion can no longer reset the failure count, cancel the superseding
// pending attempt, or schedule a retry after cancel().
export function createAcpGapResyncScheduler(resync, {
  maxAttempts = 5,
  baseDelayMs = 800,
  maxDelayMs = 12800,
  setTimeout = null,
  clearTimeout = null,
  onAttempt = null,
  onRetry = null,
  onGiveUp = null,
} = {}) {
  const attemptBudget = Math.max(1, Number(maxAttempts) || 1);
  const firstDelayMs = Math.max(0, Number(baseDelayMs) || 0);
  const ceilingMs = Math.max(firstDelayMs, Number(maxDelayMs) || 0);
  const scheduleTimer = setTimeout || globalThis.setTimeout.bind(globalThis);
  const cancelTimer = clearTimeout || globalThis.clearTimeout.bind(globalThis);
  let timer = null;
  let failures = 0;
  let generation = 0;
  const delayAfterFailures = count => Math.min(firstDelayMs * 2 ** count, ceilingMs);
  const clearTimer = () => {
    if (timer !== null) {
      cancelTimer(timer);
      timer = null;
    }
  };
  const fire = async (sessionId, cycle) => {
    if (cycle !== generation) return;
    timer = null;
    if (onAttempt) onAttempt(sessionId, failures + 1);
    try {
      await resync(sessionId);
      if (cycle !== generation) return;
      failures = 0;
    } catch (error) {
      if (cycle !== generation) return;
      failures += 1;
      if (failures >= attemptBudget) {
        if (onGiveUp) onGiveUp(sessionId, failures, error);
        return;
      }
      if (onRetry) onRetry(sessionId, failures, error);
      timer = scheduleTimer(() => { fire(sessionId, cycle); }, delayAfterFailures(failures));
    }
  };
  return {
    schedule(sessionId) {
      generation += 1;
      failures = 0;
      clearTimer();
      const cycle = generation;
      timer = scheduleTimer(() => { fire(sessionId, cycle); }, firstDelayMs);
    },
    cancel() {
      generation += 1;
      clearTimer();
    },
  };
}

export function mergeAcpTimelineSnapshot(snapshot, current, sessionId) {
  return (current || [])
    .filter(event => event?.sessionId === sessionId)
    .reduce((merged, event) => appendAcpEvent(merged, event), snapshot || []);
}

export function resolveAcpSessionControls(info) {
  const configOptions = Array.isArray(info && info.config_options)
    ? info.config_options.filter(option => option && option.type === 'select')
    : [];
  const configIds = new Set(configOptions.map(option => String(option.id || '')));
  const modeOption = configOptions.find(option => option.id === 'mode');

  return {
    configOptions,
    effectiveMode: String(
      modeOption && modeOption.currentValue
        || info && info.modes && info.modes.currentModeId
        || '',
    ),
    fallbackModels: configIds.has('model')
      ? []
      : (Array.isArray(info && info.models) ? info.models : []),
    fallbackModes: configIds.has('mode')
      ? null
      : (info && info.modes || null),
  };
}

// ACP elicitation 提交内容构造：answerKey / otherAnswerKey 是 requestedSchema 的 property key，
// 后端仅校验非空，constructor/toString/__proto__ 是合法输入。普通 {} 会让这些键命中
// Object.prototype（尤其 __proto__ 赋值触发 setter，字段在 JSON 序列化时静默丢失）；
// 统一用无原型对象构造，确保 payload 保留全部字段。
export function buildElicitationContent(groups) {
  const content = Object.create(null);
  for (const group of groups) {
    const custom = group.answers.find(answer => answer.other);
    if (custom && group.otherAnswerKey) {
      content[group.otherAnswerKey] = custom.value;
    } else if (group.multiSelect) {
      content[group.answerKey] = group.answers.map(answer => answer.value);
    } else if (group.answers[0]) {
      content[group.answerKey] = group.answers[0].value;
    }
  }
  return content;
}

export {
  commandExecutionDetails,
  contentText,
  mergeTool,
  toolItemType,
};

/**
 * Not used inside this file; only re-exported for consumers.
 * tests/codex_acp_timeline.test.mjs copies this file to a temp directory and
 * dynamically imports the re-export via a computed URL; knip cannot build an
 * edge for that channel, so the `@public` tag keeps it from being removed as a
 * dead export.
 * @public
 */
export {
  stripTerminalControlSequences,
} from '../conversation/conversation-model.js';
