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
mkdirSync(path.join(temp, 'features', 'conversation'), { recursive: true });
mkdirSync(path.join(temp, 'features', 'codex'), { recursive: true });
mkdirSync(path.join(temp, 'shared'), { recursive: true });
for (const file of ['conversation-model.js', 'deepseek-conversation.js']) {
  copyFileSync(path.join(root, 'src', 'features', 'conversation', file), path.join(temp, 'features', 'conversation', file));
}
copyFileSync(path.join(root, 'src', 'features', 'codex', 'code-native-lane.js'), path.join(temp, 'features', 'codex', 'code-native-lane.js'));
copyFileSync(path.join(root, 'src', 'shared', 'internal-message.mjs'), path.join(temp, 'shared', 'internal-message.mjs'));

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
  } = await import(`${pathToFileURL(path.join(temp, 'features', 'codex', 'code-native-lane.js')).href}?t=${Date.now()}`);

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

  // ── agent 启动失败：tool_end 的完成态与成功态必须分别保留 ────────────
  const failedAgentLane = createNativeLane();
  applyNativeChatEvent(failedAgentLane, 'chat:tool_start', {
    session_id: 'agent-live',
    id: 'agent-call-failed',
    name: 'agent',
    args: { action: 'start', prompt: '「国际AI新闻采集」只读调研' },
  });
  applyNativeChatEvent(failedAgentLane, 'chat:tool_end', {
    session_id: 'agent-live',
    id: 'agent-call-failed',
    name: 'agent',
    success: false,
    output: 'Error: write-scope contention with agent_6282bd07',
  });
  const failedAgentLive = failedAgentLane.items.find(item => item.toolId === 'agent-call-failed');
  assert.equal(failedAgentLive.state, 'done', 'tool 调用已收口，不应残留执行中');
  assert.equal(failedAgentLive.success, false, '失败事实必须独立于完成态保留');

  // ── 产品专家模式关闭时，底座裸 agent 仍是事实委派 ────────────────
  // 底座允许省略 action/profile（缺省 action=start）。这类调用不能因为
  // Pinvou 专家名册未开启而折进普通工具组，否则卡片与 transcript 入口会消失。
  const bareAgentLane = createNativeLane();
  appendLocalUserMessage(bareAgentLane, '向子智能体问你好');
  applyNativeChatEvent(bareAgentLane, 'chat:tool_start', {
    session_id: 'bare-agent-session',
    id: 'bare-agent-call',
    name: 'agent',
    args: { prompt: '向子智能体问你好' },
  });
  applyNativeChatEvent(bareAgentLane, 'chat:tool_end', {
    session_id: 'bare-agent-session',
    id: 'bare-agent-call',
    name: 'agent',
    success: true,
    output: '{"agent_id":"agent_1234"}',
  });
  applyNativeChatEvent(bareAgentLane, 'chat:done', {
    session_id: 'bare-agent-session',
    status: 'Completed',
  });
  const bareAgentTurn = projectNativeLane(bareAgentLane, 'bare-agent-session').turns[0];
  const bareAgentPresentation = bareAgentTurn.presentation.find(
    item => item.legacyItem?.toolId === 'bare-agent-call',
  );
  assert.equal(bareAgentPresentation?.type, 'tool', '裸 agent spawn 必须保持为一等子智能体卡');
  assert.equal(
    bareAgentTurn.presentation.some(item => (
      item.type === 'tool_group'
      && item.items.some(child => child.legacyItem?.toolId === 'bare-agent-call')
    )),
    false,
    '裸 agent spawn 不得折入默认收起的普通工具组',
  );

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
  // 已收口（resolved）的卡片再收到同 id 事件（历史快照误标后 pending 恢复）→
  // 重置为 active，让用户仍能选择（对应「切回显示已提交无法选择」的修复）。
  applyNativeChatEvent(lane2, 'chat:user_input_required', { session_id: 's2', id: 'call-9', questions: [{ id: 'q1' }] });
  assert.equal(card.resolved, false, '误标/历史快照卡片被 pending 恢复重置为 active');
  assert.equal(card.cardState, 'active');

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

  // ── hydrate 还原用户已选答案：单选 + 多选（multi_select 不塌缩）────
  const lane4d = createNativeLane();
  hydrateNativeLane(lane4d, {
    messages: [
      {
        role: 'assistant',
        content: [{
          type: 'tool_use', id: 'c3', name: 'request_user_input',
          input: { questions: [
            { id: 'q1', header: '语言', question: '用什么语言？', options: [{ label: 'Python', description: '' }, { label: 'Go', description: '' }], multi_select: false },
            { id: 'q2', header: '技能', question: '擅长哪些？', options: [{ label: '前端', description: '' }, { label: '后端', description: '' }, { label: '运维', description: '' }], multi_select: true },
          ] },
        }],
      },
      {
        role: 'user',
        content: [{
          type: 'tool_result', tool_use_id: 'c3', is_error: false,
          content: JSON.stringify({ answers: [
            { id: 'q1', label: 'Python', value: 'Python' },
            { id: 'q2', label: '前端', value: '前端' },
            { id: 'q2', label: '运维', value: '运维' },
          ] }),
        }],
      },
    ],
  }, []);
  const restoredInput = lane4d.items.find(item => item.type === 'user_input');
  assert.equal(restoredInput.resolved, true);
  assert.deepEqual(
    restoredInput.restoredAnswers,
    [
      { id: 'q1', label: 'Python', value: 'Python' },
      { id: 'q2', label: '前端', value: '前端' },
      { id: 'q2', label: '运维', value: '运维' },
    ],
    '单选/多选答案按 id 全量还原，multi_select 不塌缩为最后一项',
  );

  // ── hydrate 特殊 question id（constructor/toString/__proto__）不抛错 ──
  // question id 后端仅校验非空，这些保留属性名是合法输入；parseNativeUserAnswers
  // 用 Object.create(null) 分组后不得命中 Object.prototype（复核 P1）。
  for (const specialId of ['constructor', 'toString', '__proto__']) {
    const lane4s = createNativeLane();
    hydrateNativeLane(lane4s, {
      messages: [
        {
          role: 'assistant',
          content: [{
            type: 'tool_use', id: 'cs', name: 'request_user_input',
            input: { questions: [{ id: specialId, header: '选择', question: '选？', options: [{ label: 'A', description: '' }] }] },
          }],
        },
        {
          role: 'user',
          content: [{
            type: 'tool_result', tool_use_id: 'cs', is_error: false,
            content: JSON.stringify({ answers: [{ id: specialId, label: 'A', value: 'A' }] }),
          }],
        },
      ],
    }, []);
    const restoredSpecial = lane4s.items.find(item => item.type === 'user_input');
    assert.equal(restoredSpecial.resolved, true, `特殊 id "${specialId}" hydrate 不得抛错`);
    assert.deepEqual(
      restoredSpecial.restoredAnswers,
      [{ id: specialId, label: 'A', value: 'A' }],
      `特殊 id "${specialId}" 答案按 id 还原`,
    );
  }

  const hydratedReasoning = lane4.items.find(item => item.type === 'reasoning');
  assert.equal(hydratedReasoning.text, '先想目录结构');
  assert.equal(
    lane4.items.filter(item => item.type === 'assistant').map(item => item.text).join('|'),
    '好的|已完成',
  );

  const failedAgentHydratedLane = createNativeLane();
  hydrateNativeLane(failedAgentHydratedLane, {
    messages: [
      { role: 'user', content: [{ type: 'text', text: '采集 AI 新闻' }] },
      {
        role: 'assistant',
        content: [{
          type: 'tool_use',
          id: 'agent-call-hydrated-failed',
          name: 'agent',
          input: { action: 'start', prompt: '「国际AI新闻采集」只读调研' },
        }],
      },
      {
        role: 'user',
        content: [{
          type: 'tool_result',
          tool_use_id: 'agent-call-hydrated-failed',
          content: 'Error: write-scope contention with agent_6282bd07',
          is_error: true,
        }],
      },
    ],
  }, []);
  const failedAgentHydrated = failedAgentHydratedLane.items.find(
    item => item.toolId === 'agent-call-hydrated-failed',
  );
  assert.equal(failedAgentHydrated.state, 'done', '重开会话后失败工具卡仍是已收口状态');
  assert.equal(failedAgentHydrated.success, false, '重开会话后必须恢复 is_error 事实');

  // ── 切回正在跑的会话：hydration 保留 live busy ──────────────────
  applyNativeChatEvent(lane4, 'chat:turn_started', { session_id: 's4', turn_id: 't2' });
  assert.equal(lane4.busy, true);
  hydrateNativeLane(lane4, { messages: [] }, []);
  assert.equal(lane4.busy, true, '已有 live turn 时 hydration 不得清 busy');

  // ── hydration + live：内部运行时信封不上屏（对齐 bridge 过滤）─────
  const laneEnvelope = createNativeLane();
  hydrateNativeLane(laneEnvelope, {
    messages: [
      { role: 'user', content: [{ type: 'text', text: '真实用户提问' }] },
      {
        role: 'user',
        content: [
          { type: 'text', text: [
            '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
            'This is an internal runtime event, not user input.',
            'child completion summary',
            '<codewhale:subagent.done>{"agent_id":"a1","status":"completed"}</codewhale:subagent.done>',
            '</codewhale:runtime_event>',
          ].join('\n') },
          { type: 'text', text: '<turn_meta>\nInput provenance: subagent_handoff (non-authoritative)\n</turn_meta>' },
        ],
      },
      {
        role: 'user',
        content: [
          { type: 'text', text: [
            '<codewhale:runtime_event kind="background_shell_completion" visibility="internal">',
            'internal shell completion payload',
            '</codewhale:runtime_event>',
          ].join('\n') },
          { type: 'text', text: '<turn_meta>\nInput provenance: shell_completion (non-authoritative)\n</turn_meta>' },
        ],
      },
      { role: 'assistant', content: [{ type: 'text', text: '父智能体汇总' }] },
    ],
  }, []);
  assert.ok(laneEnvelope.items.some(item => item.type === 'user' && item.text.includes('真实用户提问')),
    '真实用户消息仍渲染');
  assert.ok(!laneEnvelope.items.some(item => item.type === 'user' && item.text.includes('child completion summary')),
    'hydrate 必须隐藏 subagent 交接信封');
  assert.ok(!laneEnvelope.items.some(item => item.type === 'user' && item.text.includes('internal shell completion payload')),
    'hydrate 必须隐藏 shell 完成信封');
  assert.ok(!JSON.stringify(laneEnvelope.items).includes('codewhale:runtime_event'),
    'hydrate 内部信封 XML 不得进入 lane 展示');

  // live 实时路径同样不上屏。
  const laneLiveEnvelope = createNativeLane();
  const liveChanged = applyNativeChatEvent(laneLiveEnvelope, 'chat:user_message', {
    session_id: 's-env',
    content: [
      '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
      'live child completion',
      '</codewhale:runtime_event>',
    ].join('\n'),
  });
  assert.equal(liveChanged, false, 'live 内部信封不产生可视变化');
  assert.equal(laneLiveEnvelope.items.some(item => item.type === 'user'), false,
    'live 内部信封不得 push 用户气泡');

  // hydrate 仅-provenance（无信封）遗留形态：白名单必须单独兜住。
  const laneProvenanceOnly = createNativeLane();
  hydrateNativeLane(laneProvenanceOnly, {
    messages: [
      { role: 'user', content: [{ type: 'text', text: '真实任务正文' }] },
      { role: 'user', content: [
        { type: 'text', text: '<turn_meta>\nInput provenance: shell_completion (non-authoritative)\n</turn_meta>' },
      ] },
      { role: 'assistant', content: [{ type: 'text', text: '父汇总' }] },
    ],
  }, []);
  assert.ok(laneProvenanceOnly.items.some(item => item.type === 'user' && item.text.includes('真实任务正文')),
    '真实任务正文仍渲染');
  assert.ok(!JSON.stringify(laneProvenanceOnly.items).includes('shell_completion'),
    '仅-provenance 内部消息不得进入 lane 展示');

  // ── hydration：request_user_input 的 tool_use 无 tool_result ──────
  // 快照可能落在 turn 进行中（底座 add_session_message 每次落盘）：此时
  // tool_use 尚无对应 tool_result，不能按历史恢复为 submitted（会误标并挡住
  // pending 恢复的 active 卡）——应跳过，由 chat:user_input_required 恢复。
  const lane4b = createNativeLane();
  hydrateNativeLane(lane4b, {
    messages: [
      { role: 'user', content: [{ type: 'text', text: '继续跑' }] },
      {
        role: 'assistant',
        content: [{ type: 'tool_use', id: 'c9', name: 'request_user_input', input: { questions: [{ id: 'q', header: 'H' }] } }],
      },
    ],
  }, []);
  assert.equal(lane4b.items.some(item => item.type === 'user_input'), false, '无 tool_result 的挂起提问不按历史恢复');
  // 随后 pending 恢复（get_pending_user_inputs → chat:user_input_required）出 active 卡。
  applyNativeChatEvent(lane4b, 'chat:user_input_required', {
    session_id: 's4b',
    id: 'c9',
    questions: [{ id: 'q', header: 'H', question: '选？', options: [{ label: 'A' }] }],
  });
  const pendingCard = lane4b.items.find(item => item.type === 'user_input');
  assert.equal(!!pendingCard, true);
  assert.equal(pendingCard.resolved, false);
  assert.equal(pendingCard.cardState, 'active');

  // ── remount 恢复的 active 卡超时/收口：tool_end 用 payload.name 兜底 ──
  // 恢复路径（pending → chat:user_input_required）没经过 tool_start，toolMeta
  // 缺失；tool_end 处理器若只看 toolMeta 会落入普通工具分支、卡片不收口。
  // 回归：300s 超时（success=false）后卡片进入 cancelled 终态。
  applyNativeChatEvent(lane4b, 'chat:tool_end', {
    session_id: 's4b',
    id: 'c9',
    name: 'request_user_input',
    success: false,
    output: '',
  });
  assert.equal(pendingCard.resolved, true, 'toolMeta 缺失时靠 payload.name 识别 request_user_input 收口');
  assert.equal(pendingCard.cardState, 'cancelled', '超时收口为 cancelled 终态');
  // 正常提交（success=true）同理进入 submitted 终态。
  const lane4c = createNativeLane();
  applyNativeChatEvent(lane4c, 'chat:user_input_required', {
    session_id: 's4c',
    id: 'c10',
    questions: [{ id: 'q', header: 'H', question: '选？', options: [{ label: 'A' }] }],
  });
  const pendingCard2 = lane4c.items.find(item => item.type === 'user_input');
  applyNativeChatEvent(lane4c, 'chat:tool_end', {
    session_id: 's4c',
    id: 'c10',
    name: 'request_user_input',
    success: true,
    output: 'A',
  });
  assert.equal(pendingCard2.resolved, true);
  assert.equal(pendingCard2.cardState, 'submitted', '提交收口为 submitted 终态');

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
