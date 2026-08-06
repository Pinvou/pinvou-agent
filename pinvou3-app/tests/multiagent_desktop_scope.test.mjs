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

test('原生 Code 暂时隐藏多智能体入口并由后端能力策略封死', () => {
  const codex = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const settings = read('src', 'features', 'settings', 'SettingsView.jsx');
  const tools = read('src', 'features', 'tools', 'tool-renderers.jsx');
  const policy = read('src-tauri', 'src', 'features', 'assistant', 'session_policy.rs');
  const bridge = read('src-tauri', 'src', 'features', 'assistant', 'platform', 'bridge.rs');
  const engine = read('src-tauri', 'src', 'features', 'assistant', 'engine.rs');
  const pool = read('src-tauri', 'src', 'features', 'assistant', 'engine_pool.rs');
  const builderStart = bridge.indexOf('pub fn build_engine_config_for_multi_agent');
  const builderEnd = bridge.indexOf('pub fn ', builderStart + 10);
  const multiAgentBuilder = bridge.slice(builderStart, builderEnd);

  assert.match(codex, /const NATIVE_CODE_MULTI_AGENT_AVAILABLE = false/);
  assert.match(
    codex,
    /<ComposerModelSelector[\s\S]*?multiAgentAvailable=\{NATIVE_CODE_MULTI_AGENT_AVAILABLE\}/,
  );
  assert.doesNotMatch(codex, /invoke\('set_multi_agent_mode'/);
  assert.doesNotMatch(codex, /onToggleMultiAgent=\{switchNativeMultiAgent\}/);
  assert.match(
    policy,
    /pub fn multi_agent_available\(&self\)[\s\S]{0,120}matches!\(self\.mode, SessionMode::Plain\)/,
    '仅 Work/Plain 模式开放多智能体',
  );
  assert.match(bridge, /cfg\.subagents_enabled\s*&=\s*self\.session_policy\(session_id\)\.multi_agent_available\(\)/);
  assert.ok(builderStart >= 0 && builderEnd > builderStart, '必须保留多智能体专用配置入口');
  assert.match(
    multiAgentBuilder,
    /if !self\.session_policy\(session_id\)\.multi_agent_available\(\)/,
    '多智能体专用配置入口也必须先判能力，禁止向 Code 项目投影专家文件',
  );
  assert.match(engine, /session_policy\(session_id\)[\s\S]{0,100}\.multi_agent_available\(\)/);
  assert.match(pool, /if enabled && !self\.multi_agent_available\(session_id\)/);
  assert.match(codex, /<ToolCard[\s\S]*?sessionId=\{activeId\}/);
  assert.match(codex, /<SubagentTranscriptPanel[\s\S]*?sessionId=\{activeSession\.id\}/);
  assert.match(codex, /!subagentPanel && \(activeSession \|\| draftWorkspacePath\)/);
  assert.match(settings, /multiAgentEnabled:\s*multiAgentEnabledProp/);
  assert.match(settings, /if \(onToggleMultiAgent\) await onToggleMultiAgent\(!multiAgentOn\)/);
  assert.match(tools, /detail: \{ agentId: agentId \|\| null, sessionId: sessionId \|\| null \}/);
  assert.match(tools, /listSubagentTranscripts\(sid\)/);
});
