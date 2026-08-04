#!/usr/bin/env node
// code-native-lane.js 的纯逻辑回归：chat:* 事件推进、SavedSession hydration、投影。
// 风格对齐 deepseek_conversation_timeline.test.mjs：把模块复制到临时 type:module 目录再导入。
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-code-native-lane-'));
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
mkdirSync(path.join(temp, 'conversation'), { recursive: true });
mkdirSync(path.join(temp, 'codex'), { recursive: true });
for (const file of ['conversation-model.js', 'deepseek-conversation.js']) {
  copyFileSync(path.join(root, 'src', 'features', 'conversation', file), path.join(temp, 'conversation', file));
}
copyFileSync(path.join(root, 'src', 'features', 'codex', 'code-native-lane.js'), path.join(temp, 'codex', 'code-native-lane.js'));

try {
  const {
    applyNativeChatEvent,
    appendLocalUserMessage,
    createNativeLane,
    hydrateNativeLane,
    projectNativeLane,
    removeLocalUserMessage,
  } = await import(`${pathToFileURL(path.join(temp, 'codex', 'code-native-lane.js')).href}?t=${Date.now()}`);

  // ── 发送 + 流式回合 ─────────────────────────────────────────────
  const lane = createNativeLane();
  const optimisticId = appendLocalUserMessage(lane, '修复登录页样式');
  assert.equal(lane.busy, true, '乐观插入后即 busy');
  assert.equal(lane.timeline.filter(event => event.event === 'user_start').length, 1);

  // turn_started 不重复记录起点（60 秒内复用乐观插入的 user_start）。
  applyNativeChatEvent(lane, 'chat:turn_started', { session_id: 's1', turn_id: 't1' });
  assert.equal(lane.timeline.filter(event => event.event === 'user_start').length, 1);

  applyNativeChatEvent(lane, 'chat:reasoning_start', { session_id: 's1' });
  applyNativeChatEvent(lane, 'chat:reasoning_delta', { session_id: 's1', text: '先看代码' });
  applyNativeChatEvent(lane, 'chat:reasoning_done', { session_id: 's1' });
  applyNativeChatEvent(lane, 'chat:delta', { session_id: 's1', text: '好的，' });
  applyNativeChatEvent(lane, 'chat:delta', { session_id: 's1', text: '我来处理' });
  applyNativeChatEvent(lane, 'chat:tool_start', { session_id: 's1', id: 'call-1', name: 'exec_shell', args: { command: 'ls' } });
  assert.equal(lane.thinking.phase, 'tool');
  applyNativeChatEvent(lane, 'chat:tool_end', { session_id: 's1', id: 'call-1', success: true, output: 'a.txt' });
  applyNativeChatEvent(lane, 'chat:usage', { session_id: 's1', input_tokens: 1234 });
  applyNativeChatEvent(lane, 'chat:done', { session_id: 's1', status: 'Completed' });

  assert.equal(lane.busy, false, 'done 后结束 busy');
  assert.equal(lane.tokens.input, 1234);
  const projection = projectNativeLane(lane, 's1');
  assert.equal(projection.turns.length, 1, '单 user 回合聚成一个 turn');
  const [turn] = projection.turns;
  assert.equal(turn.userText, '修复登录页样式');
  assert.equal(turn.status, 'Completed');
  const assistantItems = turn.items.filter(item => item.type === 'agent_message');
  assert.equal(assistantItems[0].legacyItem.text, '好的，我来处理', 'delta 累积成完整文本');
  const toolItems = turn.items.filter(item => item.type === 'command_execution');
  assert.equal(toolItems.length, 1, 'exec_shell 归类为 command_execution');
  assert.equal(toolItems[0].status, 'completed');
  const reasoningItems = turn.items.filter(item => item.type === 'reasoning');
  assert.equal(reasoningItems[0].text, '先看代码');

  // ── 选择确认卡：请求 → 提交后 tool_end 收口 ─────────────────────
  const lane2 = createNativeLane();
  applyNativeChatEvent(lane2, 'chat:tool_start', { session_id: 's2', id: 'call-9', name: 'request_user_input', args: {} });
  assert.equal(lane2.items.some(item => item.type === 'tool'), false, 'request_user_input 不出工具卡');
  applyNativeChatEvent(lane2, 'chat:user_input_required', {
    session_id: 's2',
    id: 'call-9',
    questions: [{ id: 'q1', header: '方案', question: '选哪个？', options: [{ label: 'A' }, { label: 'B' }] }],
  });
  const card = lane2.items.find(item => item.type === 'user_input');
  assert.equal(card.resolved, false);
  assert.equal(lane2.items.filter(item => item.type === 'user_input').length, 1);
  // 重复事件不重复出卡。
  applyNativeChatEvent(lane2, 'chat:user_input_required', { session_id: 's2', id: 'call-9', questions: [{ id: 'q1' }] });
  assert.equal(lane2.items.filter(item => item.type === 'user_input').length, 1);
  applyNativeChatEvent(lane2, 'chat:tool_end', { session_id: 's2', id: 'call-9', success: true, output: '' });
  assert.equal(card.resolved, true);
  assert.equal(card.cardState, 'submitted');

  // ── 发送失败回滚 ────────────────────────────────────────────────
  const lane3 = createNativeLane();
  const rollbackId = appendLocalUserMessage(lane3, '这条发不出去');
  removeLocalUserMessage(lane3, rollbackId);
  assert.equal(lane3.items.length, 0);
  assert.equal(lane3.timeline.length, 0, 'user_start 一并回滚');
  assert.equal(lane3.busy, false);

  // ── hydration：SavedSession messages → items ────────────────────
  const lane4 = createNativeLane();
  hydrateNativeLane(lane4, {
    messages: [
      { role: 'user', content: [{ type: 'text', text: '写个脚本' }] },
      {
        role: 'assistant',
        content: [
          { type: 'thinking', thinking: '先想目录结构' },
          { type: 'text', text: '好的' },
          { type: 'tool_use', id: 'c1', name: 'write_file', input: { path: 'a.sh' } },
        ],
      },
      { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'c1', content: 'ok' }] },
      { role: 'assistant', content: [{ type: 'text', text: '已完成' }] },
      {
        role: 'assistant',
        content: [{ type: 'tool_use', id: 'c2', name: 'request_user_input', input: { questions: [{ id: 'q', header: 'H' }] } }],
      },
      { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'c2', content: 'answers', is_error: false }] },
    ],
  }, [
    { turn_id: 't1', event: 'user_start', timestamp: 1000, ui_turn_index: 0 },
    { turn_id: 't1', event: 'assistant_done', timestamp: 2000, status: 'Completed', usage: { input_tokens: 10, output_tokens: 5 } },
  ]);
  assert.equal(lane4.hydrated, true);
  assert.equal(lane4.busy, false, '无 live 痕迹时 hydration 不恢复 busy');
  const hydrated = projectNativeLane(lane4, 's4');
  assert.equal(hydrated.turns.length, 1);
  assert.equal(hydrated.turns[0].status, 'Completed', 'timeline 事件驱动回合状态');
  const hydratedTool = lane4.items.find(item => item.type === 'tool' && item.toolId === 'c1');
  assert.equal(hydratedTool.state, 'done');
  assert.equal(hydratedTool.output, 'ok');
  assert.equal(hydratedTool.success, true);
  const hydratedInput = lane4.items.find(item => item.type === 'user_input');
  assert.equal(hydratedInput.resolved, true, '历史 request_user_input 还原为已处理卡');
  const hydratedReasoning = lane4.items.find(item => item.type === 'reasoning');
  assert.equal(hydratedReasoning.text, '先想目录结构');
  assert.equal(
    lane4.items.filter(item => item.type === 'assistant').map(item => item.text).join('|'),
    '好的|已完成',
  );

  // ── 切回正在跑的会话：hydration 保留 live busy ──────────────────
  applyNativeChatEvent(lane4, 'chat:turn_started', { session_id: 's4', turn_id: 't2' });
  assert.equal(lane4.busy, true);
  hydrateNativeLane(lane4, { messages: [] }, []);
  assert.equal(lane4.busy, true, '已有 live turn 时 hydration 不得清 busy');

  // ── 远端用户消息（遥控端发送）：去重本地乐观气泡 ────────────────
  const lane5 = createNativeLane();
  appendLocalUserMessage(lane5, '本地一句\n📎 a.png');
  applyNativeChatEvent(lane5, 'chat:user_message', { session_id: 's5', content: '本地一句' });
  assert.equal(lane5.items.filter(item => item.type === 'user').length, 1, '发送后 30 秒内的回声按本地气泡去重');
  applyNativeChatEvent(lane5, 'chat:user_message', { session_id: 's5', content: '手机端来的' });
  assert.equal(lane5.items.filter(item => item.type === 'user').length, 2);

  // ── 后台 shell 任务终态：工具卡更新为最终状态并合并输出尾段 ──────
  const lane6 = createNativeLane();
  applyNativeChatEvent(lane6, 'chat:tool_start', { session_id: 's6', id: 'call-sh', name: 'exec_shell', args: { command: 'npm test' } });
  applyNativeChatEvent(lane6, 'chat:shell_task_status', {
    session_id: 's6',
    tool_id: 'call-sh',
    task_id: 'task-1',
    status: 'Completed',
    exit_code: 0,
    stdout_tail: 'ok tail',
    stderr_tail: '',
  });
  const shellItem = lane6.items.find(item => item.toolId === 'call-sh');
  assert.equal(shellItem.state, 'done');
  assert.equal(shellItem.success, true);
  assert.equal(shellItem.exitCode, 0);
  assert.equal(shellItem.output, 'ok tail');
  applyNativeChatEvent(lane6, 'chat:tool_start', { session_id: 's6', id: 'call-sh2', name: 'exec_shell', args: { command: 'make' } });
  applyNativeChatEvent(lane6, 'chat:shell_task_status', {
    session_id: 's6',
    tool_id: 'call-sh2',
    task_id: 'task-2',
    status: 'Failed',
    exit_code: 2,
    stdout_tail: 'out',
    stderr_tail: 'boom',
  });
  const failedShell = lane6.items.find(item => item.toolId === 'call-sh2');
  assert.equal(failedShell.state, 'failed');
  assert.equal(failedShell.success, false);
  assert.equal(failedShell.output, 'out\n[STDERR] boom');
  // 未知 tool_id 的状态推送不产生变化。
  assert.equal(
    applyNativeChatEvent(lane6, 'chat:shell_task_status', { session_id: 's6', tool_id: 'ghost', task_id: 't', status: 'Completed' }),
    false,
  );

  // ── compaction：渲染为系统提示项 ─────────────────────────────────
  const lane7 = createNativeLane();
  applyNativeChatEvent(lane7, 'chat:compaction', { session_id: 's7', phase: 'start', message: 'auto compact' });
  applyNativeChatEvent(lane7, 'chat:compaction', { session_id: 's7', phase: 'done', message: '12 → 8' });
  const notices = lane7.items.filter(item => item.type === 'system');
  assert.equal(notices.length, 2);
  assert.equal(notices[0].compactPhase, 'start');
  assert.equal(notices[1].compactPhase, 'done');
  assert.equal(notices[1].text, '12 → 8');

  console.log('code_native_lane.test.mjs: all assertions passed');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
