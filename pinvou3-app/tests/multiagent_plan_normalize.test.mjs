/** 多智能体（会话内主动委派，ADR-0006）薄层契约：桥、专家卡、只读面板与取消级联。 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import {
  isTranscriptChunk,
  mergeTranscriptMessages,
  startSubagentTranscriptPolling,
  startTranscriptPolling,
} from '../src/features/multiagent/runState.mjs';
import {
  extractSubagentId,
  fileChangeStat,
  projectSubagentTranscript,
  resolveSubagentIdentity,
  resolveSubagentPresentation,
  subagentAncestorIds,
  subagentObjectiveName,
  splitSubagentTitle,
  subagentOrdinalLabel,
  subagentRoleOrdinals,
  subagentTreeIsDone,
  visibleSubagentDescendantRows,
  visibleSubagentTreeRows,
  windowSubagentTranscript,
} from '../src/features/multiagent/subagent-conversation.mjs';
import {
  captureConversationScrollPosition,
  expertDelegationText,
  isExpertDelegationCall,
  presentConversationItems,
  restoreConversationScrollPosition,
} from '../src/features/conversation/conversation-model.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (...parts) => fs.readFileSync(path.join(here, '..', ...parts), 'utf8');
const source = read('src', 'platform', 'tauri', 'bridge', 'multiagent.js');
const panelSource = read('src', 'features', 'multiagent', 'SubagentTranscriptPanel.jsx');
const toolRenderersSource = read('src', 'features', 'tools', 'tool-renderers.jsx');
const timelineSource = read('src', 'features', 'conversation', 'ConversationTimeline.jsx');
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx');
const commandSource = read('src-tauri', 'src', 'app', 'commands', 'multiagent.rs');
const chatCommandSource = read('src-tauri', 'src', 'app', 'commands', 'chat.rs');
const memoryCommandSource = read('src-tauri', 'src', 'app', 'commands', 'memory.rs');
const interactionCommandSource = read('src-tauri', 'src', 'app', 'commands', 'interaction.rs');
const interactionBridgeSource = read('src', 'platform', 'tauri', 'bridge', 'interaction.js');
const settingsSource = read('src', 'features', 'settings', 'SettingsView.jsx');
const i18nSource = read('src', 'shared', 'i18n.js');
const poolSource = read('src-tauri', 'src', 'features', 'assistant', 'engine_pool.rs');
const modeStateSource = read('src-tauri', 'src', 'core', 'mode_state.rs');
const rosterSource = read('src-tauri', 'src', 'features', 'multiagent', 'roster.rs');
const personasSource = read('src-tauri', 'src', 'features', 'personas', 'mod.rs');

test('空白新对话切换多智能体后立即通知界面，且不提前物化会话', async () => {
  const root = {};
  vm.runInNewContext(interactionBridgeSource, { window: root, globalThis: root });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.interaction;
  const state = {
    activeSessionId: null,
    pendingDraftMultiAgent: false,
    modeState: { mode: 'yolo', multiAgent: false },
  };
  let notifyCount = 0;
  let invokeCount = 0;
  const api = factory({
    state,
    notify() { notifyCount += 1; },
    async invoke() { invokeCount += 1; return {}; },
  });

  await api.setMultiAgentMode(true);
  assert.equal(state.pendingDraftMultiAgent, true);
  assert.equal(state.modeState.multiAgent, true);
  assert.equal(notifyCount, 1, '草稿态开启后必须立即发布一次状态快照');
  assert.equal(invokeCount, 0, '开关本身不得为了反馈而提前创建后端会话');

  await api.setMultiAgentMode(false);
  assert.equal(state.pendingDraftMultiAgent, false);
  assert.equal(state.modeState.multiAgent, false);
  assert.equal(notifyCount, 2, '草稿态关闭也必须立即反馈，且前一次调用不得卡住 in-flight 闸');
  assert.equal(invokeCount, 0);
});

/** 装载 bridge 模块并取回它注册的工厂，用最小 context 跑出内部函数。 */
function loadFeature(invokeImpl) {
  const dispatched = [];
  const root = {
    dispatchEvent(event) { dispatched.push(event); return true; },
    CustomEvent: class {
      constructor(type, options) {
        this.type = type;
        this.detail = options && options.detail;
      }
    },
  };
  vm.runInNewContext(source, { window: root, globalThis: root });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.multiagent;
  const listeners = {};
  const state = { multiAgent: {} };
  const api = factory({
    state,
    notify() {},
    invoke: invokeImpl || (async () => ({})),
    listen(name, fn) { listeners[name] = fn; },
  });
  return { api, state, listeners, dispatched };
}

// ── 桥：薄层契约 ────────────────────────────────────────────────────────────

test('桥不再维护运行状态机，只暴露发起与只读投影', () => {
  const { api, listeners } = loadFeature();
  assert.deepEqual(
    Object.keys(api).sort(),
    ['listSubagentTranscripts', 'readSubagentTranscript'],
    '台账/审批/运行列表与 startRun 独立入口的 API 已随 ADR-0006 退役',
  );
  assert.deepEqual(
    Object.keys(listeners).sort(),
    ['workflow:agent_complete', 'workflow:agent_progress'],
    '只监听子智能体进展/完成，不重建运行状态机',
  );
  assert.doesNotMatch(source, /workflow:approval_required/, '审批链已退役');
  assert.doesNotMatch(source, /awaiting_approval/, '运行状态机已退役');
});

test('详情读取把 offset/revision 游标原样交给后端', async () => {
  const calls = [];
  const chunk = { messages: [], next_offset: 42, revision: 'rev-1', reset: false };
  const { api } = loadFeature(async (name, args) => {
    calls.push({ name, args });
    return chunk;
  });

  const initial = await api.readSubagentTranscript('run-1', 'agent_a');
  assert.equal(initial.next_offset, 42);
  assert.equal(calls[0].name, 'read_subagent_transcript');
  assert.equal(Object.hasOwn(calls[0].args, 'offset'), false, '首次读取不伪造游标');

  await api.readSubagentTranscript('run-1', 'agent_a', { offset: 42, revision: 'rev-1' });
  assert.equal(calls[1].args.offset, 42);
  assert.equal(calls[1].args.revision, 'rev-1');
});

test('子智能体事件由共享桥转成 DOM 事件，是否投影由会话视图决定', () => {
  const { listeners, dispatched } = loadFeature();
  listeners['workflow:agent_progress']({
    payload: { session_id: 'chat-1', agent_id: 'agent_a1', role_id: 'scout', status: '检索中' },
  });
  listeners['workflow:agent_complete']({
    payload: { session_id: 'wf-2', agent_id: 'agent_b2', role_id: 'builder', failed: true },
  });
  assert.equal(dispatched.length, 2, '普通会话（非 wf-）的子智能体事件同样转发');
  assert.equal(dispatched[0].type, 'pinvou:subagent-update');
  assert.equal(dispatched[0].detail.sessionId, 'chat-1');
  assert.equal(dispatched[0].detail.agentId, 'agent_a1');
  assert.equal(dispatched[0].detail.status, '检索中');
  assert.equal(dispatched[1].detail.done, true);
  assert.equal(dispatched[1].detail.failed, true);
});

// ── 停止/回收：取消级联（ADR-0006 批次 A 的行为锁） ─────────────────────────

test('停止按钮与引擎回收都级联取消子智能体', () => {
  const cancelStart = poolSource.indexOf('pub async fn cancel(');
  const cancelEnd = poolSource.indexOf('pub async fn', cancelStart + 10);
  const cancelBody = poolSource.slice(cancelStart, cancelEnd);
  assert.ok(
    cancelBody.includes('Op::CancelSubAgents'),
    '主对话「停止」按钮是子智能体唯一确定性停止入口，必须级联取消',
  );
  assert.match(
    poolSource,
    /fn shutdown_cancel_cascade_ops\(\) -> \[Op; 2\] \{\s*\[Op::CancelSubAgents, Op::Shutdown\]/,
    '回收路径先取消后关闭，顺序由同通道 FIFO 保证',
  );
});

// ── 会话级开关 + 每轮委派提醒（Rust 源结构契约） ─────────────────────────────

test('旧独立入口退役：多智能体经会话级开关 + 每轮注入委派提醒', () => {
  assert.doesNotMatch(commandSource, /start_workflow_run/, '独立入口命令已退役');
  assert.match(commandSource, /pub\(crate\) fn delegation_reminder\(task: &str\)/, 'Work 多智能体按本轮任务生成委派提醒');
  assert.match(commandSource, /pub\(crate\) fn prepend_delegation_reminder\(/, '普通发送、编辑重发与方案接受必须复用统一提醒组装');
  assert.match(commandSource, /roster::available_role_lines\(task\)/, '名册必须按本轮任务筛选并随提醒带上');
  assert.match(rosterSource, /EXPERT_CANDIDATE_LIMIT:\s*usize\s*=\s*20/, '父模型每轮最多看到 20 位专家短候选');
  assert.match(rosterSource, /personas::executable_summaries\(\)/, '每轮匹配必须只读取轻量摘要，不克隆全部专家正文');
  assert.match(personasSource, /pub fn executable_summaries\(\)[\s\S]{0,900}filter\(\|card\| !card\.conversational_only\)/, '纯对话专家卡不得注册为执行型子智能体');
  assert.match(rosterSource, /if card\.source == "user" \|\| score > 0/, '用户自创专家优先保留，内置专家按本轮相关性入选');
  assert.match(
    commandSource,
    /串行的“修改→测试→审查”接力[\s\S]{0,120}默认共享工作区/,
    '串行修改、测试与审查必须复用共享工作区',
  );
  assert.match(
    commandSource,
    /工作区不得安排两个及以上并行写入者。同一 Git 仓库确需并行写入时\\\r?\n\s*必须使用 `workspace_policy=worktree`/,
    '同一仓库的并行写入必须使用 worktree',
  );
  assert.match(
    commandSource,
    /Git、基线或 worktree 准备失败时，说明原因并将并行写入任务改为\\\r?\n\s*串行/,
    'worktree 无法准备时必须把并行写入改为串行',
  );
  assert.match(
    chatCommandSource,
    /prepend_delegation_reminder\([\s\S]{0,180}mode_state\.multi_agent,[\s\S]{0,80}&raw_message,[\s\S]{0,40}full/,
    '开关开启时 chat 发送链按原始用户消息筛专家并拼提醒',
  );
  assert.match(
    memoryCommandSource,
    /prepend_delegation_reminder\([\s\S]{0,180}mode_state\.multi_agent,[\s\S]{0,80}&new_message,[\s\S]{0,80}new_message\.clone\(\)/,
    '编辑重发必须重新注入本轮委派提醒',
  );
  assert.match(
    memoryCommandSource,
    /reserve_turn\(&sid\)[\s\S]{0,220}mode_state\(&sid\)/,
    '编辑重发必须先占 turn 槽再读取开关，不能与模式切换交错',
  );
  assert.match(
    memoryCommandSource,
    /user_display_message\(new_message\)[\s\S]{0,250}edit_last_turn_reserved\(&sid, full, display_message, reservation\)/,
    '编辑重发的模型提醒不得污染界面与落盘历史',
  );
  assert.match(
    interactionCommandSource,
    /prepend_delegation_reminder\([\s\S]{0,180}accepted_mode_state\.multi_agent,[\s\S]{0,80}&plan_markdown,[\s\S]{0,80}accept_plan_instruction\(&plan_markdown\)/,
    '接受方案触发执行时必须按批准后的开关状态注入委派提醒',
  );
  assert.match(
    interactionCommandSource,
    /pub async fn set_multi_agent_mode\(/,
    '开关命令在 interaction 域',
  );
  assert.match(
    interactionCommandSource,
    /validate_session_id\(&session_id\)[\s\S]{0,900}reconfigure_multi_agent_mode/,
    '任何副作用之前必须先做 id 形状校验（paths 只是 join，防 ../ 穿越）',
  );
  assert.match(
    interactionCommandSource,
    /\.load\(&session_id\)[\s\S]{0,900}reconfigure_multi_agent_mode/,
    '会话必须确实存在才允许做副作用（防孤儿目录）',
  );
  assert.match(
    interactionCommandSource,
    /reconfigure_multi_agent_mode\(&session_id, enabled\)/,
    '开关切换必须走资源策略重配入口',
  );
  assert.match(
    poolSource,
    /reconfigure_multi_agent_mode[\s\S]{0,500}\.reserve\(\)[\s\S]{0,700}create_dir_all[\s\S]{0,900}set_multi_agent\(session_id, enabled\)[\s\S]{0,300}evict_locked\(session_id\)/,
    '生成中拒绝切换；名册装配、状态持久化与旧引擎回收必须和发送原子串行',
  );
  assert.match(
    poolSource,
    /Op::SetFleetRoster \{ roster \}/,
    'live 名册刷新走底座 SetFleetRoster，而不是改写工具列表',
  );
  assert.match(
    poolSource,
    /has_expert_role_projection\(&workspace\)[\s\S]{0,100}targets\.insert\(session_id\)/,
    '已关闭开关但仍加载旧专家投影的在跑会话，也必须随专家增删改刷新',
  );
  assert.doesNotMatch(
    rosterSource,
    /tools\.insert\("posture"/,
    '专家名册只承载身份与人设，不得另设专家级权限姿态',
  );
  assert.doesNotMatch(
    poolSource,
    /workflow_host_disallowed_tools/,
    '广播/回收不再按会话形态改写禁用列表——工具面与主线持平',
  );

  const assistantBridgeSource = read('src-tauri', 'src', 'features', 'assistant', 'platform', 'bridge.rs');
  const engineSource = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
  assert.match(assistantBridgeSource, /MULTI_AGENT_MAX_SPAWN_DEPTH:\s*u32\s*=\s*2/);
  assert.match(assistantBridgeSource, /MULTI_AGENT_WORK_MAX_CONCURRENT:\s*usize\s*=\s*4/);
  assert.match(assistantBridgeSource, /MULTI_AGENT_WORK_MAX_ADMITTED:\s*usize\s*=\s*8/);
  assert.doesNotMatch(assistantBridgeSource, /MULTI_AGENT_CODE_MAX_/);
  assert.match(
    assistantBridgeSource,
    /build_multi_agent_send_message_op[\s\S]{0,700}build_multi_agent_hook_executor/,
    'SendMessage 每轮覆盖 engine hook，因此多智能体发送路径必须重新携带深度护栏',
  );
  assert.match(
    engineSource,
    /if self\.multi_agent_enabled[\s\S]{0,500}build_multi_agent_send_message_op/,
    '只有多智能体引擎使用受限发送路径，普通对话保持底座行为',
  );
  assert.match(
    poolSource,
    /late sweep of deleted chat/,
    '普通会话删除一律延迟清扫（裸 agent 对所有会话可用，不只开关开启的）',
  );
  assert.match(
    poolSource,
    /spawned_at_ms: Self::now_epoch_ms\(\)/,
    '引擎必须记录纪元时间戳，供 transcripts 甄别上一进程的僵尸 worker',
  );
  const transcriptsSource = read('src-tauri', 'src', 'features', 'multiagent', 'transcripts.rs');
  assert.match(
    transcriptsSource,
    /fn projected_worker_status\(/,
    '非终态 worker 的存活判定必须走纪元比对（引擎存在 ≠ 老 worker 还活着）',
  );
  const multiagentCommandSource2 = read('src-tauri', 'src', 'app', 'commands', 'multiagent.rs');
  assert.match(
    multiagentCommandSource2,
    /engine_epoch_ms\(&run_id\)/,
    'transcripts 命令传引擎纪元而非 handle_for 存在性',
  );
  assert.doesNotMatch(
    interactionCommandSource,
    /ensure_default_roles/,
    '不再播种旧版默认角色——专家池卡片统一投影为 exp-* profile',
  );
  assert.match(
    poolSource,
    /enroll_expert_roles\(&workspace\)[\s\S]{0,80}\.map_err/,
    '专家名册写盘失败必须让开启失败，不得静默成功',
  );
  assert.match(
    poolSource,
    /reconfigure_multi_agent_mode[\s\S]{0,700}if enabled && !self\.multi_agent_mode_available\(session_id\)[\s\S]{0,900}self\.bridge\.session_workspace\(session_id\)/,
    '开启前必须先执行能力门禁；允许开启的 Work 会话仍按实际 CodeWhale 工作区装配名册',
  );
  assert.match(
    assistantBridgeSource,
    /enroll_expert_roles\(&cfg\.workspace\)[\s\S]{0,1800}FleetRoster::load\([\s\S]{0,160}&cfg\.workspace/,
    '引擎启动时名册写入与读取必须共同遵循基座 workspace',
  );
  const sessionsSource = read('src-tauri', 'src', 'features', 'sessions', 'mod.rs');
  assert.match(
    sessionsSource,
    /fn save_multi_agent_flags/,
    '开关必须持久化——Web 门禁与每轮注入都依据它，重启不能静默关闭',
  );
  assert.match(
    sessionsSource,
    /load_skill_bindings\(\);\s*store\.load_multi_agent_flags\(\)/,
    '启动时必须恢复开关清单',
  );
  assert.match(
    sessionsSource,
    /pub fn set_multi_agent\([\s\S]{0,400}-> Result<\(\)>/,
    'set_multi_agent 必须把落盘失败向上抛（界面不得谎报开启成功）',
  );
  assert.match(
    sessionsSource,
    /with_extension\("json\.tmp"\)[\s\S]{0,300}fs::rename/,
    '开关清单必须 tmp+rename 原子替换，进程中途退出不得留半个 JSON',
  );
  assert.match(
    sessionsSource,
    /pub fn set_multi_agent\([\s\S]{0,420}multi_agent_flags_io\.lock\(\)/,
    '「改内存→落盘→回滚」整个事务必须持有互斥，回滚不得覆盖并发新状态',
  );
  assert.match(
    sessionsSource,
    /ghost cleanup/,
    '启动加载必须对账剔除幽灵 id 并重写清单',
  );
  assert.match(
    poolSource,
    /set_multi_agent\(session_id, enabled\)[\s\S]{0,300}evict_locked\(session_id\)/,
    '开关持久化成功后必须回收旧引擎，下一轮按权威状态重建，不得保留旧资源策略',
  );
  assert.match(
    sessionsSource,
    /retention purge failed/,
    '保留策略清理必须同步更新 _multi_agent.json（防幽灵 id）',
  );
  const personasCommandSource = read('src-tauri', 'src', 'app', 'commands', 'personas.rs');
  const expertSyncCallCount = (personasCommandSource.match(/sync_expert_rosters\(&app, &pool\)\.await/g) || []).length;
  assert.equal(
    expertSyncCallCount,
    3,
    '专家卡增/改/删都要联动刷新开着开关的会话名册（否则 live 引擎报 profile 不存在）',
  );
  assert.match(
    personasCommandSource,
    /multiagent:roster_sync_failed/,
    '联动失败不改写卡操作结果（否则前端不刷新卡池、重试造重复卡），必须走警示事件如实提示',
  );
});

// ── workflow 与主线持平：不禁用、不教学（2026-08-03 复审校正） ────────────────

test('workflow 保持主线原状：不禁用；提醒不教它；快照供 worktree 检出', () => {
  // 提醒文案不提 workflow 的契约由 Rust 单测
  // delegation_reminder_never_mentions_the_workflow_path 在运行时钉死。
  const bridgeSource = read('src-tauri', 'src', 'features', 'assistant', 'platform', 'bridge.rs');
  assert.match(
    bridgeSource,
    /pub fn build_engine_config_for_multi_agent\(/,
    '多智能体配置只装名册',
  );
  assert.doesNotMatch(
    bridgeSource,
    /push\(WORKFLOW_TOOL_NAME/,
    '基础配置不得追加 workflow 禁令——全局禁用是对主线的能力回退',
  );
  assert.doesNotMatch(
    poolSource,
    /deny_workflow_tool_for_chat/,
    '聊天侧禁用漏斗不得追加 workflow',
  );
});

// ── spawn 判定与折叠豁免（P2-2） ─────────────────────────────────────────────

test('只有 spawn 型 agent 调用算一次委派；协调操作不算', () => {
  assert.ok(isExpertDelegationCall('agent', { prompt: '去调研' }));
  assert.ok(isExpertDelegationCall('agent', { action: 'start', items: [{ type: 'text', text: 'x' }] }));
  assert.ok(isExpertDelegationCall('agent', { message: 'x' }));
  assert.ok(!isExpertDelegationCall('agent', { action: 'wait', agent_id: 'agent_1' }), 'wait 不是委派');
  assert.ok(!isExpertDelegationCall('agent', { action: 'status' }), 'status 不是委派');
  assert.ok(!isExpertDelegationCall('agent', { action: 'cancel', agent_id: 'agent_1' }), 'cancel 不是委派');
  assert.ok(!isExpertDelegationCall('agent', {}), '无任务正文不算派工');
  assert.ok(!isExpertDelegationCall('web_search', { prompt: 'x' }), '其它工具不受影响');
  assert.ok(!isExpertDelegationCall('agent', { items: [] }), '空 items 不是有效派工');
});

test('items 任务正文与底座同源归一，专家卡不会显示空摘要', () => {
  assert.equal(
    expertDelegationText({ items: [
      { type: 'text', text: ' 调研市场规模 ' },
      { type: 'mention', name: 'brief', path: '/tmp/brief.md' },
    ] }),
    '调研市场规模\n[mention:brief](/tmp/brief.md)',
  );
  assert.equal(expertDelegationText({ prompt: ' 直接任务 ', items: [{ type: 'text', text: 'ignored' }] }), '直接任务');
});

test('spawn 型 agent 项不折进工具组；协调操作与普通工具照常归组', () => {
  const spawn = { id: 's1', type: 'tool', tool: { name: 'agent' }, legacyItem: { args: { prompt: '去调研' } } };
  const wait = { id: 'w1', type: 'tool', tool: { name: 'agent' }, legacyItem: { args: { action: 'wait', agent_id: 'a' } } };
  const search = { id: 'q1', type: 'tool', tool: { name: 'web_search', rawInput: {} } };
  const presented = presentConversationItems([spawn, wait, search]);
  assert.equal(presented.length, 2, 'spawn 独立成项，wait+search 折成一组');
  assert.equal(presented[0], spawn, '专家卡项原样保留在组外（历史加载不再被默认折叠藏起）');
  assert.equal(presented[1].type, 'tool_group');
  assert.deepEqual(presented[1].items.map(item => item.id), ['w1', 'q1']);

  // Codex ACP 的同名外部工具没有 legacyItem，不受豁免影响。
  const codexAgent = { id: 'c1', type: 'tool', tool: { name: 'agent', rawInput: { prompt: 'x' } } };
  const codexPresented = presentConversationItems([codexAgent]);
  assert.equal(codexPresented[0].type, 'tool_group');
});

test('开关 UI 挂在模型列表下方，经 interaction 桥调后端', () => {
  assert.match(settingsSource, /data-testid="multiagent-toggle"/, '模型选择器弹层里必须有开关行');
  assert.match(settingsSource, /bridge\.interaction\.setMultiAgentMode/, '开关走 interaction 桥');
  assert.match(
    interactionBridgeSource,
    /invoke\("set_multi_agent_mode", \{ sessionId: sid, enabled: !!enabled \}\)/,
    '桥逐字传 enabled',
  );
  assert.match(
    interactionBridgeSource,
    /multiAgentToggleInFlight\.has\(flightKey\)/,
    '桥必须按会话 in-flight 丢弃重复切换（全局布尔会让 A 在途时殃及 B 的开关）',
  );
  assert.match(
    interactionBridgeSource,
    /state\.pendingDraftMultiAgent = !!enabled;/,
    '草稿态开开关只寄存意图，不得物化会话（否则左侧列表凭空多一条空对话）',
  );
  assert.doesNotMatch(
    interactionBridgeSource.slice(
      interactionBridgeSource.indexOf('async function setMultiAgentMode'),
      interactionBridgeSource.indexOf('// plan-stuck'),
    ),
    /ensureSession\(/,
    '开关本身不允许创建会话（注释提及不算，禁止的是调用）',
  );
  const sessionsBridgeSource = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
  assert.match(
    sessionsBridgeSource,
    /pendingMultiAgent = state\.pendingDraftMultiAgent === true;[\s\S]{0,900}set_multi_agent_mode/,
    '寄存的开关意图在首条消息物化会话时落后端',
  );
  assert.match(
    sessionsBridgeSource,
    /function switchActiveTo\(id, opts\) \{\s*\/\/[^\n]*\n    state\.pendingDraftMultiAgent = false;/,
    '离开草稿时未消费的寄存意图必须作废',
  );
  assert.match(
    settingsSource,
    /disabled=\{multiAgentBusy \|\| busy\}/,
    '切换期间或当前回复生成中必须禁用开关按钮',
  );
  assert.match(i18nSource, /关闭会回收引擎，并取消仍在运行的子智能体/, '中文开关文案必须如实说明关闭会取消');
  assert.match(i18nSource, /turning it off recycles the engine and cancels any subagents still running/, '英文开关文案必须如实说明关闭会取消');
  assert.match(i18nSource, /オフにするとエンジンを回収し、実行中のサブエージェントをキャンセルする/, '日文开关文案必须如实说明关闭会取消');
  assert.doesNotMatch(
    i18nSource,
    /关闭不影响在跑的子智能体|turning it off never interrupts running subagents|オフにしても実行中のサブエージェントは中断されない/,
    '不得再保留与 ADR-0006 和实际回收行为相反的旧文案',
  );
  assert.match(
    modeStateSource,
    /关闭停止注入并回收引擎，取消\s*\/\/\/ 仍在后台运行的子智能体/,
    'Rust 状态注释必须与关闭时的取消级联一致',
  );
  assert.match(
    interactionBridgeSource,
    /previousMultiAgent[\s\S]{0,600}invoke\("set_multi_agent_mode"/,
    '点击必须乐观翻转（名册装配与引擎同步可能耗时，等返回才翻拨杆像点了没反应），失败回滚',
  );
  assert.match(
    interactionBridgeSource,
    /catch \(invokeError\) \{[\s\S]{0,300}runOnSession\(sid, function \(\) \{/,
    '开关失败的回滚与报错必须定向回触发会话——await 期间用户可能已切走（复核 P1）',
  );
  const transcriptsSource = read('src-tauri', 'src', 'features', 'multiagent', 'transcripts.rs');
  assert.match(
    transcriptsSource,
    /pub objective: Option<String>/,
    '清单必须携带任务目标：同角色多子智能体靠它区分（复核 P1）',
  );
  assert.match(
    transcriptsSource,
    /pub has_transcript: bool/,
    '清单以 worker ledger 为主表：排队/刚启动（无 transcript）的子智能体必须可见（复核 P1）',
  );
  assert.match(
    transcriptsSource,
    /pub parent_run_id: Option<String>/,
    '清单必须携带直接父代理，不能把多级代理全部伪装成同级成员',
  );
  assert.match(
    transcriptsSource,
    /pub spawn_depth: Option<u32>/,
    '清单必须保留底座派生深度，遗留记录允许未知',
  );
  assert.match(
    transcriptsSource,
    /pub struct SubagentTranscriptChunk[\s\S]{0,260}pub next_offset: u64[\s\S]{0,160}pub revision: String[\s\S]{0,120}pub reset: bool/,
    '详情轮询必须使用可复位的字节游标，不能每 1.5 秒整读完整 transcript',
  );
  const multiagentBridgeSource = read('src', 'platform', 'tauri', 'bridge', 'multiagent.js');
  assert.match(
    multiagentBridgeSource,
    /return null;/,
    '读取失败返回 null 而不是 []——界面要能区分"没有子智能体"和"读取失败"（复核 P2）',
  );
  const panelSource = read('src', 'features', 'multiagent', 'SubagentTranscriptPanel.jsx');
  assert.match(
    panelSource,
    /listReadFailed/,
    '面板必须展示读取失败态并保留上次有效清单，不能清空',
  );
  assert.match(
    panelSource,
    /const terminalWithoutTranscript =/,
    '子智能体在建 transcript 前失败时必须直接显示 ledger 错误，不能再发必败读取',
  );
  assert.match(panelSource, /copy\.agentNoTranscript\(agent && agent\.error\)/);
  assert.match(panelSource, /const agentResolved = !!agent/);
  assert.match(panelSource, /const transcriptUnavailable = !!\(agent && agent\.has_transcript === false\)/);
  assert.match(
    panelSource,
    /subtitle \|\| presentation\.task \|\| entry\.agent_id/,
    '清单行优先展示专家身份副标题；无任务标题时展示任务目标，遗留行回退 agent_id',
  );
  assert.match(
    toolRenderersSource,
    /const \{ identity, name, subtitle \} = presentation;/,
    '行内卡与右侧清单共用任务主标题、专家身份副标题的投影结果',
  );
  assert.match(
    transcriptsSource,
    /fn read_header_agent_id/,
    '清单轮询只读表头行认身份，正文仅"成功完成判受阻"时整读一次（复核 P2）',
  );
  assert.match(
    transcriptsSource,
    /Result<Vec<SubagentTranscriptSummary>, String>/,
    'ledger 损坏/权限错误必须如实上抛——吞成空表会把故障伪装成"没有子智能体"（复核 P2）',
  );
  const fileIngestSource2 = read('src-tauri', 'src', 'features', 'files', 'file_ingest.rs');
  assert.doesNotMatch(
    fileIngestSource2,
    /cfg!\(target_os/,
    'OS 差异必须走 platform 适配层（架构守卫 rust_target_cfg_outside_adapter）',
  );
  const engineSource = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
  assert.match(
    engineSource,
    /fn emit_turn_started\(/,
    'turn_started 与 admission 解耦：底座自启的续跑轮也要能宣告忙碌',
  );
  assert.match(
    engineSource,
    /transition\.newly_active[\s\S]{0,400}emit_turn_started\(app, session_id\)/,
    '无 admission 的续跑轮（子智能体完成后的父汇总轮）必须发 turn_started——否则界面空闲、停止缺席、再发消息撞"已有运行中轮次"（复核 P1）',
  );
  const managerSource = read('src-tauri', 'src', 'features', 'remote_control', 'manager.rs');
  assert.match(
    managerSource,
    /MULTI_AGENT_WEB_EXECUTION_DENYLIST/,
    'Web 只读必须由后端统一封禁执行入口——编辑重发/计划裁决/澄清提交都曾绕过（复核 P1）',
  );
  assert.match(
    managerSource,
    /validate_multi_agent_session_web_scope\(app, command, session_id\)/,
    '多智能体封禁必须挂在统一校验点，不散落各命令',
  );
  assert.match(
    chatViewSource,
    /editable=\{!busy && !isMultiAgentReadOnly && item\.id === lastUserId\}/,
    'Web 只读会话不得经"编辑最后一条"绕过发送',
  );
  assert.match(
    toolRenderersSource,
    /multiAgentWebReadOnly/,
    '计划裁决等执行型卡片操作在 Web 只读会话置灰（后端漏斗为权威）',
  );
  assert.match(
    toolRenderersSource,
    /webReadOnly && !item\.resolved/,
    '澄清卡在 Web 只读会话呈现为锁定说明，不留"能点但必败"的按钮（复核 P2）',
  );
  const sessionsBridgeSource2 = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
  assert.match(
    sessionsBridgeSource2,
    /delete_session[\s\S]{0,400}enterDraft\(\)[\s\S]{0,200}pendingDraftMultiAgent = true/,
    '草稿开关落盘失败必须中止物化并保留意图——首条消息不得静默退化成普通对话（复核 P1）',
  );
  const chatBridgeSource2 = read('src', 'platform', 'tauri', 'bridge', 'chat.js');
  assert.match(
    chatBridgeSource2,
    /prefillComposer\(text\);\s*return;/,
    '物化中止时输入必须回填输入框，不得静默丢字（复核 P1）',
  );
  assert.match(
    toolRenderersSource,
    /subagentRoleOrdinals\(list\)/,
    '轮询广播必须携带同角色序号，行内卡的 ①② 与面板同源一致',
  );
  assert.match(
    toolRenderersSource,
    /\.\.\.\(prev \|\| \{\}\), \.\.\.detail/,
    '实时事件不带 seq/blocked 等补字段，卡片状态必须字段合并，不得整包覆盖',
  );
  const personasBridgeSource = read('src', 'platform', 'tauri', 'bridge', 'personas.js');
  assert.match(
    personasBridgeSource,
    /__PINVOU_SHARED_I18N__/,
    '名册同步警示文案取自统一词典 src/shared/i18n.js，不在桥内表重复维护',
  );
  assert.match(
    interactionBridgeSource,
    /multiAgent: !!st\.multi_agent/,
    'modeState 镜像必须带 multiAgent，模式事件不得把开关状态冲掉',
  );
});

test('transcripts 命令仅读取 Work 会话自己的 CodeWhale 工作区', () => {
  assert.match(commandSource, /fn subagent_workspace\([\s\S]{0,100}pool: &EnginePool/);
  assert.match(
    commandSource,
    /fn subagent_workspace\([\s\S]{0,500}!pool\.multi_agent_mode_available\(session_id\)[\s\S]{0,220}pool\.session_workspace\(session_id\)/,
    '读取工作区前必须拒绝 Code 会话，避免把项目共享 .codewhale 当成会话记录',
  );
  assert.doesNotMatch(commandSource, /resolve_workflow_approval/, '审批命令已退役');
});

test('模型起名：任务说明第一行「」提取为子智能体显示名', () => {
  assert.deepEqual(
    splitSubagentTitle('「调研专家-AI新闻」检索过去24小时的AI要闻'),
    { name: '调研专家-AI新闻', rest: '检索过去24小时的AI要闻' },
  );
  assert.equal(splitSubagentTitle('没有起名的普通任务说明').name, null);
  assert.equal(splitSubagentTitle('「」空名不算').name, null);
  assert.equal(
    splitSubagentTitle('「这个名字实在太长了超过二十四个字符的上限所以不算数」正文').name,
    null,
    '超长不当名字，防整段说明被吞进标题',
  );
});

test('子智能体名称投影：任务标题为主、专家身份为副，旧记录及裸派逐级兜底', () => {
  const roleCards = {
    scout: '调研专家', manager: '规划专家', builder: '执行专家', reviewer: '审查专家', general: '通用执行者',
  };
  const personas = [{ id: 'market', name: '市场分析师', dept: 'market' }];
  const present = (input) => resolveSubagentPresentation({
    personas, roleCards, agentId: 'agent_12345678', ...input,
  });

  assert.equal(
    present({ role: 'exp-market', sessionName: 'ignored-model-name' }).name,
    '市场分析师',
    '没有任务标题的专家仍以真名兜底，且忽略机器 session name',
  );
  const namedExpert = present({
    role: 'exp-market',
    sessionName: 'reviewer-behavior',
    objective: '「评审-行为链路」追查完整调用链',
    ordinal: { seq: 3, count: 5 },
  });
  assert.equal(namedExpert.name, '评审-行为链路', '任务标题是同专家多实例的主标题');
  assert.equal(namedExpert.subtitle, '市场分析师', '专家池真名作为稳定身份副标题');
  assert.equal(namedExpert.task, '追查完整调用链');
  assert.equal(namedExpert.explicitName, '评审-行为链路');
  assert.doesNotMatch(namedExpert.name, /③/, '有明确任务标题时不再追加同角色序号');
  const legacyExpert = present({
    role: 'exp-market',
    sessionName: 'reviewer-behavior',
    objective: '追查完整调用链',
    ordinal: { seq: 3, count: 5 },
  });
  assert.equal(legacyExpert.name, '市场分析师 ③', '无任务标题的旧记录保留专家序号回退');
  assert.equal(legacyExpert.subtitle, null);
  assert.equal(
    present({ sessionName: 'research-ai-news', objective: '「提示词名称」任务' }).name,
    '提示词名称',
    '任务首行的界面名优先于机器 name/session_name',
  );
  assert.equal(
    present({ sessionName: 'agent_12345678', objective: '「资料核验员」任务' }).name,
    '资料核验员',
    '底座用 agent_id 回填的 session_name 是占位值，继续使用任务标题',
  );
  assert.equal(
    present({ agentType: 'explore', objective: '普通调研任务' }).name,
    '普通调研任务',
    '普通对话没起名时从模型写出的任务目标提炼名称',
  );
  assert.equal(present({ agentType: 'implementer', objective: '' }).name, '执行专家');
  assert.equal(present({ agentType: 'verifier', objective: '' }).name, '审查专家');
});

test('子智能体任务名提炼：优先结构化目标、清理标签并按字符安全截断', () => {
  assert.equal(
    subagentObjectiveName('SCOPE: 背景\nQUESTION: 深挖两个 AI 安全事件的完整时间线'),
    '深挖两个 AI 安全事件…',
  );
  assert.equal(subagentObjectiveName('  - TASK: 核查桥接字段  '), '核查桥接字段');
  assert.equal(
    subagentObjectiveName('请检查这是一个非常非常长而且需要被压缩展示的子智能体任务名称', 12),
    '检查这是一个非常非常长而…',
  );
  assert.equal(
    subagentObjectiveName('请向用户发送一句问候：“你好”。直接回复即可'),
    '向用户发送一句问候',
    '模型漏写界面名时，任务兜底也必须使用短名称',
  );
});

test('底座内置 role 别名映射成可读角色，不冒充自定义专家', () => {
  assert.equal(resolveSubagentIdentity('explorer', [], 'agent_a').roleKey, 'scout');
  assert.equal(resolveSubagentIdentity('verifier', [], 'agent_b').roleKey, 'reviewer');
  assert.equal(resolveSubagentIdentity('builder', [], 'agent_c').roleKey, 'builder');
});

test('同角色多实例：头像按实例散列，名字按登记序编号', () => {
  const list = [
    { agent_id: 'agent_aaaaaaa1', role: 'scout' },
    { agent_id: 'agent_aaaaaaa2', role: 'scout' },
    { agent_id: 'agent_aaaaaaa3', role: 'scout' },
    { agent_id: 'agent_bbbbbbb1', role: 'builder' },
  ];
  const ordinals = subagentRoleOrdinals(list);
  assert.equal(subagentOrdinalLabel(ordinals.get('agent_aaaaaaa1')), ' ①');
  assert.equal(subagentOrdinalLabel(ordinals.get('agent_aaaaaaa3')), ' ③');
  assert.equal(subagentOrdinalLabel(ordinals.get('agent_bbbbbbb1')), '', '同角色单实例不加序号');
  const first = resolveSubagentIdentity('scout', [], 'agent_aaaaaaa1');
  const second = resolveSubagentIdentity('scout', [], 'agent_aaaaaaa2');
  assert.notEqual(first.avatarKey, second.avatarKey, '同角色多实例头像必须按实例散列（四个同貌无法区分，真机点名）');
  assert.equal(
    resolveSubagentIdentity('scout', []).avatarKey,
    'wf-role-scout',
    '没有实例 id（如 spawn 尚未返回）时回退角色头像',
  );
});

test('无 role 的普通 agent 按 agent_type 分组编号', () => {
  const list = [
    { agent_id: 'agent_aaaaaaa1', agent_type: 'explore' },
    { agent_id: 'agent_aaaaaaa2', agent_type: 'explore' },
    { agent_id: 'agent_bbbbbbb1', agent_type: 'implementer' },
  ];
  const ordinals = subagentRoleOrdinals(list);
  assert.equal(subagentOrdinalLabel(ordinals.get('agent_aaaaaaa1')), ' ①');
  assert.equal(subagentOrdinalLabel(ordinals.get('agent_aaaaaaa2')), ' ②');
  assert.equal(subagentOrdinalLabel(ordinals.get('agent_bbbbbbb1')), '');
});

test('多级代理树：默认只列直属根，按父节点逐级展开并保留孤儿记录', () => {
  const list = [
    { agent_id: 'agent_root_a', parent_run_id: null, spawn_depth: 1 },
    { agent_id: 'agent_child_a', parent_run_id: 'agent_root_a', spawn_depth: 2 },
    { agent_id: 'agent_grandchild_a', parent_run_id: 'agent_child_a', spawn_depth: 3 },
    { agent_id: 'agent_root_b', parent_run_id: null, spawn_depth: 1 },
    { agent_id: 'agent_orphan', parent_run_id: 'agent_evicted', spawn_depth: 3 },
  ];
  const ids = rows => rows.map(row => row.entry.agent_id);

  const collapsed = visibleSubagentTreeRows(list, new Set());
  assert.deepEqual(ids(collapsed), ['agent_root_a', 'agent_root_b', 'agent_orphan']);
  assert.equal(collapsed[0].childCount, 1);

  const firstLevel = visibleSubagentTreeRows(list, new Set(['agent_root_a']));
  assert.deepEqual(ids(firstLevel), ['agent_root_a', 'agent_child_a', 'agent_root_b', 'agent_orphan']);
  assert.equal(firstLevel[1].depth, 1);

  const secondLevel = visibleSubagentTreeRows(
    list,
    new Set(['agent_root_a', 'agent_child_a']),
  );
  assert.deepEqual(
    ids(secondLevel),
    ['agent_root_a', 'agent_child_a', 'agent_grandchild_a', 'agent_root_b', 'agent_orphan'],
  );
  assert.equal(secondLevel[2].depth, 2);
  assert.deepEqual(
    subagentAncestorIds(list, 'agent_grandchild_a'),
    ['agent_root_a', 'agent_child_a'],
  );
});

test('主对话行内卡只投影自己的后代，按子节点逐级展开', () => {
  const list = [
    { agent_id: 'agent_root_a', parent_run_id: null },
    { agent_id: 'agent_child_a', parent_run_id: 'agent_root_a' },
    { agent_id: 'agent_grandchild_a', parent_run_id: 'agent_child_a' },
    { agent_id: 'agent_root_b', parent_run_id: null },
    { agent_id: 'agent_child_b', parent_run_id: 'agent_root_b' },
    { agent_id: 'agent_cycle', parent_run_id: 'agent_cycle' },
  ];
  const ids = rows => rows.map(row => row.entry.agent_id);

  const collapsed = visibleSubagentDescendantRows(list, 'agent_root_a', new Set());
  assert.deepEqual(ids(collapsed), ['agent_child_a']);
  assert.equal(collapsed[0].depth, 0);
  assert.equal(collapsed[0].childCount, 1);

  const expanded = visibleSubagentDescendantRows(
    list,
    'agent_root_a',
    new Set(['agent_child_a']),
  );
  assert.deepEqual(ids(expanded), ['agent_child_a', 'agent_grandchild_a']);
  assert.equal(expanded[1].depth, 1);
  assert.deepEqual(
    visibleSubagentDescendantRows(list, 'agent_missing', new Set()),
    [],
  );
  assert.equal(subagentTreeIsDone(list, 'agent_root_a'), false, '运行中的后代必须保持轮询');
  assert.equal(
    subagentTreeIsDone(list.map(entry => ({ ...entry, done: true })), 'agent_root_a'),
    true,
    '父节点与全部后代终态后才能停表',
  );
  assert.equal(subagentTreeIsDone(list, 'agent_missing'), false, '根记录未出现时不得提前停表');
});

// ── 行内专家卡（消息流内的委派可视化） ───────────────────────────────────────

test('agent 工具调用渲染成行内专家卡，点击打开只读面板', () => {
  assert.match(
    timelineSource,
    /if \(item\.type === 'tool' \|\| item\.type === 'command_execution' \|\| item\.type === 'file_change'\)/,
    '独立 agent 工具项不得被共享时间线丢弃',
  );
  assert.match(
    timelineSource,
    /function ToolItem[\s\S]*?const custom = renderToolItem && renderToolItem\(item\)/,
    '独立工具项与工具组必须共用产品级工具渲染器',
  );
  assert.match(
    toolRenderersSource,
    /if \(EXPERT_CARD_ENABLED && \(item\.name === 'agent' \|\| isAgentWaitCall\(item\.name, item\.args\)\)\) \{\s*return <ExpertAgentCard/,
    'agent 委派与新旧 wait 调用共用产品级展示，并按 capability 门禁（Web 无 multiAgent bridge）',
  );
  assert.match(
    toolRenderersSource,
    /const EXPERT_CARD_ENABLED = can\('multiAgent'\)/,
    '门禁是模块级常量，一次构建内恒定（Hook 数量稳定）',
  );
  assert.match(
    toolRenderersSource,
    /args\.profile \|\| args\.role/,
    '承担者以底座正式契约字段 profile 为准（role 是内置类型别名）',
  );
  assert.match(
    toolRenderersSource,
    /watchExpertCard\(sessionId, agentId\)/,
    '卡片必须接权威落盘轮询（实时事件会丢：拥塞/重启/停止级联）',
  );
  assert.match(
    toolRenderersSource,
    /pinvou:subagent-ledger-update/,
    '一次会话级 ledger 轮询必须把完整父子投影共享给全部行内卡',
  );
  assert.match(
    toolRenderersSource,
    /visibleSubagentDescendantRows\(ledger, agentId, expandedChildIds\)/,
    '主对话里的直属卡必须只显示自己的后代树',
  );
  assert.match(
    toolRenderersSource,
    /data-testid="expert-agent-child-card"/,
    '后代节点必须是可点开的专家卡，不展示原始 JSON',
  );
  assert.match(
    toolRenderersSource,
    /if \(prev && prev\.done && !detail\.done\) return prev;/,
    '终态 ratchet：迟到的非终态实时事件不得把卡翻回工作中',
  );
  assert.match(
    toolRenderersSource,
    /copy\.blockedTag/,
    '[BLOCKED] 的"完成"不得显示绿色完成',
  );
  assert.match(
    toolRenderersSource,
    /pinvou:open-subagent/,
    '点击整卡经 DOM 事件通知 ChatView 打开面板',
  );
});

test('子智能体 ID 只接受 CodeWhale 实例格式，不把 agent_id 字段名当成实例', () => {
  assert.equal(extractSubagentId('agent_id'), null);
  assert.equal(extractSubagentId('schema: { agent_id: string }'), null);
  assert.equal(
    extractSubagentId('{"agent_id":"agent_7fb1c7be","status":"running"}'),
    'agent_7fb1c7be',
  );
  assert.equal(extractSubagentId({ agent_id: 'agent_7A7D442F' }), 'agent_7A7D442F');
  assert.equal(extractSubagentId('agent_1234'), null, '非正式短 id 不得误绑卡片');
});

test('ChatView 监听打开事件并为工作会话挂载只读面板，旧运行条带已退役', () => {
  assert.match(chatViewSource, /pinvou:open-subagent/);
  assert.match(chatViewSource, /<SubagentTranscriptPanel/);
  assert.match(chatViewSource, /captureConversationScrollPosition\(/);
  assert.match(chatViewSource, /restoreConversationScrollPosition\(/);
  assert.match(
    chatViewSource,
    /selectionRequestId: \(current\?\.selectionRequestId \|\| 0\) \+ 1/,
    '重复点击同一代理卡也必须产生新的详情选择请求',
  );
  assert.match(
    panelSource,
    /\[sessionId, initialAgentId, selectionRequestId\]/,
    '详情返回列表后，相同 agentId 的新请求仍必须重新选中',
  );
  assert.doesNotMatch(chatViewSource, /MultiAgentRunStrip/, '阶段条/确认卡随台账退役');
});

test('子智能体侧栏开合后保持聊天阅读位置', () => {
  const browsing = { scrollHeight: 1000, scrollTop: 650, clientHeight: 250 };
  const browsingSnapshot = captureConversationScrollPosition(browsing, false);
  assert.deepEqual(browsingSnapshot, { stickToBottom: false, bottomGap: 100 });
  browsing.scrollHeight = 1400;
  assert.equal(restoreConversationScrollPosition(browsing, browsingSnapshot), 1050);

  const following = { scrollHeight: 1000, scrollTop: 750, clientHeight: 250 };
  const followingSnapshot = captureConversationScrollPosition(following, true);
  following.scrollHeight = 1400;
  assert.equal(restoreConversationScrollPosition(following, followingSnapshot), 1150);
});

// ── 只读面板（列表 → 详情） ─────────────────────────────────────────────────

test('面板是只读执行记录：列表→详情两级，复用共享对话时间线', () => {
  assert.match(panelSource, /<ConversationTimeline/, '必须复用共享对话时间线组件');
  assert.match(panelSource, /projectSubagentTranscript\(\{/);
  assert.match(panelSource, /startSubagentTranscriptPolling\(\{[\s\S]*agentResolved,[\s\S]*agentDone,/);
  assert.match(panelSource, /accept: isTranscriptChunk/);
  assert.match(panelSource, /transcriptCursorRef\.current/);
  assert.match(panelSource, /mergeTranscriptMessages\(current, chunk\)/);
  assert.match(panelSource, /windowSubagentTranscript\(projected, visibleTranscriptItems\)/);
  assert.match(panelSource, /copy\.showEarlierTranscript/);
  assert.match(
    panelSource,
    /active: \(list\) => !Array\.isArray\(list\) \|\| list\.some\(\(entry\) => !entry\.done\)/,
    '清单全部终态后必须停止定时轮询',
  );
  assert.match(panelSource, /pinvou:subagent-update/, '新子智能体实时事件必须能唤醒终态清单');
  assert.match(panelSource, /listSubagentTranscripts\(sessionId\)/, '列表来自底座落盘投影');
  assert.match(panelSource, /setSelectedAgentId\(null\)/, '详情可返回列表');
  assert.match(panelSource, /visibleSubagentTreeRows/, '多级代理清单必须按父子树折叠展示');
  assert.match(panelSource, /copy\.childAgentCount/, '有下级的代理必须显示数量提示');
  assert.match(panelSource, /copy\.agentsEmpty/, '空列表有说明');
  assert.match(panelSource, /copy\.blockedTag/, '受阻标注保留（仅展示提示）');
  assert.doesNotMatch(panelSource, /fixed inset-y-0/, '面板是 flex 内嵌列，不是浮层抽屉');
  assert.doesNotMatch(panelSource, /<textarea|<input/, '只读执行记录不是第二个聊天入口');
});

// ── transcript 适配层（纯函数） ─────────────────────────────────────────────

test('transcript 适配：tool_result 载体不开新轮，结果按 id 回填', () => {
  const { turns } = projectSubagentTranscript({
    messages: [
      { role: 'user', content: [{ type: 'text', text: '调研任务' }] },
      {
        role: 'assistant',
        content: [
          { type: 'thinking', thinking: '先搜索' },
          { type: 'text', text: '我来查一下。' },
          { type: 'tool_use', id: 'tu-1', name: 'web_search', input: { query: 'x' } },
        ],
      },
      {
        role: 'user',
        content: [{ type: 'tool_result', tool_use_id: 'tu-1', content: '搜索结果' }],
      },
      { role: 'assistant', content: [{ type: 'text', text: '结论如下。' }] },
    ],
    agent: { agentId: 'a1', role: 'scout', done: true, failed: false, error: null },
  });
  assert.equal(turns.length, 1, '一个子智能体 = 一个 turn，tool_result 载体不得切轮');
  const turn = turns[0];
  assert.equal(turn.userText, '调研任务');
  assert.equal(turn.status, 'Completed');
  assert.ok(turn.lifecycleKnown);

  const reasoning = turn.items.find((item) => item.type === 'reasoning');
  assert.equal(reasoning.text, '先搜索', 'thinking 字段必须改名为 text');
  const tool = turn.items.find((item) => item.type === 'tool');
  assert.equal(tool.tool.rawOutput, '搜索结果', '结果按 tool_use_id 回填');
  assert.equal(tool.status, 'completed');
  assert.equal(turn.operationCount, 1);
  assert.ok(
    turn.presentation.some((item) => item.type === 'tool_group'),
    '工具条目要折成 tool_group 紧凑组',
  );
});

test('transcript 适配：内部运行时信封不得渲染为任务指令气泡', () => {
  const envelopeText = [
    '<codewhale:runtime_event kind="subagent_completion" visibility="internal">',
    'This is an internal runtime event, not user input.',
    'panel child completion summary',
    '<codewhale:subagent.done>{"agent_id":"agent_1a2b3c4d","status":"completed"}</codewhale:subagent.done>',
    '</codewhale:runtime_event>',
  ].join('\n');
  const { turns } = projectSubagentTranscript({
    messages: [
      { role: 'user', content: [{ type: 'text', text: '真实任务指令' }] },
      { role: 'user', content: [
        { type: 'text', text: envelopeText },
        { type: 'text', text: '<turn_meta>\nInput provenance: subagent_handoff (non-authoritative)\n</turn_meta>' },
      ] },
      { role: 'assistant', content: [{ type: 'text', text: '继续执行' }] },
      { role: 'user', content: [
        { type: 'text', text: '<turn_meta>\nInput provenance: shell_completion (non-authoritative)\n</turn_meta>' },
      ] },
    ],
    agent: { agentId: 'a3', role: 'builder', done: true, failed: false, error: null },
  });
  assert.equal(turns.length, 1, '内部信封不得切出假轮次');
  assert.equal(turns[0].userText, '真实任务指令', '仅真实任务指令进入 userText');
  assert.ok(!JSON.stringify(turns[0]).includes('child completion summary'), '信封正文不得上屏');
  assert.ok(!JSON.stringify(turns[0]).includes('codewhale:runtime_event'), '信封 XML 不得进入展示');
  assert.ok(!JSON.stringify(turns[0]).includes('shell_completion'), '仅-provenance 形态同样不上屏');
});

test('transcript 适配：文件工具归 file_change，终态后不留转圈条目', () => {
  const { turns } = projectSubagentTranscript({
    messages: [
      { role: 'user', content: [{ type: 'text', text: '写报告' }] },
      {
        role: 'assistant',
        content: [
          { type: 'tool_use', id: 'tu-1', name: 'edit_file', input: { path: 'report.md' } },
          { type: 'tool_use', id: 'tu-2', name: 'exec_shell', input: { command: 'ls' } },
        ],
      },
    ],
    agent: { agentId: 'a2', role: 'builder', done: true, failed: true, error: 'boom' },
  });
  const [turn] = turns;
  const file = turn.items.find((item) => item.type === 'file_change');
  assert.equal(file.tool.locations[0].path, 'report.md');
  const command = turn.items.find((item) => item.type === 'command_execution');
  assert.equal(command.tool.kind, 'execute');
  assert.ok(
    turn.items.every((item) => item.status !== 'in_progress'),
    'agent 已终态时不能留下永远转圈的工具条目',
  );
  assert.equal(turn.status, 'Failed');
  assert.equal(turn.error, 'boom');
});

test('transcript 适配：v0.9.5 canonical File write/edit/patch 归 file_change，read 不算', () => {
  const { turns } = projectSubagentTranscript({
    messages: [
      { role: 'user', content: [{ type: 'text', text: '写文件' }] },
      {
        role: 'assistant',
        content: [
          { type: 'tool_use', id: 'tu-1', name: 'File', input: { action: 'write', path: 'notes/a.md' } },
          { type: 'tool_use', id: 'tu-2', name: 'File', input: { action: 'read', path: 'notes/a.md' } },
          { type: 'tool_use', id: 'tu-3', name: 'Bash', input: { command: 'ls' } },
          {
            type: 'tool_use', id: 'tu-4', name: 'File', input: {
              action: 'patch',
              patch: '*** Update File: notes/a.md\n*** Add File: notes/b.md',
            },
          },
        ],
      },
    ],
    agent: { agentId: 'a3', role: 'builder', done: true, failed: false },
  });
  const [turn] = turns;
  const writes = turn.items.filter((item) => item.type === 'file_change');
  assert.equal(writes.length, 2, 'File write/patch 归 file_change，read 不算');
  assert.equal(writes[0].tool.locations[0].path, 'notes/a.md');
  assert.deepEqual(
    writes[1].tool.locations.map(location => location.path),
    ['notes/a.md', 'notes/b.md'],
  );
  const command = turn.items.find((item) => item.type === 'command_execution');
  assert.equal(command.tool.kind, 'execute');
  assert.equal(command.tool.name, 'Bash');
});

test('transcript 适配：坏消息跳过不炸，+N -M 从 unified diff 数出', () => {
  const { turns } = projectSubagentTranscript({
    messages: [null, 'garbage', { role: 'assistant' }, { role: 'assistant', content: [{ type: 'text', text: 'ok' }] }],
    agent: null,
  });
  assert.equal(turns[0].items.length, 1);
  assert.equal(turns[0].status, 'running', '没有摘要时按进行中展示');

  assert.deepEqual(
    fileChangeStat('@@ -1,3 +1,4 @@\n-old\n+new\n+more\n context\nReplaced 1 occurrence in report.md'),
    { added: 2, removed: 1 },
  );
  assert.equal(fileChangeStat('Wrote 10 bytes'), null, '没有 diff 就不硬凑统计');
  assert.equal(fileChangeStat(null), null);
});

test('长 transcript 只渲染末尾窗口，向前展开不丢完整统计', () => {
  const items = [
    { id: 'm1', type: 'agent_message', text: 'one' },
    { id: 'm2', type: 'agent_message', text: 'two' },
    { id: 't1', type: 'tool', status: 'completed', tool: { name: 'a' } },
    { id: 't2', type: 'tool', status: 'completed', tool: { name: 'b' } },
    { id: 'm3', type: 'agent_message', text: 'three' },
  ];
  const projection = {
    turns: [{ id: 'agent_a', items, presentation: items, operationCount: 2 }],
  };
  const windowed = windowSubagentTranscript(projection, 3);
  assert.equal(windowed.hiddenCount, 2);
  assert.deepEqual(windowed.view.turns[0].items.map(item => item.id), ['t1', 't2', 'm3']);
  assert.equal(windowed.view.turns[0].presentation[0].type, 'tool_group');
  assert.equal(windowed.view.turns[0].presentation[0].items.length, 2);
  assert.equal(windowed.view.turns[0].operationCount, 2, '页脚统计仍基于完整记录');

  const complete = windowSubagentTranscript(projection, 10);
  assert.equal(complete.hiddenCount, 0);
  assert.equal(complete.view, projection, '无需窗口时复用原投影');
});

test('身份解析：内置角色稳定名片，exp-* 映射真卡，无匹配原样展示', () => {
  assert.deepEqual(resolveSubagentIdentity(null, []), {
    kind: 'builtin', roleKey: 'general', avatarKey: 'wf-role-general',
  });
  assert.equal(resolveSubagentIdentity('scout', []).roleKey, 'scout');
  assert.equal(resolveSubagentIdentity('scout', []).avatarKey, 'wf-role-scout');

  const personas = [{ id: 'user-市场-1a2b', name: '市场分析师', dept: 'market' }];
  // '市'、'场' 各折成一个 '-'：user-市场-1a2b → user----1a2b（与 Rust roster 同规则）
  const expert = resolveSubagentIdentity('exp-user----1a2b', personas);
  assert.equal(expert.kind, 'expert', 'slug 折叠规则与 Rust roster 一致才能对上');
  assert.equal(expert.personaName, '市场分析师');
  assert.equal(expert.avatarKey, 'user-市场-1a2b', '专家头像用真卡 id');

  const unknown = resolveSubagentIdentity('exp-deleted-card', personas);
  assert.equal(unknown.kind, 'custom', '专家卡被删后原样展示、不冒充');
  assert.equal(unknown.avatarKey, 'wf-role-exp-deleted-card');
  assert.equal(resolveSubagentIdentity('wizard', []).name, 'wizard');
});

// ── 轮询（沿用既有回归；落盘/实时合并已收敛到 Rust transcripts::list） ─────────

test('增量 transcript chunk 追加新消息，reset 时替换旧消息', () => {
  const first = { messages: [{ id: 1 }], next_offset: 10, revision: 'r1', reset: false };
  assert.equal(isTranscriptChunk(first), true);
  assert.equal(isTranscriptChunk([{ id: 1 }]), false, '旧的整表数组不再是详情协议');
  assert.equal(isTranscriptChunk({ ...first, next_offset: -1 }), false);

  assert.deepEqual(mergeTranscriptMessages(null, first), [{ id: 1 }]);
  const current = [{ id: 1 }];
  const unchanged = mergeTranscriptMessages(
    current,
    { messages: [], next_offset: 10, revision: 'r1', reset: false },
  );
  assert.equal(unchanged, current, '没有新增消息时复用引用，避免无意义重渲染');
  assert.deepEqual(
    mergeTranscriptMessages(current, { messages: [{ id: 2 }], next_offset: 20, revision: 'r2', reset: false }),
    [{ id: 1 }, { id: 2 }],
  );
  assert.deepEqual(
    mergeTranscriptMessages(current, { messages: [{ id: 9 }], next_offset: 8, revision: 'r9', reset: true }),
    [{ id: 9 }],
    '文件被截断/替换后必须清掉旧投影',
  );
});

test('transcript 运行中串行轮询，停止后取消 timer；终态只做最后一次读取', async () => {
  const reads = [];
  let resolveRead;
  const pendingRead = new Promise((resolve) => { resolveRead = resolve; });
  const timers = [];
  const stop = startTranscriptPolling({
    read: () => { reads.push(Date.now()); return pendingRead; },
    onMessages: () => {},
    active: true,
    intervalMs: 5,
    schedule: (callback) => { timers.push(callback); return timers.length; },
    cancel: () => { timers.length = 0; },
  });
  assert.equal(reads.length, 1, '启动即读一次');
  assert.equal(timers.length, 0, '上一次未完成不排下一次');
  resolveRead([]);
  await pendingRead;
  await Promise.resolve();
  assert.equal(timers.length, 1, '完成后才排下一次');
  stop();
  assert.equal(timers.length, 0, '停止后取消 timer');

  const finalReads = [];
  startTranscriptPolling({
    read: async () => { finalReads.push(1); return []; },
    onMessages: () => {},
    active: false,
    schedule: () => { throw new Error('终态不得再排 timer'); },
    cancel: () => {},
  });
  await Promise.resolve();
  assert.equal(finalReads.length, 1, '终态只做最后一次读取');

  let dynamicSchedules = 0;
  startTranscriptPolling({
    read: async () => [{ agent_id: 'agent_done', done: true }],
    onMessages: () => {},
    active: (list) => !Array.isArray(list) || list.some((entry) => !entry.done),
    schedule: () => { dynamicSchedules += 1; return dynamicSchedules; },
    cancel: () => {},
  });
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(dynamicSchedules, 0, '全部子智能体终态后停止列表轮询');
});

test('详情清单未解析或终态无 transcript 时不发起读取', async () => {
  const cases = [
    {
      name: '清单尚未解析',
      agentResolved: false,
      transcriptUnavailable: false,
      agentDone: false,
    },
    {
      name: '终态且无 transcript',
      agentResolved: true,
      transcriptUnavailable: true,
      agentDone: true,
    },
  ];

  for (const testCase of cases) {
    let reads = 0;
    const stop = startSubagentTranscriptPolling({
      bridgeAvailable: true,
      selectedAgentId: 'agent_12345678',
      agentResolved: testCase.agentResolved,
      transcriptUnavailable: testCase.transcriptUnavailable,
      agentDone: testCase.agentDone,
      read: async () => {
        reads += 1;
        return { messages: [], next_offset: 0, revision: 'r1', reset: false };
      },
      accept: isTranscriptChunk,
      onMessages: () => {},
      schedule: () => { throw new Error(`${testCase.name} 不得安排详情轮询`); },
      cancel: () => {},
    });
    await Promise.resolve();
    assert.equal(stop, undefined, `${testCase.name} 不应启动详情轮询`);
    assert.equal(reads, 0, `${testCase.name} 的详情读取次数必须为 0`);
  }
});
// ── modeState 竞态回归：陈旧读取不得覆盖权威改写（审计意见）────────
// 装载 interaction 桥的 runtime 快照，用可控 invoke 精确编排异步返回顺序。
// 场景：syncModeState 先发起 get_mode_state（将返回旧值），toggle 先落盘
// （in-flight 清空）后旧读取才返回。只靠瞬时 in-flight 集合识别不了这种
// 顺序——epoch 校验必须在场（审计 P1）。
function loadInteractionRuntime() {
  const root = {};
  vm.runInNewContext(interactionBridgeSource, { window: root, globalThis: root });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.interaction;
  const state = {
    activeSessionId: 'chat-a',
    modeState: { mode: 'yolo', multiAgent: false },
    pendingDraftMultiAgent: false,
    chatItems: [],
    messages: [],
  };
  const deferred = {};
  const calls = [];
  const runtime = {
    state,
    calls,
    notifyCount: 0,
    defer(name) {
      deferred[name] = deferred[name] || {};
      deferred[name].promise = new Promise((resolve, reject) => {
        deferred[name].resolve = resolve;
        deferred[name].reject = reject;
      });
      return deferred[name];
    },
  };
  runtime.api = factory({
    state,
    notify() { runtime.notifyCount += 1; },
    bt(key) { return key; },
    addSystemItem() {},
    addAuthoritySyncNotice() {},
    addChatItem() {},
    timeStr() { return ''; },
    runSyncOnSession(sid, fn) {
      // 记录跨会话定向调用：sid !== active 时 fn 必须落在 sid 的 buffer 上，
      // 不能直接改当前显示。本 mock 简化执行 fn 但保留调用证据。
      if (sid !== state.activeSessionId) calls.push('runSyncOnSession:' + sid);
      fn();
    },
    getBuffer() { return null; },
    flushAssistantMessageToHistory() {},
    resetPendingAssistant() {},
    rerenderFromMessages() {},
    ensureSession: async () => (state.activeSessionId || 'chat-a'),
    sendMessage: async () => {},
    reconcileRemoteTurn: async () => true,
    isBusyFor() { return false; },
    markRemoteTurn() {},
    turnUsageDirty: {},
    invoke(name, args) {
      calls.push(name);
      if (deferred[name] && deferred[name].promise) return deferred[name].promise;
      return Promise.resolve({ mode: 'yolo', multi_agent: false });
    },
  });
  return runtime;
}

test('陈旧 get_mode_state 不得覆盖已完成 toggle：set 先落盘、旧读取后返回', async () => {
  const rt = loadInteractionRuntime();
  const get = rt.defer('get_mode_state');
  const set = rt.defer('set_multi_agent_mode');
  const syncP = rt.api.syncModeState();                    // t0: get 挂起（将返回旧值 false）
  const toggleP = rt.api.setMultiAgentMode(true);          // t1: 乐观翻转 true，set 在途
  set.resolve({ mode: 'yolo', multi_agent: true });        // t2: toggle 先落盘，finally 清空 in-flight
  await toggleP;
  get.resolve({ mode: 'yolo', multi_agent: false });       // t3: 旧 get 最后返回
  await syncP;
  assert.equal(rt.state.modeState.multiAgent, true,
    'toggle 已完成的权威值 true 不得被旧读取 false 覆盖（审计 P1 顺序）');
});

test('get_mode_state 返回时 toggle 仍在途：旧读取丢弃，乐观态保持', async () => {
  const rt = loadInteractionRuntime();
  const get = rt.defer('get_mode_state');
  const set = rt.defer('set_multi_agent_mode');
  const syncP = rt.api.syncModeState();
  const toggleP = rt.api.setMultiAgentMode(true);          // 乐观翻转 true，set 在途
  get.resolve({ mode: 'yolo', multi_agent: false });       // get 先返回（toggle 尚未落盘）
  await syncP;
  assert.equal(rt.state.modeState.multiAgent, true,
    '在途 toggle 期间的旧读取不得把乐观态覆盖回去');
  set.resolve({ mode: 'yolo', multi_agent: true });
  await toggleP;
});

test('get_mode_state 失败且期间发生 toggle：默认值不得覆盖权威改写', async () => {
  const rt = loadInteractionRuntime();
  const get = rt.defer('get_mode_state');
  const set = rt.defer('set_multi_agent_mode');
  const syncP = rt.api.syncModeState();
  const toggleP = rt.api.setMultiAgentMode(true);
  set.resolve({ mode: 'yolo', multi_agent: true });
  await toggleP;
  get.reject(new Error('backend down'));                   // get 失败（旧值本不可信）
  await syncP;
  assert.equal(rt.state.modeState.multiAgent, true,
    'get 失败也不得用 yolo/false 默认值把已完成的切换砸掉');
});

test('无权威改写时 syncModeState 正常写回后端值（epoch 不变）', async () => {
  const rt = loadInteractionRuntime();
  const get = rt.defer('get_mode_state');
  const syncP = rt.api.syncModeState();
  get.resolve({ mode: 'plan', multi_agent: true });
  await syncP;
  // 逐字段断言（vm 上下文的对象原型与主上下文不同，deepEqual 不可用）
  assert.equal(rt.state.modeState.mode, 'plan');
  assert.equal(rt.state.modeState.multiAgent, true,
    '没有发生过 toggle 的普通读取必须照常生效');
});
