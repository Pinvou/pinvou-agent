// 代码模块原生（品悟 Engine）会话的本地会话车道。
//
// ACP 会话由后端维护 timeline（get_codex_acp_timeline）；原生会话复用主聊天的
// engine 链路：chat 命令发消息、`chat:*` 事件推进、SavedSession messages 落盘。
// 本模块把一个会话的展示状态（chatItems/busy/thinking/tokens/memory/turn timeline）
// 收敛成纯数据 lane，便于 React 侧按 session 缓存与单测；渲染统一走
// projectDeepSeekConversation → ConversationTimeline。
//
// lane.items 是 bridge chatItems 的兼容子集：user / assistant(text) / reasoning /
// tool / user_input / careful_blocked / system / plan_card。与 bridge 的差异：assistant 保留
// 原始 markdown 文本（bridge 存预渲染 html），渲染层用 ConversationMarkdown；
// plan_card 的终态文案存 statusKey（approved/discarded/superseded/historical），
// 三语文案在渲染层按 key 组装（与 compactPhase 同一约定）。

import { projectDeepSeekConversation } from '../conversation/deepseek-conversation.js';
import { isInternalRuntimeEnvelopeText, isInternalUserMessage } from '../../shared/internal-message.mjs';

export function createNativeLane() {
  return {
    hydrated: false,
    items: [],
    busy: false,
    thinking: null,
    tokens: { input: 0, max: 0 },
    timeline: [],
    streamId: 0,
    streamText: '',
    toolMeta: {},
    planSnapshot: { plan: null, todos: null },
    // chat:memory 推送的本回合注入记忆快照（{ items, runtimePath, updatedAt }），
    // 未收到过事件时为 null；会话级状态，不随 hydration 落盘/清空。
    memory: null,
    // chat:compaction phase=start → true，done/fail → false；用于禁用手动压缩入口。
    compacting: false,
    seq: 0,
  };
}

function nextId(lane) {
  lane.seq += 1;
  return lane.seq;
}

function timeStr() {
  return new Date().toTimeString().slice(0, 5);
}

// ── Plan 审批（语义镜像 bridge chat-events.js 的 plan_snapshot/plan_ready）─────
// plan 类工具：hydrate 时不还原工具卡，改在本条 assistant 消息末尾还原只读方案卡
// （对齐 bridge rerenderFromMessages 的 PLAN_TOOLS 处理）。
const PLAN_TOOLS = ['update_plan', 'checklist_write', 'todo_write'];

/// plan 类工具结果格式："...updated:\n{json}"——切第一个换行后 parse（对齐 bridge
/// parsePlanSnapshot / engine.rs）。content 可能是 string 或 Anthropic blocks 数组。
export function parseNativePlanSnapshot(content) {
  const text = typeof content === 'string'
    ? content
    : (Array.isArray(content) ? content.map(block => (block && typeof block.text === 'string' ? block.text : '')).join('') : '');
  const newline = text.indexOf('\n');
  if (newline < 0) return null;
  try { return JSON.parse(text.slice(newline + 1)); } catch { return null; }
}

/// tool_result.content 归一成纯文本（对齐 bridge toolResultText）。
function toolResultText(content) {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content.map(block => (block && typeof block.text === 'string' ? block.text : '')).join('');
  }
  return '';
}

/// request_user_input 结果是纯 JSON {answers:[{id,label,value}]}（turn_loop.rs ToolResult::json）。
/// 按 question.id 匹配，还原成 QuestionChoiceCard 的 answers 数组（顺序对齐 questions，
/// 未命中的问题占 null，渲染层过滤）。multi_select 多选保留全部同 id 答案、不塌缩，
/// 与提交时 markNativeInputResolved 存的全量数组一致。
function parseNativeUserAnswers(content, questions) {
  let ans;
  try { ans = JSON.parse(toolResultText(content)).answers; } catch { return null; }
  if (!Array.isArray(ans)) return null;
  // 用无原型对象：question id 仅被后端校验非空，constructor/toString/__proto__ 是合法输入，
  // 普通 {} 会让这些键命中 Object.prototype 继承属性，.push 抛 TypeError（复核 P1）。
  const byId = Object.create(null);
  ans.forEach(a => { if (a && a.id != null) (byId[a.id] = byId[a.id] || []).push(a); });
  const out = [];
  for (const q of questions) {
    const matches = byId[q.id];
    if (!matches || !matches.length) { out.push(null); continue; }
    matches.forEach(a => out.push({ id: q.id, label: a.label, value: a.value }));
  }
  return out;
}

/// accept_plan 的 plan_markdown 拼法（对齐 bridge composePlanMarkdown）：这段文本会进
/// 后端执行指令（LLM 面向），标签保持中文，不随界面语言。
export function composeNativePlanMarkdown(snapshots) {
  const lines = [];
  const plan = snapshots && snapshots.plan;
  const todos = snapshots && snapshots.todos;
  const sym = status => (status === 'completed' ? '●' : status === 'in_progress' ? '◎' : '○');
  if (plan && Array.isArray(plan.items)) {
    if (plan.explanation) lines.push('**方案：**', plan.explanation, '');
    lines.push('**步骤：**');
    plan.items.forEach((item, index) => lines.push(`${index + 1}. ${sym(item.status)} ${item.step}`));
    lines.push('');
  }
  if (todos && Array.isArray(todos.items)) {
    lines.push('**细分待办：**');
    todos.items.forEach((item, index) => lines.push(`${index + 1}. ${sym(item.status)} ${item.content}`));
  }
  return lines.length > 0 ? lines.join('\n') : '（plan 为空）';
}

/// 渲染层往 lane 追加系统提示项（accept/discard 失败等），对齐 bridge addSystemItem。
export function appendNativeSystemItem(lane, text) {
  lane.items.push({ id: nextId(lane), type: 'system', text: String(text || ''), time: timeStr() });
}

/// plan_card 状态迁移（批准/放弃/新方案覆盖），供事件与视图动作共用。
function resolvePlanCard(card, cardState, statusKey) {
  card.cardState = cardState;
  card.resolved = true;
  card.statusKey = statusKey;
}

function visibleUserTurnIndex(lane) {
  const count = lane.items.filter(item => item && item.type === 'user').length;
  return Math.max(0, count - 1);
}

function openTimelineStart(lane, withinMs = 0) {
  const open = [...lane.timeline]
    .reverse()
    .find(event => event.event === 'user_start'
      && !lane.timeline.some(other => other.event === 'assistant_done' && other.turn_id === event.turn_id));
  if (!open) return null;
  if (withinMs > 0 && Math.abs(Date.now() - Number(open.timestamp || 0)) > withinMs) return null;
  return open;
}

function recordTurnStarted(lane, turnId) {
  lane.timeline.push({
    turn_id: turnId || `ui_native_${Date.now()}`,
    event: 'user_start',
    timestamp: Date.now(),
    ui_turn_index: visibleUserTurnIndex(lane),
  });
}

function recordTurnCompleted(lane, payload) {
  const open = openTimelineStart(lane);
  if (!open) return;
  lane.timeline.push({
    turn_id: open.turn_id,
    event: 'assistant_done',
    timestamp: Date.now(),
    status: payload && payload.status || (payload && payload.error ? 'Failed' : 'Completed'),
    error: payload && payload.error || null,
    ui_turn_index: open.ui_turn_index,
  });
}

function finalizeStream(lane) {
  if (!lane.streamId) return;
  const item = lane.items.find(candidate => candidate.id === lane.streamId);
  if (item) item.streaming = false;
  lane.streamId = 0;
  lane.streamText = '';
}

function finalizeReasoning(lane) {
  const completedAt = Date.now();
  for (const item of lane.items) {
    if (item && item.type === 'reasoning' && item.streaming) {
      item.streaming = false;
      item.completedAt = completedAt;
    }
  }
}

/// 发送前乐观插入用户气泡并记录 turn 起点；chat 命令同步失败时用
/// removeLocalUserMessage 回滚。返回临时 item id。
export function appendLocalUserMessage(lane, text) {
  const id = nextId(lane);
  lane.items.push({ id, type: 'user', text: String(text || ''), time: timeStr(), localEchoTs: Date.now() });
  recordTurnStarted(lane);
  lane.busy = true;
  lane.thinking = { active: true, startedAt: Date.now(), phase: 'thinking', toolName: null };
  return id;
}

export function removeLocalUserMessage(lane, id) {
  lane.items = lane.items.filter(item => item.id !== id);
  // 该 turn 未被 engine 接纳（不会有 assistant_done），把乐观记录的 user_start 一并回滚。
  const open = openTimelineStart(lane);
  if (open) lane.timeline = lane.timeline.filter(event => event !== open);
  lane.busy = false;
  lane.thinking = null;
}

/// chat:* 事件 → lane 状态。payload 一律带 session_id（后端 forwarder 打 tag）。
/// 返回是否有可视变化；无变化时 React 侧不必 bump 渲染。
export function applyNativeChatEvent(lane, name, payload) {
  const p = payload || {};
  switch (name) {
    case 'chat:user_message': {
      const content = String(p.content || '');
      if (!content) return false;
      // 内部运行时信封（subagent handoff / background shell 完成等）：与 bridge 实时
      // 路径一致不上屏；后续 transcript 重载同样会被 hydrate 过滤，两条路径行为对齐。
      if (isInternalRuntimeEnvelopeText(content)) return false;
      // accept_plan 的用户回声（本地/远端批准都会广播）：先把命中的 active 方案卡
      // 置为已批准（对齐 bridge chat-events.js 的 action === "accept_plan" 处理），
      // 再走普通用户消息去重/插入。
      let changed = false;
      if (String(p.action || '') === 'accept_plan') {
        const actionPlanId = String(p.plan_id || p.planId || '').trim();
        lane.items.forEach(item => {
          if (item && item.type === 'plan_card' && item.cardState === 'active' && !item.resolved
              && (!actionPlanId || String(item.planId || '') === actionPlanId)) {
            resolvePlanCard(item, 'approved', 'approved');
            changed = true;
          }
        });
      }
      const lastUser = [...lane.items].reverse().find(item => item && item.type === 'user');
      if (lastUser) {
        // 本地乐观插入已覆盖：文本一致，或刚发送（本地气泡带 📎 附件名等展示
        // 修饰，与后端回声文本不同）30 秒内视为同一消息的回声。
        if (lastUser.text === content
          || (lastUser.localEchoTs && Date.now() - lastUser.localEchoTs < 30000)) {
          delete lastUser.localEchoTs;
          return changed;
        }
      }
      lane.items.push({ id: nextId(lane), type: 'user', text: content, time: timeStr() });
      recordTurnStarted(lane);
      lane.busy = true;
      lane.thinking = { active: true, startedAt: Date.now(), phase: 'thinking', toolName: null };
      return true;
    }
    case 'chat:turn_started': {
      lane.busy = true;
      if (!lane.thinking || !lane.thinking.active) {
        lane.thinking = { active: true, startedAt: Date.now(), phase: 'thinking', toolName: null };
      }
      // 本地乐观插入 / chat:user_message 已记录起点时，60 秒内复用不重复记。
      if (!openTimelineStart(lane, 60000)) recordTurnStarted(lane, p.turn_id);
      return true;
    }
    case 'chat:reasoning_start': {
      finalizeStream(lane);
      finalizeReasoning(lane);
      lane.items.push({
        id: nextId(lane),
        type: 'reasoning',
        text: '',
        streaming: true,
        startedAt: Date.now(),
        completedAt: null,
      });
      return true;
    }
    case 'chat:reasoning_delta': {
      const text = String(p.text || '');
      if (!text) return false;
      let item = [...lane.items].reverse().find(candidate => (
        candidate && candidate.type === 'reasoning' && candidate.streaming
      ));
      if (!item) {
        applyNativeChatEvent(lane, 'chat:reasoning_start', p);
        item = lane.items[lane.items.length - 1];
      }
      item.text += text;
      return true;
    }
    case 'chat:reasoning_done': {
      finalizeReasoning(lane);
      lane.items = lane.items.filter(item => !(
        item && item.type === 'reasoning' && !item.streaming && !item.text
      ));
      return true;
    }
    case 'chat:delta': {
      const text = String(p.text || '');
      if (!text) return false;
      finalizeReasoning(lane);
      lane.streamText += text;
      const existing = lane.items.find(item => item.id === lane.streamId);
      if (existing) {
        existing.text = lane.streamText;
        existing.streaming = true;
      } else {
        lane.streamId = nextId(lane);
        lane.items.push({
          id: lane.streamId,
          type: 'assistant',
          text: lane.streamText,
          time: timeStr(),
          streaming: true,
        });
      }
      return true;
    }
    case 'chat:tool_start': {
      if (!p.id) return false;
      lane.toolMeta[p.id] = { name: p.name, args: p.args };
      finalizeReasoning(lane);
      finalizeStream(lane);
      lane.thinking = { active: true, startedAt: lane.thinking?.startedAt || Date.now(), phase: 'tool', toolName: p.name || null };
      // request_user_input 不渲染工具卡，等 chat:user_input_required 的选择卡片。
      if (p.name === 'request_user_input') return true;
      if (lane.items.some(item => item && item.type === 'tool' && item.toolId === p.id)) return false;
      lane.items.push({
        id: nextId(lane),
        type: 'tool',
        toolId: p.id,
        name: p.name || '',
        args: p.args,
        output: null,
        success: null,
        state: 'running',
      });
      return true;
    }
    case 'chat:tool_delta': {
      const item = [...lane.items].reverse().find(candidate => (
        candidate && candidate.type === 'tool' && candidate.toolId === p.id
      ));
      if (!item || !p.content) return false;
      item.output = String(item.output || '') + String(p.content);
      return true;
    }
    case 'chat:tool_end': {
      const meta = lane.toolMeta[p.id];
      delete lane.toolMeta[p.id];
      lane.thinking = lane.busy
        ? { active: true, startedAt: lane.thinking?.startedAt || Date.now(), phase: 'thinking', toolName: null }
        : null;
      // remount 恢复的 active 卡（pending → chat:user_input_required）没有经过
      // tool_start，toolMeta 缺失；后端 tool_end payload 自带 name，用它兜底判断，
      // 避免超时/收口时落入普通工具分支导致卡片不收口。
      if ((meta?.name || p.name) === 'request_user_input') {
        const card = [...lane.items].reverse().find(item => (
          item && item.type === 'user_input' && item.toolCallId === p.id && !item.resolved
        ));
        if (card) {
          card.resolved = true;
          card.cardState = p.success ? 'submitted' : 'cancelled';
        }
        return true;
      }
      const item = [...lane.items].reverse().find(candidate => (
        candidate && candidate.type === 'tool' && candidate.toolId === p.id
      ));
      if (item) {
        item.output = typeof p.output === 'string' ? p.output : JSON.stringify(p.output);
        item.success = Boolean(p.success);
        item.state = 'done';
      }
      // Careful 拦截：metadata.safety_level==='dangerous' 且 blocked → 拦截提示卡。
      const md = p.metadata;
      if (md && md.safety_level === 'dangerous' && md.blocked) {
        lane.items.push({ id: nextId(lane), type: 'careful_blocked', args: meta && meta.args, metadata: md, time: timeStr() });
      }
      return true;
    }
    case 'chat:usage': {
      const input = Number(p.input_tokens || 0);
      if (input <= 0) return false;
      lane.tokens = { input, max: lane.tokens.max };
      return true;
    }
    case 'chat:user_input_required': {
      const questions = Array.isArray(p.questions) ? p.questions : [];
      if (!p.id || !questions.length) return false;
      const existing = lane.items.find(item => (
        item && item.type === 'user_input' && item.toolCallId === p.id
      ));
      if (existing) {
        // 同 id 卡片已存在：未解决 → 无需重复；已 resolved（历史快照误标为
        // submitted 的进行中提问）→ 重置为 active，让用户仍能选择。
        if (!existing.resolved) return false;
        existing.resolved = false;
        existing.cardState = 'active';
        existing.questions = questions;
        return true;
      }
      lane.items.push({
        id: nextId(lane),
        type: 'user_input',
        toolCallId: p.id,
        questions,
        resolved: false,
        cardState: 'active',
        time: timeStr(),
      });
      return true;
    }
    case 'chat:transient_error': {
      if (!p.error) return false;
      const notice = `⚠️ ${p.error}`;
      if (lane.items.some(item => item && item.type === 'system' && item.text === notice)) return false;
      lane.items.push({ id: nextId(lane), type: 'system', text: notice, time: timeStr() });
      return true;
    }
    case 'chat:shell_task_status': {
      // 后台 shell 任务终态（语义对齐 bridge finishBackgroundToolItem）：
      // 把对应工具卡更新为最终状态并合并 stdout/stderr 尾段。
      const item = [...lane.items].reverse().find(candidate => (
        candidate && candidate.type === 'tool' && candidate.toolId === p.tool_id
      ));
      if (!item) return false;
      const status = String(p.status || 'Failed');
      const success = status === 'Completed';
      item.success = success;
      item.state = success ? 'done' : 'failed';
      item.exitCode = p.exit_code ?? null;
      const tail = [p.stdout_tail, p.stderr_tail && `[STDERR] ${p.stderr_tail}`]
        .filter(Boolean)
        .join('\n');
      if (tail) item.output = item.output ? `${item.output}\n${tail}` : tail;
      return true;
    }
    case 'chat:compaction': {
      // 压缩事件渲染为系统提示项；三语文案在渲染层按 compactPhase 组装。
      const phase = String(p.phase || 'done');
      lane.compacting = phase === 'start';
      lane.items.push({
        id: nextId(lane),
        type: 'system',
        compactPhase: phase,
        text: String(p.message || ''),
        time: timeStr(),
      });
      return true;
    }
    case 'chat:memory': {
      // 每轮 chat 后后端推送的本会话注入记忆快照（chat.rs 对全部会话发射）。
      // 只归一化 id/kind/text 三字段，渲染层做轻量展示（条数徽标 + 弹层列表）。
      const items = (Array.isArray(p.items) ? p.items : [])
        .map(item => ({
          id: String(item && item.id || ''),
          kind: String(item && item.kind || ''),
          text: String(item && item.text || ''),
        }))
        .filter(item => item.text);
      lane.memory = { items, runtimePath: String(p.runtime_path || ''), updatedAt: Date.now() };
      return true;
    }
    case 'chat:plan_snapshot': {
      // update_plan/checklist_write 后实时更新快照（只带本次改的那份，另一份为 null）。
      let changed = false;
      if (p.plan_snapshot) { lane.planSnapshot.plan = p.plan_snapshot; changed = true; }
      if (p.todos_snapshot) { lane.planSnapshot.todos = p.todos_snapshot; changed = true; }
      return changed;
    }
    case 'chat:plan_ready': {
      // Plan 模式调过 update_plan → 弹方案审批卡（对齐 bridge：快照可空、无 plan_id
      // 时按只读历史卡处理）。
      const planId = String(p.plan_id || p.planId || '').trim();
      if (planId && lane.items.some(item => (
        item && item.type === 'plan_card' && String(item.planId || '') === planId
      ))) return false;
      // 新方案出现 → 旧的 active 方案卡冻结（已被新方案覆盖）。
      lane.items.forEach(item => {
        if (item && item.type === 'plan_card' && item.cardState === 'active') {
          resolvePlanCard(item, 'frozen', 'superseded');
        }
      });
      const snaps = { plan: p.plan_snapshot || null, todos: p.todos_snapshot || null };
      lane.items.push({
        id: nextId(lane),
        type: 'plan_card',
        plan: snaps.plan,
        todos: snaps.todos,
        planMarkdown: composeNativePlanMarkdown(snaps),
        planId: planId || null,
        cardState: planId ? 'active' : 'frozen',
        resolved: !planId,
        statusKey: planId ? '' : 'historical',
        time: timeStr(),
      });
      return true;
    }
    case 'chat:done': {
      finalizeReasoning(lane);
      finalizeStream(lane);
      recordTurnCompleted(lane, p);
      lane.busy = false;
      lane.thinking = null;
      if (p.error) {
        lane.items.push({ id: nextId(lane), type: 'system', text: `⚠️ ${p.error}`, time: timeStr() });
      }
      return true;
    }
    case 'chat:plan_resolved': {
      // discard_plan 后端广播（本地已乐观冻结，此为多端/远端回声同步）：把匹配的
      // active 方案卡幂等冻结为 discarded（对齐 bridge chat-events.js plan_resolved）。
      const planId = String(p.plan_id || p.planId || '').trim();
      if (!planId) return false;
      let changed = false;
      lane.items.forEach(item => {
        if (item && item.type === 'plan_card' && String(item.planId || '') === planId
            && item.cardState === 'active' && !item.resolved) {
          resolvePlanCard(item, 'frozen', 'discarded');
          changed = true;
        }
      });
      return changed;
    }
    default:
      return false;
  }
}

function messageText(blocks) {
  return blocks
    .filter(block => block && block.type === 'text' && block.text)
    .map(block => String(block.text))
    .join('\n')
    .trim();
}

// 与 platform/{tauri,web}/bridge.js 的 userMessageDisplayText 判定保持一致：
// CodeWhale 内部运行时信封（subagent handoff / background shell 完成等）以
// role=user 持久化供父模型上下文使用，展示层不得渲染为用户气泡。
// 共享 ESM 实现见 src/shared/internal-message.mjs（bridge 闭包不可 import）。

/// SavedSession messages → lane.items（hydration 是 rerenderFromMessages 的精简版：
/// 覆盖 user / assistant text / thinking / tool_use+tool_result / request_user_input /
/// plan 工具的历史方案卡；persona、成品卡等主聊天专属形态不在代码会话出现，不做还原）。
/// 方案卡降级语义与 work 冷启动对齐：只还原**只读历史卡**（planId 为空、不可批准）——
/// 后端没有按会话查询待批方案快照的接口（mode_state 只有 pending_plan_id，work 侧也不读），
/// 待批方案跨 remount 不再可点批准，用户让 AI 重出方案即可。
export function hydrateNativeLane(lane, saved, timelineEvents = []) {
  // 同窗口切回正在跑的会话时，lane 已被 chat:* 事件推进过：磁盘快照（只落已提交
  // 内容）会滞后于实时状态，hydration 后保留 busy，由后续事件继续推进；冷启动
  // 首次 hydration 时 lane 无任何 live 痕迹，未配对的 user_start 只能按中断展示。
  const hadLiveTurn = Boolean(
    lane.busy
      || lane.streamId
      || (lane.thinking && lane.thinking.active)
      || Object.keys(lane.toolMeta).length > 0,
  );
  const messages = saved && Array.isArray(saved.messages) ? saved.messages : [];
  const resultById = {};
  for (const message of messages) {
    const blocks = Array.isArray(message && message.content) ? message.content : [];
    for (const block of blocks) {
      if (block && block.type === 'tool_result') {
        resultById[block.tool_use_id] = { content: block.content, is_error: Boolean(block.is_error) };
      }
    }
  }
  lane.items = [];
  lane.streamId = 0;
  lane.streamText = '';
  lane.toolMeta = {};
  // planSnapshot 是 live 进度（磁盘无对应物），随全量重载清空；历史方案由下方卡片还原。
  lane.planSnapshot = { plan: null, todos: null };
  for (const message of messages) {
    const role = message && message.role;
    const raw = message && message.content;
    const blocks = Array.isArray(raw)
      ? raw
      : (typeof raw === 'string' && raw ? [{ type: 'text', text: raw }] : []);
    if (role === 'user') {
      if (isInternalUserMessage(blocks)) continue; // 内部运行时信封/交接：保留在模型上下文，不上屏
      const text = messageText(blocks);
      if (text) lane.items.push({ id: nextId(lane), type: 'user', text, time: '' });
      for (const block of blocks) {
        if (!block || block.type !== 'tool_result') continue;
        const item = [...lane.items].reverse().find(candidate => (
          candidate && candidate.type === 'tool' && candidate.toolId === block.tool_use_id
        ));
        if (item) {
          item.output = typeof block.content === 'string' ? block.content : JSON.stringify(block.content);
          item.success = !block.is_error;
          item.state = 'done';
        }
      }
      continue;
    }
    if (role !== 'assistant') continue;
    let textBuf = '';
    let planSnap = null;
    let todosSnap = null;
    let sawPlanTool = false;
    const flushText = () => {
      if (!textBuf) return;
      lane.items.push({ id: nextId(lane), type: 'assistant', text: textBuf, time: '', streaming: false });
      textBuf = '';
    };
    for (const block of blocks) {
      if (!block) continue;
      if (block.type === 'text') {
        textBuf += block.text || '';
      } else if (block.type === 'thinking') {
        flushText();
        const reasoning = String(block.thinking || block.text || '');
        if (reasoning) {
          lane.items.push({ id: nextId(lane), type: 'reasoning', text: reasoning, streaming: false, startedAt: null, completedAt: null });
        }
      } else if (block.type === 'tool_use') {
        flushText();
        if (block.name === 'request_user_input') {
          const questions = (block.input && block.input.questions) || [];
          if (Array.isArray(questions) && questions.length) {
            const result = resultById[block.id];
            // 磁盘快照可能落在 turn 进行中（底座 add_session_message 每次落盘）：
            // 此时 tool_use 还没有对应 tool_result。若按历史恢复，result 缺失会
            // 落入 submitted 误标，且挡住 get_pending_user_inputs 恢复的 active 卡
            // （幂等去重按 toolCallId 命中）。此处跳过，交给 pending 恢复为可交互卡。
            if (!result) continue;
            lane.items.push({
              id: nextId(lane),
              type: 'user_input',
              toolCallId: block.id,
              questions,
              resolved: true,
              cardState: result.is_error ? 'cancelled' : 'submitted',
              // 还原用户曾提交的答案：历史卡切走再切回后仍能看到自己选了啥。
              // （#226 已保证走到这里 result 存在，无需 result && 守卫）
              restoredAnswers: !result.is_error
                ? parseNativeUserAnswers(result.content, questions)
                : null,
              time: '',
            });
          }
          continue;
        }
        // update_plan / checklist_write / todo_write → 收集快照，本条消息末尾还原
        // 只读方案卡（对齐 work hydration：plan 工具不还原工具卡）。
        if (PLAN_TOOLS.includes(block.name)) {
          const snap = parseNativePlanSnapshot(resultById[block.id] && resultById[block.id].content);
          if (snap) {
            if (block.name === 'update_plan') planSnap = snap;
            else todosSnap = snap;
          }
          sawPlanTool = true;
          continue;
        }
        lane.items.push({
          id: nextId(lane),
          type: 'tool',
          toolId: block.id,
          name: block.name || '',
          args: block.input,
          output: null,
          success: null,
          state: 'pending',
        });
      }
    }
    flushText();
    // 本条 assistant 消息用过 plan 工具 → 还原一张只读历史方案卡。
    if (sawPlanTool && (planSnap || todosSnap)) {
      const snaps = { plan: planSnap, todos: todosSnap };
      lane.items.push({
        id: nextId(lane),
        type: 'plan_card',
        plan: planSnap,
        todos: todosSnap,
        planMarkdown: composeNativePlanMarkdown(snaps),
        planId: null,
        cardState: 'frozen',
        resolved: true,
        statusKey: 'historical',
        time: '',
      });
    }
  }
  // 未被 tool_result 回填的工具卡按失败收尾，避免历史里残留"执行中"。
  for (const item of lane.items) {
    if (item && item.type === 'tool' && item.state !== 'done') {
      item.state = 'done';
      item.success = item.success === null ? false : item.success;
    }
  }
  lane.timeline = Array.isArray(timelineEvents) ? [...timelineEvents] : [];
  lane.busy = hadLiveTurn;
  if (!lane.busy) lane.thinking = null;
  lane.hydrated = true;
  return lane;
}

/// lane → ConversationTimeline 使用的 turn 投影。
export function projectNativeLane(lane, sessionId) {
  return projectDeepSeekConversation({
    chatItems: lane ? lane.items : [],
    busy: Boolean(lane && lane.busy),
    thinking: lane ? lane.thinking : null,
    tokens: lane ? lane.tokens : null,
    sessionId,
    timelineEvents: lane ? lane.timeline : [],
  });
}
