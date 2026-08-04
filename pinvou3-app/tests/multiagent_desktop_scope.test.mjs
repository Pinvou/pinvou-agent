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

test('共享界面按 capability 订阅并阻止 Web 续写多智能体会话', () => {
  const main = read('src', 'app', 'main.jsx');
  const chat = read('src', 'features', 'chat', 'ChatView.jsx');
  const panel = read('src', 'features', 'multiagent', 'SubagentTranscriptPanel.jsx');
  const i18n = read('src', 'shared', 'i18n.js');
  const remoteCommands = read('src-tauri', 'src', 'app', 'commands', 'remote_control.rs');

  assert.match(main, /\.\.\.\(MULTI_AGENT_ENABLED \? \['multiAgent'\] : \[\]\)/);
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
    /ensure_web_chat_session_supported\(&session_id, store\.mode_state\(&session_id\)\.multi_agent\)\?/,
    'Web 续写必须校验多智能体开关（桌面专属）',
  );
  assert.equal((i18n.match(/uiMultiAgent:/g) || []).length, 3, '多智能体界面必须提供中英日文案');
});
