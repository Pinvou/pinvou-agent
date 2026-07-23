function contentText(content) {
  if (!content) return '';
  if (typeof content === 'string') return content;
  if (content.type === 'text') return String(content.text || '');
  if (content.text != null) return String(content.text);
  return '';
}

const ESC = String.fromCharCode(0x1b);
const BEL = String.fromCharCode(0x07);
const C1_CSI = String.fromCharCode(0x9b);
const OSC_SEQUENCE = new RegExp(`${ESC}\\][\\s\\S]*?(?:${BEL}|${ESC}\\\\)`, 'g');
const CSI_SEQUENCE = new RegExp(`(?:${ESC}\\[|${C1_CSI})[0-?]*[ -/]*[@-~]`, 'g');
const SINGLE_ESCAPE_SEQUENCE = new RegExp(`${ESC}[()][0-2A-Z]`, 'g');

/**
 * ACP 命令输出保留的是终端原始文本，其中可能包含颜色、光标和超链接控制码。
 * 浏览器不是终端，渲染前必须清理这些序列，否则 ESC 会显示成方框乱码。
 */
export function stripTerminalControlSequences(value) {
  return String(value ?? '')
    .replace(OSC_SEQUENCE, '')
    .replace(CSI_SEQUENCE, '')
    .replace(SINGLE_ESCAPE_SEQUENCE, '');
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
    usage: null,
    status: 'idle',
    error: null,
    startedAt: null,
    completedAt: null,
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
      return {
        ...block,
        type: toolItemType(block.tool),
        status: block.tool && block.tool.status || 'pending',
      };
    }
    if (block.type === 'permission') {
      return {
        ...block,
        status: block.permission.resolved ? 'completed' : 'waiting',
        completedAt: block.permission.resolvedAt || null,
      };
    }
    if (block.type === 'plan') {
      return { ...block, status: turn.completedAt ? 'completed' : 'in_progress', completedAt: turn.completedAt };
    }
    return block;
  });
}

const OPERATION_ITEM_TYPES = new Set(['command_execution', 'file_change', 'tool']);

/**
 * Item 是事实语义，presentation 只控制视觉聚合。工具组不会改写、合并或丢弃
 * 任何 Item；展开后仍按原始时序逐项展示。
 */
export function presentTurnItems(items) {
  const result = [];
  for (const item of items || []) {
    if (OPERATION_ITEM_TYPES.has(item.type)) {
      const previous = result[result.length - 1];
      if (previous && previous.type === 'tool_group') {
        previous.items.push(item);
        continue;
      }
      result.push({
        id: `tool-group-${item.id}`,
        type: 'tool_group',
        items: [item],
      });
      continue;
    }
    result.push(item);
  }
  return result;
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
    turn.items = normalizeTurnItems(turn);
    turn.presentation = presentTurnItems(turn.items);
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

export function commandExecutionDetails(tool) {
  const rawInput = tool && tool.rawInput;
  const rawOutput = tool && tool.rawOutput;
  const command = rawInput && typeof rawInput === 'object' && rawInput.command != null
    ? stripTerminalControlSequences(rawInput.command)
    : stripTerminalControlSequences(tool && tool.title || '');
  const cwd = rawInput && typeof rawInput === 'object' && rawInput.cwd != null
    ? stripTerminalControlSequences(rawInput.cwd)
    : '';
  let output = '';
  let exitCode = null;
  if (typeof rawOutput === 'string') {
    output = rawOutput;
  } else if (rawOutput && typeof rawOutput === 'object') {
    output = String(
      rawOutput.formatted_output
        ?? rawOutput.output
        ?? rawOutput.text
        ?? '',
    );
    const code = rawOutput.exit_code ?? rawOutput.exitCode;
    if (code !== undefined && code !== null && code !== '') exitCode = Number(code);
  }
  if (!output && tool && typeof tool.content === 'string') output = tool.content;
  output = stripTerminalControlSequences(output);
  const commandLines = command.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  return {
    command,
    cwd,
    output,
    exitCode: Number.isNaN(exitCode) ? null : exitCode,
    summary: commandLines[0] || String(tool && tool.title || '执行 Shell 命令'),
    commandCount: commandLines.length,
  };
}

export function appendAcpEvent(events, incoming) {
  if (!incoming) return events || [];
  if ((events || []).some(event => event.sessionId === incoming.sessionId && event.seq === incoming.seq)) {
    return events;
  }
  return [...(events || []), incoming].sort((a, b) => Number(a.seq || 0) - Number(b.seq || 0));
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

export { contentText, mergeTool, toolItemType };
