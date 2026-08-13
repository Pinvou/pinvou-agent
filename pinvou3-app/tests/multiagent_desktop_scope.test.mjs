import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

test('多智能体第一阶段只在桌面宿主开放', () => {
  const platform = read('src', 'shared', 'platform.js');
  const bootstrap = read('src', 'platform', 'web', 'bootstrap.js');
  const webBridge = read('src', 'platform', 'web', 'bridge.js');
  const policy = JSON.parse(read('src', 'platform', 'web', 'access-policy.json'));

  assert.match(platform, /\bmultiAgent:\s*true\b/, '桌面默认能力必须显式开放');
  assert.doesNotMatch(bootstrap, /\bmultiAgent\s*:\s*true\b/, 'Web capability 不得提前开放');

  for (const command of [
    'set_multi_agent_mode',
    'list_subagent_transcripts',
    'read_subagent_transcript',
  ]) {
    assert.equal(policy.allowed_commands.includes(command), false, `${command} 必须保持桌面专属`);
    assert.equal(webBridge.includes(command), false, `Web bridge 不得代理 ${command}`);
  }
});

test('共享界面不订阅废弃运行态，并阻止 Web 续写多智能体会话', () => {
  const main = read('src', 'app', 'main.jsx');
  const detached = read('src', 'app', 'DetachedShell.jsx');
  const tauriBridge = read('src', 'platform', 'tauri', 'bridge.js');
  const chat = read('src', 'features', 'chat', 'ChatView.jsx');
  const panel = read('src', 'features', 'multiagent', 'SubagentTranscriptPanel.jsx');
  const i18n = read('src', 'shared', 'i18n.js');
  const remoteCommands = read('src-tauri', 'src', 'app', 'commands', 'remote_control.rs');

  assert.doesNotMatch(
    main,
    /APP_BRIDGE_STATE_DOMAINS\s*=\s*\[[\s\S]{0,400}['"]multiAgent['"]/,
    '多智能体投影由命令与 DOM 事件提供，不得订阅已退役的空运行态',
  );
  assert.doesNotMatch(detached, /useBridgeState\(\[[\s\S]{0,300}['"]multiAgent['"]/);
  assert.doesNotMatch(tauriBridge, /activeRunId/, '旧 Workflow 运行台账状态不得残留');
  assert.match(
    chat,
    /const isMultiAgentReadOnly = !MULTI_AGENT_ENABLED\s*&& !!\(bs && bs\.modeState && bs\.modeState\.multiAgent\)/,
    'Web 只读判定看会话级开关（modeState.multiAgent 双端同步）',
  );
  assert.match(chat, /data-testid="multiagent-desktop-only"/);
  const settings = read('src', 'features', 'settings', 'SettingsView.jsx');
  assert.match(settings, /const canMultiAgent = can\('multiAgent'\)/, '开关行必须按 capability 门禁');
  assert.match(settings, /data-testid="multiagent-toggle"/, '模型列表下方必须有会话级开关');
  assert.match(panel, /listSubagentTranscripts\(sessionId\)/);
  assert.match(
    remoteCommands,
    /ensure_web_chat_session_supported\(store\.mode_state\(&session_id\)\.multi_agent\)\?/,
    'Web 续写必须校验多智能体开关（桌面专属）',
  );
  assert.equal((i18n.match(/uiMultiAgent:/g) || []).length, 3, '多智能体界面必须提供中英日文案');
});

test('Pinvou 多智能体接入 Work 与原生 Code，并隔离 Code 状态根', () => {
  const codex = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const settings = read('src', 'features', 'settings', 'SettingsView.jsx');
  const tools = read('src', 'features', 'tools', 'tool-renderers.jsx');
  const commands = read('src-tauri', 'src', 'app', 'commands', 'multiagent.rs');
  const interaction = read('src-tauri', 'src', 'app', 'commands', 'interaction.rs');
  const appRoot = read('src-tauri', 'src', 'lib.rs');
  const policy = read('src-tauri', 'src', 'features', 'assistant', 'session_policy.rs');
  const bridge = read('src-tauri', 'src', 'features', 'assistant', 'platform', 'bridge.rs');
  const engine = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
  const pool = read('src-tauri', 'src', 'features', 'assistant', 'engine_pool.rs');
  const builderStart = bridge.indexOf('pub(crate) fn build_engine_config_for_multi_agent');
  const builderEnd = bridge.indexOf('pub fn ', builderStart + 10);
  const multiAgentBuilder = bridge.slice(builderStart, builderEnd);

  // 逐 token 断言，避免"或"正则把删除一半的实现伪装成通过（P2 审计修复）。
  assert.match(codex, /set_multi_agent_mode/, 'Code 车道必须能经会话命令切换多智能体开关');
  assert.match(codex, /SubagentTranscriptPanel/, 'Code 车道必须挂载只读执行记录面板');
  assert.match(codex, /ToolCard/, 'Code 车道必须能渲染专家/委派工具卡');
  // 产品开关不得改写普通 Engine 的 subagents_enabled（#162 复审 P1：整体关闭会
  // 误伤原生 Code 会话的底座 agent/agents/*/workflow 能力）。
  assert.doesNotMatch(
    bridge,
    /subagents_enabled\s*&=\s*self\.session_policy/,
    '不得把产品入口收缩当成禁用底座委派能力的开关',
  );
  assert.match(
    policy,
    /pub fn supports_multi_agent_mode\(&self\)[\s\S]{0,160}SessionMode::Plain \| SessionMode::Code/,
    'Work/Plain 与原生 Code 都开放 Pinvou 多智能体产品能力',
  );
  assert.match(
    bridge,
    /pub fn multi_agent_mode_available[\s\S]{0,500}!external_acp && self\.session_policy\(session_id\)\.supports_multi_agent_mode\(\)/,
    '最终能力必须同时检查产品模式与原生/外部 ACP 运行时轴',
  );
  assert.match(
    appRoot,
    /set_external_acp_session_predicate[\s\S]{0,300}acp_pool\.is_acp\(session_id\)/,
    '外部 ACP 判定必须由进程内同一份 AcpPool 注入，不能靠前端隐藏入口',
  );
  assert.match(
    bridge,
    /cfg\.workspace = roots\.execution;[\s\S]{0,120}cfg\.subagent_state_root = Some\(roots\.ledger\);/,
    '引擎执行根与 delegated-agent 状态根必须显式分离',
  );
  assert.ok(builderStart >= 0 && builderEnd > builderStart, '必须保留多智能体专用配置入口');
  assert.match(
    multiAgentBuilder,
    /FleetRoster::load\([\s\S]{0,120}snapshot\.fleet_config\(\)[\s\S]{0,80}&cfg\.workspace/,
    '初始名册必须合并全局专家配置与真实 execution workspace',
  );
  assert.doesNotMatch(
    multiAgentBuilder,
    /enroll_expert_roles|roots\.ledger/,
    '多智能体配置不得再向会话 ledger 写入或从中加载专家 TOML',
  );
  assert.match(
    bridge,
    /build_multi_agent_dt_config[\s\S]{0,500}config\.fleet = Some\(snapshot\.fleet_config\(\)\.clone\(\)\)/,
    '全局专家池必须通过 CodeWhale 原生 fleet profiles 进入每轮配置',
  );
  assert.match(engine, /bridge\.multi_agent_mode_available\(session_id\)/);
  assert.match(pool, /if enabled && !self\.multi_agent_mode_available\(session_id\)/);
  assert.match(
    commands,
    /fn subagent_state_root\([\s\S]{0,500}!pool\.multi_agent_mode_available\(session_id\)[\s\S]{0,220}pool\.session_state_root\(session_id\)/,
    'Code transcript 必须读取会话私有状态根，不得读取项目目录',
  );
  assert.match(codex, /multiAgentEnabled=\{nativeMultiAgentEnabled\}/);
  assert.match(codex, /multiAgentAvailable=\{nativeMultiAgentAvailable\}/);
  assert.match(codex, /onToggleMultiAgent=\{switchNativeMultiAgent\}/);
  assert.match(
    interaction,
    /multi_agent_available:\s*pool\.multi_agent_mode_available\(&session_id\)/,
    'Code 页开关可用性必须来自后端会话策略，不能由前端写死',
  );
  assert.match(
    codex,
    /multiAgentAvailable:\s*Boolean\(modeState && modeState\.multi_agent_available\)/,
    'Code 页必须消费 get_mode_state 返回的策略能力',
  );
  assert.match(
    codex,
    /nativeMultiAgentEnabled\s*=\s*nativeMultiAgentAvailable\s*&&\s*nativeMultiAgentSelected/,
    '旧状态不得绕过会话策略重新启用多智能体展示',
  );
  assert.match(
    codex,
    /renderToolItem=\{isNativeAgent\s*\?/,
    '原生 Code 必须始终把真实 agent 调用交给行内子智能体卡渲染',
  );
  assert.doesNotMatch(
    codex,
    /renderToolItem=\{isNativeAgent && nativeMultiAgentEnabled/,
    '产品专家模式关闭后也不能隐藏底座已经实际执行的裸 agent 调用',
  );
  assert.match(
    codex,
    /subagentPanel && activeSession && isNativeAgent && \(/,
    '原生 Code 的真实子智能体 transcript 必须始终可打开',
  );
  assert.doesNotMatch(
    codex,
    /subagentPanel && activeSession && isNativeAgent && nativeMultiAgentEnabled/,
    '产品专家模式关闭后不得隐藏既有 transcript',
  );
  assert.match(settings, /multiAgentEnabled:\s*multiAgentEnabledProp/);
  assert.match(settings, /if \(onToggleMultiAgent\) await onToggleMultiAgent\(!multiAgentOn\)/);
  assert.match(tools, /if \(typeof window === 'undefined' \|\| !agentId\) return;/);
  assert.match(tools, /detail: \{ agentId, sessionId: sessionId \|\| null \}/);
  assert.match(tools, /listSubagentTranscripts\(sid\)/);
});
