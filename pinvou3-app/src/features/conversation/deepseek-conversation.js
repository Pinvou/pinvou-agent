import { presentConversationItems } from './conversation-model.js';

const SHELL_TOOLS = new Set([
  'exec_shell',
  'exec_shell_wait',
  'exec_wait',
  'task_shell_start',
  'task_shell_wait',
  'shell',
]);

function stableItemId(item, index) {
  return `deepseek-${item && item.id != null ? item.id : index}`;
}

function toolStatus(item) {
  if (item && item.state === 'running') return 'in_progress';
  if (item && (item.success === false || item.state === 'failed')) return 'failed';
  if (item && (item.state === 'done' || item.success === true)) return 'completed';
  return 'pending';
}

function projectItem(item, index) {
  const id = stableItemId(item, index);
  if (item.type === 'assistant') {
    return {
      id,
      type: 'agent_message',
      status: item.streaming ? 'in_progress' : 'completed',
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
    usage: null,
    status: 'completed',
    error: null,
    startedAt: null,
    completedAt: null,
  };
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
} = {}) {
  const turns = [];
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
      current = emptyTurn(`deepseek-${sessionId || 'session'}-turn-${item.id != null ? item.id : index}`);
      current.userItem = item;
      current.userText = String(item.text || '');
      turns.push(current);
      continue;
    }
    ensureTurn(index).items.push(projectItem(item, index));
  }

  for (const turn of turns) {
    turn.presentation = presentConversationItems(turn.items);
    turn.waitingPermission = turn.items.some(item => (
      ['permission', 'user_input'].includes(item.type)
      && item.legacyItem
      && item.legacyItem.resolved === false
    ));
  }

  const activeTurn = turns[turns.length - 1];
  if (activeTurn && busy) {
    activeTurn.status = 'running';
    activeTurn.startedAt = thinking && thinking.startedAt || Date.now();
    activeTurn.activityLabel = thinking && thinking.phase === 'tool' && thinking.toolName
      ? `正在调用 ${thinking.toolName}`
      : '正在处理';
  }
  if (activeTurn && tokens && tokens.max > 0) {
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
