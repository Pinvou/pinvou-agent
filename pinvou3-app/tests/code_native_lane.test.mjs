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
    appendNativeSystemItem,
    composeNativePlanMarkdown,
    createNativeLane,
    hydrateNativeLane,
    parseNativePlanSnapshot,
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

  // ── Plan 审批：snapshot → ready → 覆盖/批准 ─────────────────────
  const planSnap = {
    explanation: '先改配置再跑测试',
    items: [{ step: '改配置', status: 'pending' }, { step: '跑测试', status: 'pending' }],
  };
  const todosSnap = { items: [{ content: '子任务 A', status: 'in_progress' }] };

  const lane8 = createNativeLane();
  appendLocalUserMessage(lane8, '帮我重构登录模块');
  applyNativeChatEvent(lane8, 'chat:user_message', { session_id: 's8', content: '帮我重构登录模块' });
  // plan_snapshot：只带本次改的那份，另一份保留。
  assert.equal(applyNativeChatEvent(lane8, 'chat:plan_snapshot', { session_id: 's8', plan_snapshot: planSnap, todos_snapshot: null }), true);
  assert.equal(lane8.planSnapshot.plan, planSnap);
  assert.equal(lane8.planSnapshot.todos, null);
  applyNativeChatEvent(lane8, 'chat:plan_snapshot', { session_id: 's8', plan_snapshot: null, todos_snapshot: todosSnap });
  assert.equal(lane8.planSnapshot.todos, todosSnap);
  assert.equal(lane8.planSnapshot.plan, planSnap);
  // plan_ready：弹 active 审批卡，planMarkdown 对齐 bridge composePlanMarkdown。
  assert.equal(applyNativeChatEvent(lane8, 'chat:plan_ready', {
    session_id: 's8', plan_id: 'plan-1', plan_snapshot: planSnap, todos_snapshot: todosSnap,
  }), true);
  const card1 = lane8.items.find(item => item.type === 'plan_card');
  assert.equal(card1.cardState, 'active');
  assert.equal(card1.resolved, false);
  assert.equal(card1.planId, 'plan-1');
  assert.equal(card1.plan.explanation, '先改配置再跑测试');
  assert.match(card1.planMarkdown, /\*\*方案：\*\*/);
  assert.match(card1.planMarkdown, /1\. ○ 改配置/);
  assert.match(card1.planMarkdown, /\*\*细分待办：\*\*/);
  assert.match(card1.planMarkdown, /1\. ◎ 子任务 A/);
  // 同 plan_id 重复 ready 不再出卡。
  assert.equal(applyNativeChatEvent(lane8, 'chat:plan_ready', {
    session_id: 's8', plan_id: 'plan-1', plan_snapshot: planSnap, todos_snapshot: null,
  }), false);
  assert.equal(lane8.items.filter(item => item.type === 'plan_card').length, 1);
  // 新方案 → 旧卡冻结为 superseded，新卡 active。
  applyNativeChatEvent(lane8, 'chat:plan_ready', {
    session_id: 's8', plan_id: 'plan-2', plan_snapshot: planSnap, todos_snapshot: null,
  });
  assert.equal(card1.cardState, 'frozen');
  assert.equal(card1.resolved, true);
  assert.equal(card1.statusKey, 'superseded');
  const card2 = lane8.items.find(item => item.type === 'plan_card' && item.planId === 'plan-2');
  assert.equal(card2.cardState, 'active');
  // turn 终态不清理方案卡（work 语义：审批与回合生命周期解耦）。
  applyNativeChatEvent(lane8, 'chat:done', { session_id: 's8', status: 'Completed' });
  assert.equal(lane8.busy, false);
  assert.equal(card2.cardState, 'active');
  // 投影：plan_card → type 'plan' + extensionType 区分（渲染层据此出审批卡）。
  const planTurn = projectNativeLane(lane8, 's8').turns[0];
  const projectedPlans = planTurn.items.filter(item => item.type === 'plan');
  assert.equal(projectedPlans.length, 2);
  assert.equal(projectedPlans[0].extensionType, 'plan_card');
  // 远端批准回声（action=accept_plan）：命中卡片置 approved，消息照常入列。
  assert.equal(applyNativeChatEvent(lane8, 'chat:user_message', {
    session_id: 's8', content: '✅ 就这么干', action: 'accept_plan', plan_id: 'plan-2',
  }), true);
  assert.equal(card2.cardState, 'approved');
  assert.equal(card2.resolved, true);
  assert.equal(card2.statusKey, 'approved');
  assert.equal(lane8.items.filter(item => item.type === 'user').length, 2);
  assert.equal(lane8.busy, true, '批准后进入执行回合');
  // plan_id 不命中不批卡。
  const lane8b = createNativeLane();
  applyNativeChatEvent(lane8b, 'chat:plan_ready', { session_id: 's8b', plan_id: 'plan-9', plan_snapshot: planSnap, todos_snapshot: null });
  applyNativeChatEvent(lane8b, 'chat:user_message', { session_id: 's8b', content: '✅ 就这么干', action: 'accept_plan', plan_id: 'plan-other' });
  const orphanCard = lane8b.items.find(item => item.type === 'plan_card');
  assert.equal(orphanCard.cardState, 'active', 'plan_id 不匹配不误批');
  // 无 plan_id 的 ready（历史快照重放）→ 只读历史卡。
  const lane10 = createNativeLane();
  applyNativeChatEvent(lane10, 'chat:plan_ready', { session_id: 's10', plan_snapshot: planSnap, todos_snapshot: null });
  const legacyCard = lane10.items.find(item => item.type === 'plan_card');
  assert.equal(legacyCard.cardState, 'frozen');
  assert.equal(legacyCard.resolved, true);
  assert.equal(legacyCard.statusKey, 'historical');
  // composeNativePlanMarkdown 空快照兜底。
  assert.equal(composeNativePlanMarkdown({ plan: null, todos: null }), '（plan 为空）');
  // 系统提示项（accept/discard 失败路径）。
  appendNativeSystemItem(lane10, '⚠️ accept_plan 失败: boom');
  assert.equal(lane10.items[lane10.items.length - 1].type, 'system');

  // ── hydration：plan 工具还原只读历史方案卡，不还原工具卡 ──────────
  const lane9 = createNativeLane();
  hydrateNativeLane(lane9, {
    messages: [
      { role: 'user', content: [{ type: 'text', text: '出个方案' }] },
      {
        role: 'assistant',
        content: [
          { type: 'text', text: '好的，方案如下' },
          { type: 'tool_use', id: 'p1', name: 'update_plan', input: planSnap },
          { type: 'tool_use', id: 'p2', name: 'checklist_write', input: {} },
        ],
      },
      {
        role: 'user',
        content: [
          { type: 'tool_result', tool_use_id: 'p1', content: `Plan updated:\n${JSON.stringify(planSnap)}` },
          { type: 'tool_result', tool_use_id: 'p2', content: `Checklist updated:\n${JSON.stringify(todosSnap)}` },
        ],
      },
    ],
  }, []);
  const historical = lane9.items.find(item => item.type === 'plan_card');
  assert.equal(historical.cardState, 'frozen');
  assert.equal(historical.resolved, true);
  assert.equal(historical.statusKey, 'historical');
  assert.equal(historical.planId, null, 'hydrate 降级为只读历史卡（与 work 冷启动对齐）');
  assert.equal(historical.plan.explanation, '先改配置再跑测试');
  assert.equal(historical.todos.items.length, 1);
  assert.equal(
    lane9.items.some(item => item.type === 'tool' && (item.toolId === 'p1' || item.toolId === 'p2')),
    false,
    'plan 工具不还原工具卡',
  );
  assert.match(historical.planMarkdown, /\*\*方案：\*\*/);
  assert.deepEqual(lane9.planSnapshot, { plan: null, todos: null }, 'hydration 清空 live 快照');
  // parseNativePlanSnapshot 边界：无换行 / 坏 JSON / blocks 数组。
  assert.equal(parseNativePlanSnapshot('no-newline'), null);
  assert.equal(parseNativePlanSnapshot('bad\n{json'), null);
  assert.equal(
    parseNativePlanSnapshot([{ type: 'text', text: `Plan updated:\n${JSON.stringify(planSnap)}` }]).explanation,
    '先改配置再跑测试',
  );

  // ── chat:memory：注入记忆快照存入 lane（不归一化字段被丢弃、空文本过滤）──
  const lane11 = createNativeLane();
  assert.equal(lane11.memory, null, '未收到事件前无记忆快照');
  assert.equal(applyNativeChatEvent(lane11, 'chat:memory', {
    session_id: 's11',
    runtime_path: '/tmp/mem.md',
    items: [
      { id: 'profile.call_name', kind: 'profile', text: '称呼：欣哥' },
      { id: 'preference.1', kind: 'preference', text: '先给结论' },
      { id: 'preference.2', kind: 'preference', text: '' },
      'garbage',
    ],
  }), true);
  assert.equal(lane11.memory.runtimePath, '/tmp/mem.md');
  assert.equal(lane11.memory.items.length, 2, '空文本与非对象条目被过滤');
  assert.deepEqual(lane11.memory.items[0], { id: 'profile.call_name', kind: 'profile', text: '称呼：欣哥' });
  assert.equal(typeof lane11.memory.updatedAt, 'number');
  // 空快照（记忆全局关闭时后端也发射）同样落 lane，渲染层据此不显示徽标。
  applyNativeChatEvent(lane11, 'chat:memory', { session_id: 's11', runtime_path: '', items: [] });
  assert.equal(lane11.memory.items.length, 0);
  // 记忆快照是会话级 live 状态：hydration 重载消息不清空（磁盘无对应物）。
  applyNativeChatEvent(lane11, 'chat:memory', {
    session_id: 's11', runtime_path: '/tmp/mem.md', items: [{ id: 'p', kind: 'profile', text: '称呼：欣哥' }],
  });
  hydrateNativeLane(lane11, { messages: [] }, []);
  assert.equal(lane11.memory.items.length, 1, 'hydration 保留记忆快照');

  // ── compaction 进行中标记：start 置位、done/fail 复位（用于禁用压缩入口）──
  const lane12 = createNativeLane();
  assert.equal(lane12.compacting, false);
  applyNativeChatEvent(lane12, 'chat:compaction', { session_id: 's12', phase: 'start' });
  assert.equal(lane12.compacting, true);
  applyNativeChatEvent(lane12, 'chat:compaction', { session_id: 's12', phase: 'done', message: '12 → 8' });
  assert.equal(lane12.compacting, false);
  applyNativeChatEvent(lane12, 'chat:compaction', { session_id: 's12', phase: 'start' });
  applyNativeChatEvent(lane12, 'chat:compaction', { session_id: 's12', phase: 'fail', message: 'boom' });
  assert.equal(lane12.compacting, false, 'fail 同样复位');

  // ── lane13: chat:plan_resolved 远端回声（多端/远端 discard 同步）─────────
  // 本地 discardNativePlan 已乐观冻结;plan_resolved 是后端广播,保证另一端 active 卡
  // 同步冻结。对齐 bridge chat-events.js plan_resolved。
  const lane13 = createNativeLane();
  appendLocalUserMessage(lane13, '审视下方案');
  applyNativeChatEvent(lane13, 'chat:plan_ready', {
    session_id: 's13', plan_id: 'plan-r', plan_snapshot: planSnap, todos_snapshot: null,
  });
  const resCard = lane13.items.find(item => item.type === 'plan_card');
  assert.equal(resCard.cardState, 'active');
  assert.equal(resCard.resolved, false);
  // plan_resolved 命中 active 卡 → 幂等冻结为 discarded。
  assert.equal(applyNativeChatEvent(lane13, 'chat:plan_resolved', {
    session_id: 's13', plan_id: 'plan-r', action: 'discard_plan',
  }), true);
  assert.equal(resCard.cardState, 'frozen');
  assert.equal(resCard.resolved, true);
  assert.equal(resCard.statusKey, 'discarded');
  // 缺 plan_id 直接跳过。
  assert.equal(applyNativeChatEvent(lane13, 'chat:plan_resolved', { session_id: 's13' }), false);
  // 已 resolved 的卡再次收到同 plan_id 不再变化（幂等）。
  assert.equal(applyNativeChatEvent(lane13, 'chat:plan_resolved', {
    session_id: 's13', plan_id: 'plan-r',
  }), false);

  console.log('code_native_lane.test.mjs: all assertions passed');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
