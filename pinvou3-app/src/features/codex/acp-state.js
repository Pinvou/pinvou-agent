import {
  commandExecutionDetails,
  presentConversationItems,
  stripTerminalControlSequences,
} from '../conversation/conversation-model.js';

export function unifiedConversationUiEnabled() {
  try {
    return localStorage.getItem('pinvou_conversation_ui_v2') !== 'false';
  } catch {
    return true;
  }
}

export function updateAcpAttachmentDraft(drafts, attachmentId, update) {
  for (const [owner, attachments] of Object.entries(drafts || {})) {
    if (!attachments.some(attachment => attachment.id === attachmentId)) continue;
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
    content: update.content !== undefined ? update.content : current.content,
    locations: update.locations !== undefined ? update.locations : current.locations,
    rawInput: update.rawInput !== undefined ? update.rawInput : current.rawInput,
    rawOutput: update.rawOutput !== undefined ? update.rawOutput : current.rawOutput,
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
      turn.error = data.error || null;
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
  stripTerminalControlSequences,
  toolItemType,
};
