// 代码模块原生（品悟 Engine）会话的本地会话车道。
//
// ACP 会话由后端维护 timeline（get_codex_acp_timeline）；原生会话复用主聊天的
// engine 链路：chat 命令发消息、`chat:*` 事件推进、SavedSession messages 落盘。
// 本模块把一个会话的展示状态（chatItems/busy/thinking/tokens/turn timeline）
// 收敛成纯数据 lane，便于 React 侧按 session 缓存与单测；渲染统一走
// projectDeepSeekConversation → ConversationTimeline。
//
// lane.items 是 bridge chatItems 的兼容子集：user / assistant(text) / reasoning /
// tool / user_input / careful_blocked / system。与 bridge 的差异：assistant 保留
// 原始 markdown 文本（bridge 存预渲染 html），渲染层用 ConversationMarkdown。

import { projectDeepSeekConversation } from '../conversation/deepseek-conversation.js';

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
      const lastUser = [...lane.items].reverse().find(item => item && item.type === 'user');
      if (lastUser) {
        // 本地乐观插入已覆盖：文本一致，或刚发送（本地气泡带 📎 附件名等展示
        // 修饰，与后端回声文本不同）30 秒内视为同一消息的回声。
        if (lastUser.text === content
          || (lastUser.localEchoTs && Date.now() - lastUser.localEchoTs < 30000)) {
          delete lastUser.localEchoTs;
          return false;
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
      if (meta && meta.name === 'request_user_input') {
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
      if (lane.items.some(item => item && item.type === 'user_input' && item.toolCallId === p.id)) return false;
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
      lane.items.push({
        id: nextId(lane),
        type: 'system',
        compactPhase: phase,
        text: String(p.message || ''),
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

/// SavedSession messages → lane.items（hydration 是 rerenderFromMessages 的精简版：
/// 覆盖 user / assistant text / thinking / tool_use+tool_result / request_user_input；
/// persona、成品卡、plan 卡等主聊天专属形态不在代码会话出现，不做还原）。
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
  for (const message of messages) {
    const role = message && message.role;
    const raw = message && message.content;
    const blocks = Array.isArray(raw)
      ? raw
      : (typeof raw === 'string' && raw ? [{ type: 'text', text: raw }] : []);
    if (role === 'user') {
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
            lane.items.push({
              id: nextId(lane),
              type: 'user_input',
              toolCallId: block.id,
              questions,
              resolved: true,
              cardState: result && result.is_error ? 'cancelled' : 'submitted',
              time: '',
            });
          }
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
