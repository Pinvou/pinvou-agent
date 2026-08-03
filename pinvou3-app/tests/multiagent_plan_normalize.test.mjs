/** 多智能体（会话内主动委派，ADR-0006）薄层契约：桥、专家卡、只读面板与取消级联。 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { startTranscriptPolling } from '../src/features/multiagent/runState.mjs';
import {
  extractSubagentId,
  fileChangeStat,
  projectSubagentTranscript,
  resolveSubagentIdentity,
  splitSubagentTitle,
  subagentOrdinalLabel,
  subagentRoleOrdinals,
} from '../src/features/multiagent/subagent-conversation.mjs';
import {
  captureConversationScrollPosition,
  isExpertDelegationCall,
  presentConversationItems,
  restoreConversationScrollPosition,
} from '../src/features/conversation/conversation-model.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (...parts) => fs.readFileSync(path.join(here, '..', ...parts), 'utf8');
const source = read('src', 'platform', 'tauri', 'bridge', 'multiagent.js');
const panelSource = read('src', 'features', 'multiagent', 'SubagentTranscriptPanel.jsx');
const toolRenderersSource = read('src', 'features', 'tools', 'tool-renderers.jsx');
const chatViewSource = read('src', 'features', 'chat', 'ChatView.jsx');
const commandSource = read('src-tauri', 'src', 'app', 'commands', 'multiagent.rs');
const chatCommandSource = read('src-tauri', 'src', 'app', 'commands', 'chat.rs');
const interactionCommandSource = read('src-tauri', 'src', 'app', 'commands', 'interaction.rs');
const interactionBridgeSource = read('src', 'platform', 'tauri', 'bridge', 'interaction.js');
const settingsSource = read('src', 'features', 'settings', 'SettingsView.jsx');
const poolSource = read('src-tauri', 'src', 'features', 'assistant', 'engine_pool.rs');

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

test('子智能体事件转成 DOM 事件供专家卡自订阅，任意会话都转发', () => {
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
  assert.match(commandSource, /pub\(crate\) fn delegation_reminder\(\)/, '委派提醒抽成函数供发送链注入');
  assert.match(commandSource, /roster::available_role_lines\(\)/, '名册必须随提醒带上');
  assert.match(
    chatCommandSource,
    /if mode_state\.multi_agent \{[\s\S]*?delegation_reminder\(\)/,
    '开关开启时 chat 发送链每轮拼提醒',
  );
  assert.match(
    interactionCommandSource,
    /pub async fn set_multi_agent_mode\(/,
    '开关命令在 interaction 域',
  );
  assert.match(
    interactionCommandSource,
    /ensure_git_repository\(&workspace\)/,
    '开启时会话工作区必须 git 化（并行子智能体 spawn 的前置）',
  );
  assert.match(
    interactionCommandSource,
    /validate_session_id\(&session_id\)[\s\S]{0,900}create_dir_all/,
    '任何副作用之前必须先做 id 形状校验（paths 只是 join，防 ../ 穿越）',
  );
  assert.match(
    interactionCommandSource,
    /\.load\(&session_id\)[\s\S]{0,600}create_dir_all/,
    '会话必须确实存在才允许做副作用（防孤儿目录）',
  );
  assert.match(
    interactionCommandSource,
    /refresh_multi_agent_roster\(&session_id\)/,
    '开启时把名册整册即时推给在跑引擎',
  );
  assert.match(
    poolSource,
    /Op::SetFleetRoster \{ roster \}/,
    'live 名册刷新走底座 SetFleetRoster，而不是改写工具列表',
  );
  assert.doesNotMatch(
    poolSource,
    /workflow_host_disallowed_tools/,
    '广播/回收不再按会话形态改写禁用列表——工具面与主线持平',
  );
  assert.match(
    poolSource,
    /late sweep of deleted run/,
    '删除运行后要有延迟清扫，兜住底座异步写 ledger 复活目录的竞态',
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
    '不再播种内置默认角色——名册只来自专家池（用户决策：委派本质是写提示词）',
  );
  assert.match(
    interactionCommandSource,
    /enroll_expert_roles\(&workspace\)[\s\S]{0,80}\.map_err/,
    '专家名册写盘失败必须让开启失败，不得静默成功',
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
    /entry\.multi_agent = previous/,
    '落盘失败必须回滚内存，界面状态与磁盘一致',
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
    /fn save_multi_agent_flags_locked/,
    '保存本体与事务共用同一临界区（互斥不可重入，靠 _locked 拆分）',
  );
  assert.match(
    sessionsSource,
    /ghost cleanup/,
    '启动加载必须对账剔除幽灵 id 并重写清单',
  );
  assert.match(
    interactionCommandSource,
    /refresh_multi_agent_roster\(&session_id\)\.await[\s\S]{0,300}set_multi_agent\(&session_id, false\)/,
    '名册推送失败必须回滚开关并报错，不得谎报开启成功',
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
  const platformSource = read('src-tauri', 'src', 'features', 'multiagent', 'platform', 'mod.rs');
  assert.match(
    platformSource,
    /fn snapshot_workspace\(/,
    'worktree 子智能体只检出 HEAD，必须有工作区快照通道',
  );
  assert.match(
    chatCommandSource,
    /spawn_blocking\(move \|\| \{\s*crate::features::multiagent::platform::snapshot_workspace/,
    '多智能体轮次发送前快照工作区（阻塞 git 走 spawn_blocking），否则本轮输入文件在 worktree 里不可见',
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
  assert.match(
    settingsSource,
    /legacyMultiAgentSession = !!\(bs && bs\.activeSessionId && String\(bs\.activeSessionId\)\.indexOf\('wf-'\) === 0\)/,
    '存量 wf- 会话恒为开启，前端必须隐藏开关行（且不得踩 activeSessionId 的 TDZ）',
  );
  assert.match(
    interactionCommandSource,
    /is_workflow_session_id\(&session_id\)[\s\S]{0,220}恒为开启/,
    '后端必须拒绝对存量 wf- 会话切换开关，防止名册落进错误目录',
  );
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
    /disabled=\{multiAgentBusy\}/,
    '切换期间必须禁用开关按钮',
  );
  assert.match(
    settingsSource,
    /multiAgentOn\s*\?\s*'pinvou-ultra-row'/,
    '开启态整行套 ultracode 式紫色波动面板',
  );
  assert.match(
    settingsSource,
    /multiAgentOn \? 'text-\[#6d28d9\] dark:text-\[#c4b5fd\]'/,
    '开启多智能体后，模型选择器触发按钮字体转面板同款紫作在场提示（真机建议：不开弹层也能看出模式）',
  );
  assert.match(
    settingsSource,
    /async function toggleMultiAgent\(\) \{[\s\S]{0,600}setMultiAgentRevealing\(!multiAgentOn\)/,
    '动效只在用户点击开启一刻播放；会话切换/重启同步出现的开启态不得重播（真机点名弹层重开不重播）',
  );
  assert.match(
    settingsSource,
    /event\.animationName === 'pinvou-ultra-reveal'/,
    '揭幕结束即摘 reveal 类（光晕无限循环不能作摘类信号）；同名光晕动画跨类续跑不重启',
  );
  const baseCss = read('src', 'styles', 'base.css');
  assert.match(baseCss, /\.pinvou-ultra-row \{/, '渐变面板样式定义在全局样式');
  {
    const rowRuleStart = baseCss.indexOf('.pinvou-ultra-row {');
    const rowRule = baseCss.slice(rowRuleStart, baseCss.indexOf('}', rowRuleStart));
    assert.doesNotMatch(
      rowRule,
      /background/,
      '行元素自身不得落底色：全部紫色画在被 clip 的 ::before 上，否则开启瞬间整行先闪成纯紫、扩散不可见',
    );
  }
  {
    const beforeStart = baseCss.indexOf('.pinvou-ultra-row::before {');
    const beforeRule = baseCss.slice(beforeStart, baseCss.indexOf('}', beforeStart));
    assert.match(beforeRule, /background-color: #2e1065/, '面板底色与波纹同在 ::before 一层，被揭幕一起裁剪');
    assert.match(
      beforeRule,
      /animation:\s*pinvou-ultra-aurora1[\s\S]{0,220}infinite/,
      '开启态光晕必须持续漂移不停（真机点名"动画不能停"）；仅揭幕是点击一次性',
    );
  }
  assert.match(
    baseCss,
    /@property --pinvou-aurora1 \{[\s\S]{0,120}syntax: '<percentage>'/,
    '光晕圆心必须注册成可插值的自定义属性，否则 var 动画退化为分段跳变',
  );
  assert.match(
    baseCss,
    /@keyframes pinvou-ultra-aurora2[\s\S]{0,120}--pinvou-aurora2/,
    '原创「专家光晕」：三团柔光各自独立周期往复漂移（互质错拍），不再仿制 Claude 面板',
  );
  assert.match(
    baseCss,
    /at var\(--pinvou-aurora1\)/,
    '光晕圆心由注册属性驱动横向漂移',
  );
  assert.match(
    baseCss,
    /at var\(--pinvou-aurora3\)/,
    '至少三团光晕（呼应"多"智能体），单团会读成廉价高光',
  );
  assert.match(
    baseCss,
    /pinvou-ultra-reveal (0\.[89]\d?|1(\.\d+)?)s/,
    '揭幕要慢到能看清从拨杆处从零铺满（真机点名 0.5s 太快）',
  );
  assert.doesNotMatch(
    baseCss,
    /pinvou-ultra-aurora\d [\d.]+s [^,;]*\b\d+\s*[,;]/,
    '光晕不得播有限次数就定格（真机点名"动画不能停"）',
  );
  assert.doesNotMatch(
    baseCss,
    /data:image\/png|image-rendering/,
    '像素噪点贴图在 40px 高的小行上糊成脏斑（真机点名更丑），面板保持纯渐变',
  );
  assert.match(
    baseCss,
    /@keyframes pinvou-ultra-reveal[\s\S]{0,160}clip-path: circle\(0% at 91%/,
    '开启动效必须从拨杆位置（行右侧 91%）向外圆形扩散',
  );
  assert.match(
    interactionBridgeSource,
    /previousMultiAgent[\s\S]{0,600}invoke\("set_multi_agent_mode"/,
    '点击必须乐观翻转（后端 git 化数百毫秒，等返回才翻拨杆像点了没反应），失败回滚',
  );
  const platformSource2 = read('src-tauri', 'src', 'features', 'multiagent', 'platform', 'mod.rs');
  assert.match(
    platformSource2,
    /join\("\.git"\)\.exists\(\)/,
    '已初始化的工作区重复开关不得再 spawn git init（Windows 进程启动贵）',
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
    platformSource2,
    /fn ensure_state_excluded/,
    'worker ledger 与子智能体完整对话（.codewhale/state）不得进入工作区 git 快照（复核 P1）',
  );
  assert.match(
    platformSource2,
    /--ignore-unmatch/,
    '历史遗留已被跟踪的运行状态要在下轮快照就地停跟踪',
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
    /\(usesCustomTitle \? title\.rest : entry\.objective\) \|\| entry\.agent_id/,
    '清单行展示任务目标（起了「」名时展示去名后的正文）；无 ledger 的遗留行回退 agent_id',
  );
  const fileIngestSource = read('src-tauri', 'src', 'features', 'files', 'file_ingest.rs');
  assert.match(
    fileIngestSource,
    /item\(\s*"git",/,
    'git 是多智能体的系统依赖，必须出现在依赖体检里（Windows 最常缺）',
  );
  assert.match(
    platformSource2,
    /依赖体检/,
    '缺 git 的报错要给人话与出路（指到 设置 → 依赖体检），不甩原始 NotFound',
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
    chatCommandSource,
    /快照失败\*?\*?中止/,
    '快照失败必须中止本轮（复核 P2：静默继续会让 worktree 子智能体基于旧文件出过期结果）',
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
    /subagentRoleOrdinals\(list\)/,
    '轮询广播必须携带同角色序号，行内卡的 ①② 与面板同源一致',
  );
  assert.match(
    toolRenderersSource,
    /splitSubagentTitle\(/,
    '行内卡优先显示模型起的「」名（底座 role 字段只收 ASCII，中文名走文本约定）',
  );
  assert.match(
    panelSource,
    /splitSubagentTitle\(/,
    '面板同样优先显示模型起的「」名，与行内卡一致',
  );
  assert.match(
    toolRenderersSource,
    /\.\.\.\(prev \|\| \{\}\), \.\.\.detail/,
    '实时事件不带 seq/blocked 等补字段，卡片状态必须字段合并，不得整包覆盖',
  );
  assert.doesNotMatch(baseCss, /filter: brightness/, 'hover 滤镜整层重绘会卡按钮');
  const ultraSection = baseCss.slice(baseCss.indexOf('.pinvou-ultra-row {'));
  assert.doesNotMatch(
    ultraSection,
    /will-change|repeating-conic-gradient/,
    'WebView2 下伪元素提层慢、conic 重绘贵，波动面板不得使用（实测淘汰）',
  );
  assert.match(
    baseCss,
    /pinvou-ultra-sheen [\d.]+s linear infinite/,
    '面板必须有匀速穿行的流光带——单靠慢速光晕会读成静止（真机点名"没有流动性"）',
  );
  assert.match(
    baseCss,
    /@keyframes pinvou-ultra-sheen[\s\S]{0,160}background-position/,
    '流光带用可平铺渐变的 background-position 平移（廉价重绘，实测顺滑）',
  );
  assert.match(
    baseCss,
    /@keyframes pinvou-ultra-sheen[\s\S]{0,400}animation-timing-function/,
    '流光带过场要有缓急并留停顿拍（真机点名"僵硬"：匀速等距循环像传送带）',
  );
  assert.match(
    baseCss,
    /linear-gradient\(112deg,[\s\S]{0,240}rgba\(245, 243, 255, 0\.4/,
    '流光带峰值要够亮、带宽要窄于视窗，否则读成整行缓明缓暗（真机点名"看不出来"）',
  );
  assert.match(
    baseCss,
    /at var\(--pinvou-aurora1\) var\(--pinvou-aurora1y\)/,
    '光晕必须 x/y 双轴互质周期二维游移（真机点名"僵硬"：仅横向来回像滑块）',
  );
  assert.match(
    baseCss,
    /@keyframes pinvou-ultra-splash[\s\S]{0,260}scale\(26\)/,
    '揭幕增长边缘必须带水花亮环（真机点名"尾部要有水花感"），随铺满消散',
  );
  assert.match(
    baseCss,
    /\.pinvou-ultra-row-reveal::after \{[\s\S]{0,120}pinvou-ultra-splash 1\.2s cubic-bezier\(0\.3, 0, 0\.25, 1\)/,
    '水花与揭幕必须同时长同缓动（都从 0 按同一 f\(t\) 增长才贴边）；改任一侧必须同步另一侧',
  );
  assert.match(
    baseCss,
    /pinvou-ultra-reveal 1\.2s cubic-bezier\(0\.3, 0, 0\.25, 1\)/,
    '揭幕 1.2s 慢起步（真机连点两次"太快"：0.5s→0.9s→1.2s）',
  );
  assert.match(baseCss, /prefers-reduced-motion/, '波动必须尊重减少动态偏好');
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

test('transcripts 命令面向任意会话：wf- 走遗留运行目录，普通会话走自身工作区', () => {
  assert.match(commandSource, /fn subagent_workspace\(session_id: &str\)/);
  assert.match(commandSource, /agent_run_workspace_dir\(session_id\)/, '遗留 wf- 运行仍可读');
  assert.match(commandSource, /session_workspace_dir\(session_id\)/, '普通会话读自己的工作区');
  assert.doesNotMatch(
    commandSource,
    /不是工作流运行/,
    'transcripts 不再被 wf- 前缀门禁挡住',
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

// ── 行内专家卡（消息流内的委派可视化） ───────────────────────────────────────

test('agent 工具调用渲染成行内专家卡，点击打开只读面板', () => {
  assert.match(
    toolRenderersSource,
    /if \(EXPERT_CARD_ENABLED && item\.name === 'agent'\) \{\s*return <ExpertAgentCard/,
    'agent 调用不走通用工具卡，但必须按 capability 门禁（Web 无 multiAgent bridge）',
  );
  assert.match(
    toolRenderersSource,
    /const EXPERT_CARD_ENABLED = can\('multiAgent'\)/,
    '门禁是模块级常量，一次构建内恒定（Hook 数量稳定）',
  );
  assert.match(
    toolRenderersSource,
    /LEDGER_STATUS_TOKENS/,
    'ledger 英文状态 token 必须映射 i18n，不得原样上屏',
  );
  assert.match(
    toolRenderersSource,
    /copy\.coordinationRow\(action\)/,
    '协调行文案走 i18n',
  );
  assert.match(
    toolRenderersSource,
    /args\.profile \|\| args\.role/,
    '承担者以底座正式契约字段 profile 为准（role 是内置类型别名）',
  );
  assert.match(
    toolRenderersSource,
    /data-testid="agent-coordination-row"/,
    'status/wait/cancel 渲染成安静单行，不冒充新委派',
  );
  assert.match(
    toolRenderersSource,
    /watchExpertCard\(agentId\)/,
    '卡片必须接权威落盘轮询（实时事件会丢：拥塞/重启/停止级联）',
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
  assert.match(toolRenderersSource, /data-testid="expert-agent-card"/);
  assert.match(
    toolRenderersSource,
    /pinvou:open-subagent/,
    '点击整卡经 DOM 事件通知 ChatView 打开面板',
  );
  assert.match(
    toolRenderersSource,
    /pinvou:subagent-update/,
    '实时状态按 agent_id 自订阅桥转发的 DOM 事件',
  );
  assert.match(
    toolRenderersSource,
    /resolveSubagentIdentity\(/,
    '头像与名字经身份解析复用专家池',
  );
  assert.match(toolRenderersSource, /extractSubagentId\(item\.output\)/);
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

test('ChatView 监听打开事件并挂载只读面板（任意会话可用），旧运行条带已退役', () => {
  assert.match(chatViewSource, /pinvou:open-subagent/);
  assert.match(chatViewSource, /<SubagentTranscriptPanel/);
  assert.match(chatViewSource, /captureConversationScrollPosition\(/);
  assert.match(chatViewSource, /restoreConversationScrollPosition\(/);
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
  assert.match(panelSource, /startTranscriptPolling\(\{[\s\S]*active: !\(agent && agent\.done\)/);
  assert.match(panelSource, /listSubagentTranscripts\(sessionId\)/, '列表来自底座落盘投影');
  assert.match(panelSource, /setSelectedAgentId\(null\)/, '详情可返回列表');
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
});
