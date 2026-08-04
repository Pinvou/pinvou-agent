/**
 * 子智能体 transcript → ConversationTimeline turn 的适配层。
 *
 * 与 deepseek-conversation.js / acp-state.js 平级同构：纯函数、无 React。
 * 输入是底座落盘的裸 Message 数组（role + content blocks，Anthropic 惯例），
 * 输出是共享对话组件（Codex 式无气泡文档流）能直接渲染的 turn。
 *
 * 两条关键语义（都有单测锁住）：
 * - user 消息分两种：真实任务指令（含正文、无 tool_result）与 tool_result
 *   载体。后者不开新 turn、不产生条目，只把结果回填到同 id 的工具条目上；
 *   照抄"见 user 就开 turn"会把一个子任务切成 N 个假轮次。
 * - transcript 没有任何时间戳，所有 startedAt/completedAt 都是 null；
 *   渲染层据此隐藏时长，不显示假的"0秒"。
 */

import { presentConversationItems } from '../conversation/conversation-model.js';

// CodeWhale 的直接子智能体 id 由 UUID 前 8 位生成（agent_ + 8 位十六进制）。
// 必须按完整契约匹配，不能把工具 schema 里的字段名 `agent_id` 当成实例 id。
const CODEWHALE_SUBAGENT_ID = /\bagent_[0-9a-f]{8}\b/i;

export function extractSubagentId(output) {
  let text;
  if (typeof output === 'string') {
    text = output;
  } else {
    try {
      text = JSON.stringify(output ?? '');
    } catch {
      text = String(output ?? '');
    }
  }
  const match = text.match(CODEWHALE_SUBAGENT_ID);
  return match ? match[0] : null;
}

const FILE_CHANGE_TOOLS = new Set([
  'write_file',
  'edit_file',
  'append_file',
  'apply_patch',
  'fim_edit',
]);
const COMMAND_TOOLS = new Set([
  'exec_shell',
  'exec_shell_wait',
  'exec_shell_interact',
  'exec_wait',
  'exec_interact',
  'task_shell_start',
]);
const OPERATION_TYPES = new Set(['command_execution', 'file_change', 'tool']);

function textBlocksOf(blocks) {
  return blocks
    .filter((block) => block && block.type === 'text' && typeof block.text === 'string')
    .map((block) => block.text)
    .join('\n')
    .trim();
}

/** 含正文且不含 tool_result 的 user 消息才是任务指令。 */
function isTaskInstruction(message, blocks) {
  if (!message || message.role !== 'user') return false;
  if (blocks.some((block) => block && block.type === 'tool_result')) return false;
  return textBlocksOf(blocks).length > 0;
}

function toolItemType(name) {
  if (FILE_CHANGE_TOOLS.has(name)) return 'file_change';
  if (COMMAND_TOOLS.has(name)) return 'command_execution';
  return 'tool';
}

function toolLocations(name, input) {
  if (!input || typeof input !== 'object') return [];
  const path = typeof input.path === 'string' ? input.path.trim() : '';
  return path ? [{ path }] : [];
}

function turnStatusFromAgent(agent) {
  if (!agent || !agent.done) return 'running';
  if (!agent.failed) return 'Completed';
  return agent.status === 'interrupted' ? 'Interrupted' : 'Failed';
}

/**
 * 把裸消息数组整形成单个 turn（一个子智能体 = 一次任务 = 一个 turn）。
 * `agent` 是落盘摘要（mergeAgentSnapshots 输出），提供终态与错误。
 */
export function projectSubagentTranscript({ messages, agent }) {
  const items = [];
  const byToolUseId = new Map();
  let userText = null;

  for (const message of messages || []) {
    if (!message || typeof message !== 'object') continue;
    const blocks = Array.isArray(message.content) ? message.content : [];
    if (message.role === 'user') {
      if (isTaskInstruction(message, blocks)) {
        const text = textBlocksOf(blocks);
        userText = userText == null ? text : `${userText}\n\n${text}`;
        continue;
      }
      for (const block of blocks) {
        if (!block || block.type !== 'tool_result') continue;
        const target = byToolUseId.get(block.tool_use_id);
        if (!target) continue;
        target.tool.rawOutput = block.content;
        target.status = block.is_error ? 'failed' : 'completed';
      }
      continue;
    }
    if (message.role !== 'assistant') continue;
    for (const block of blocks) {
      if (!block || typeof block !== 'object') continue;
      if (block.type === 'text' && typeof block.text === 'string' && block.text.trim()) {
        items.push({
          id: `text-${items.length}`,
          type: 'agent_message',
          text: block.text,
        });
      } else if (block.type === 'thinking' && typeof block.thinking === 'string'
        && block.thinking.trim()) {
        // 落盘字段名是 thinking，时间线条目要求 text。
        items.push({
          id: `reasoning-${items.length}`,
          type: 'reasoning',
          text: block.thinking,
          status: 'completed',
          startedAt: null,
          completedAt: null,
        });
      } else if (block.type === 'tool_use') {
        const name = String(block.name || '').trim();
        const type = toolItemType(name);
        const item = {
          id: block.id || `tool-${items.length}`,
          type,
          status: 'in_progress',
          startedAt: null,
          completedAt: null,
          tool: {
            name,
            title: name,
            kind: type === 'command_execution' ? 'execute' : type === 'file_change' ? 'edit' : 'tool',
            rawInput: block.input,
            rawOutput: null,
            locations: toolLocations(name, block.input),
          },
        };
        items.push(item);
        if (block.id) byToolUseId.set(block.id, item);
      }
    }
  }

  // agent 已终态时不留永远转圈的工具条目（尾部调用可能没等到 receipt）。
  if (agent && agent.done) {
    for (const item of items) {
      if (OPERATION_TYPES.has(item.type) && item.status === 'in_progress') {
        item.status = agent.failed ? 'failed' : 'completed';
      }
    }
  }

  const operationItems = items.filter((item) => OPERATION_TYPES.has(item.type));
  const failedOperationCount = operationItems
    .filter((item) => item.status === 'failed').length;

  const turn = {
    id: (agent && agent.agentId) || 'subagent',
    status: turnStatusFromAgent(agent),
    lifecycleKnown: !!(agent && agent.done),
    startedAt: null,
    completedAt: null,
    error: (agent && agent.error) || null,
    userText,
    items,
    presentation: presentConversationItems(items),
    operationCount: operationItems.length,
    failedOperationCount,
  };
  return { turns: [turn] };
}

/**
 * 子智能体的展示身份：内置四角色用稳定名片（i18n roleCards），`exp-*` 角色
 * 匹配回专家池真卡（名字/部门/头像），无匹配按原样 id 展示、用合成头像。
 *
 * slug 规则与 Rust 侧 roster::expert_role_slug 一致（仅展示用途的镜像：
 * 非 [a-z0-9._-] 折成 '-'，前缀 exp-）；对不上就回退，不会错认。
 */
const SUBAGENT_TYPE_ALIASES = new Set([
  'general', 'general-purpose', 'general_purpose', 'worker', 'default',
  'explore', 'exploration', 'explorer', 'scout',
  'plan', 'planning', 'planner', 'awaiter', 'manager',
  'implementer', 'implement', 'implementation', 'builder',
  'review', 'code-review', 'code_review', 'reviewer',
  'verifier', 'verify', 'verification', 'validator', 'tester',
]);

export function subagentRoleForType(agentType) {
  const normalized = String(agentType || '').trim().toLowerCase();
  if (['explore', 'exploration', 'explorer', 'scout'].includes(normalized)) return 'scout';
  if (['plan', 'planning', 'planner', 'awaiter', 'manager'].includes(normalized)) return 'manager';
  if (['implementer', 'implement', 'implementation', 'builder'].includes(normalized)) return 'builder';
  if (
    ['review', 'code-review', 'code_review', 'reviewer',
      'verifier', 'verify', 'verification', 'validator', 'tester'].includes(normalized)
  ) return 'reviewer';
  return 'general';
}

export function resolveSubagentIdentity(role, personas, agentId, agentType) {
  const rawRole = String(role || '').trim();
  const roleId = rawRole
    ? (SUBAGENT_TYPE_ALIASES.has(rawRole.toLowerCase()) ? subagentRoleForType(rawRole) : rawRole)
    : subagentRoleForType(agentType);
  const builtin = ['scout', 'manager', 'builder', 'reviewer', 'general'];
  // 通用角色卡没有"真人"人设：有 agentId 时头像按实例散列（AppIcon 按 id
  // 哈希 50 张本地头像），同角色派多个实例各有面孔——四个同貌"调研专家"
  // 无法区分（真机截图点名）。专家池成员是具体人设，头像保持人设卡不变。
  const instanceKey = (roleKey) => (agentId ? String(agentId) : roleKey);
  if (!roleId) {
    return { kind: 'builtin', roleKey: 'general', avatarKey: instanceKey('wf-role-general') };
  }
  if (builtin.includes(roleId)) {
    return { kind: 'builtin', roleKey: roleId, avatarKey: instanceKey(`wf-role-${roleId}`) };
  }
  if (roleId.startsWith('exp-')) {
    const match = (personas || []).find((card) => {
      if (!card || !card.id) return false;
      const slug = String(card.id)
        .toLowerCase()
        .replace(/[^a-z0-9._-]/g, '-')
        .replace(/^-+|-+$/g, '');
      return `exp-${slug}` === roleId || roleId.startsWith(`exp-${slug}-`);
    });
    if (match) {
      return {
        kind: 'expert',
        roleKey: null,
        personaId: match.id,
        personaName: match.name,
        personaDept: match.dept,
        avatarKey: match.id,
      };
    }
  }
  return { kind: 'custom', roleKey: null, name: roleId, avatarKey: instanceKey(`wf-role-${roleId}`) };
}

/**
 * 同角色多实例的展示序号：按清单顺序（ledger 登记序，即派出顺序）编号。
 * 行内卡（经轮询事件）与面板（直接读清单）用同一份数据，序号一致。
 */
export function subagentRoleOrdinals(summaries) {
  const counts = new Map();
  const assigned = new Map();
  for (const entry of summaries || []) {
    if (!entry || !entry.agent_id) continue;
    const rawRole = String(entry.role || '').trim();
    const key = rawRole
      ? (SUBAGENT_TYPE_ALIASES.has(rawRole.toLowerCase()) ? subagentRoleForType(rawRole) : rawRole)
      : subagentRoleForType(entry.agent_type);
    const seq = (counts.get(key) || 0) + 1;
    counts.set(key, seq);
    assigned.set(entry.agent_id, { key, seq });
  }
  const out = new Map();
  for (const [agentId, { key, seq }] of assigned) {
    out.set(agentId, { seq, count: counts.get(key) });
  }
  return out;
}

const ORDINAL_GLYPHS = ['①', '②', '③', '④', '⑤', '⑥', '⑦', '⑧', '⑨', '⑩'];

/** 序号后缀：同角色仅一个实例不加；超出 ⑩ 回退 #N。 */
export function subagentOrdinalLabel(ordinal) {
  if (!ordinal || !(ordinal.count > 1)) return '';
  return ` ${ORDINAL_GLYPHS[ordinal.seq - 1] || `#${ordinal.seq}`}`;
}

/**
 * 模型给子智能体起的名字：任务说明第一行以「名字」开头（委派提醒教学的
 * 约定，如「调研专家-AI新闻」）。底座 role 字段只收 ASCII token，中文名
 * 走不了字段，只能走文本约定；没起名回退角色映射名+序号。上限 24 字，
 * 防止整段说明被吞进标题。
 */
export function splitSubagentTitle(text) {
  const raw = String(text || '');
  const match = raw.match(/^\s*「([^」\n]{1,24})」\s*[:：、\-—]?\s*/);
  if (!match || !match[1].trim()) return { name: null, rest: raw };
  return { name: match[1].trim(), rest: raw.slice(match[0].length) };
}

/**
 * 模型没有填写 agent.name、也没有按「名称」约定写标题时，从它自己写出的
 * objective 第一条有效任务语句提炼一个短名称。这里只做确定性的展示投影，
 * 不另起一次模型调用；因此普通对话临时派出的裸 agent 也不会退回成三张
 * 一模一样的“通用执行者”卡。
 */
export function subagentObjectiveName(text, maxLength = 24) {
  const lines = String(text || '')
    .split(/\r?\n/)
    .map(line => line.trim())
    .filter(Boolean);
  if (!lines.length) return null;

  const marker = /^(?:[-*#>]\s*)*(?:question|task|objective|goal|assignment|任务|目标|问题)\s*[:：]\s*(.+)$/i;
  const marked = lines.map(line => line.match(marker)).find(Boolean);
  let candidate = marked ? marked[1] : lines.find(line => {
    const normalized = line.replace(/^(?:[-*#>]\s*)+/, '').trim();
    return normalized
      && !/^(?:assignment metadata|scope|already_known|effort|stop_condition|context)\s*[:：]?$/i.test(normalized)
      && !/^<\/?codewhale:/i.test(normalized);
  });
  if (!candidate) return null;

  candidate = candidate
    .replace(/^(?:[-*#>]\s*)+/, '')
    .replace(/^(?:question|task|objective|goal|assignment|任务|目标|问题)\s*[:：]\s*/i, '')
    .replace(/\s+/g, ' ')
    .trim();
  if (!candidate) return null;
  const characters = [...candidate];
  return characters.length > maxLength
    ? `${characters.slice(0, maxLength).join('')}…`
    : candidate;
}

/**
 * 行内专家卡与右侧面板共用的名称决策。
 *
 * 优先级：专家池真名 > 模型显式 name/session_name > 任务首行「名称」 >
 * agent type 对应的本地化角色 > 通用角色。底座未显式起名时会把 agent_id
 * 写入 session_name，这种占位值不能暴露给用户。
 */
export function resolveSubagentPresentation({
  role,
  agentType,
  sessionName,
  objective,
  personas,
  agentId,
  roleCards,
  ordinal,
}) {
  const identity = resolveSubagentIdentity(role, personas, agentId, agentType);
  const title = splitSubagentTitle(objective || '');
  const rawSessionName = String(sessionName || '').trim();
  const modelName = rawSessionName && rawSessionName !== String(agentId || '')
    ? rawSessionName
    : null;
  const explicitName = identity.kind === 'expert' ? null : (modelName || title.name);
  const objectiveName = identity.kind === 'expert' || explicitName
    ? null
    : subagentObjectiveName(objective);
  const baseName = identity.kind === 'expert'
    ? identity.personaName
    : explicitName
      || objectiveName
      || (identity.kind === 'custom'
        ? identity.name
        : ((roleCards && roleCards[identity.roleKey])
          || (roleCards && roleCards.general)
          || identity.roleKey));
  return {
    identity,
    name: baseName + (explicitName ? '' : subagentOrdinalLabel(ordinal)),
    task: (title.name ? title.rest : String(objective || '')).trim(),
    explicitName,
    objectiveName,
  };
}

/**
 * 从 edit_file / apply_patch 的结果正文（unified diff）里数出 +N -M。
 * write_file 没有 diff，返回 null，只显示路径。
 */
export function fileChangeStat(rawOutput) {
  if (typeof rawOutput !== 'string' || !rawOutput.includes('@@')) return null;
  let added = 0;
  let removed = 0;
  for (const line of rawOutput.split(/\r?\n/)) {
    if (line.startsWith('+++') || line.startsWith('---')) continue;
    if (line.startsWith('+')) added += 1;
    else if (line.startsWith('-')) removed += 1;
  }
  if (added === 0 && removed === 0) return null;
  return { added, removed };
}
