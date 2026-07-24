#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const source = path.join(root, 'src', 'features', 'codex', 'acp-state.js');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-codex-acp-'));
const modulePath = path.join(temp, 'acp-state.mjs');
copyFileSync(source, modulePath);

const event = (seq, type, data, turnId = 'turn-1') => ({
  version: 1,
  sessionId: 'session-1',
  turnId,
  seq,
  timestamp: `2026-07-23T00:00:0${Math.min(seq, 9)}Z`,
  event: { type, data },
});

try {
  const {
    appendAcpEvent,
    commandExecutionDetails,
    projectAcpTimeline,
    resolveAcpSessionControls,
    stripTerminalControlSequences,
  } = await import(`${pathToFileURL(modulePath).href}?t=${Date.now()}`);
  const events = [
    event(1, 'user_message', { content: [{ type: 'text', text: '修改 README' }] }),
    event(2, 'turn_started', { status: 'running' }),
    event(3, 'agent_thought_chunk', { update: { content: { type: 'text', text: '先检查文件。' } } }),
    event(4, 'tool_call', { update: {
      toolCallId: 'tool-1', title: '读取 README', kind: 'read', status: 'in_progress',
      rawInput: { path: 'README.md' },
    } }),
    event(5, 'tool_call_update', { update: {
      toolCallId: 'tool-1', status: 'completed', rawOutput: { text: '# PINVOU' },
    } }),
    event(6, 'permission_requested', { toolCallId: 'tool-2', request: {
      toolCall: { toolCallId: 'tool-2', title: '写入 README' },
      options: [{ optionId: 'allow-once', name: '允许一次', kind: 'allow_once' }],
    } }),
    event(7, 'permission_resolved', { toolCallId: 'tool-2', optionId: 'allow-once', outcome: 'selected' }),
    event(8, 'agent_message_chunk', { update: { content: { type: 'text', text: '已经完成' } } }),
    event(9, 'agent_message_chunk', { update: { content: { type: 'text', text: '修改。' } } }),
    event(11, 'usage', { update: { used: 120, size: 1000 } }),
    event(10, 'turn_completed', { status: 'Completed', error: null }),
  ];

  const projected = projectAcpTimeline([events[4], ...events, events[4]]);
  assert.equal(projected.turns.length, 1);
  const turn = projected.turns[0];
  assert.equal(turn.userText, '修改 README');
  assert.equal(turn.thoughtText, '先检查文件。');
  assert.equal(turn.assistantText, '已经完成修改。');
  assert.equal(turn.tools.length, 1, 'tool updates must be merged in place');
  assert.equal(turn.tools[0].status, 'completed');
  assert.deepEqual(turn.tools[0].rawInput, { path: 'README.md' });
  assert.deepEqual(turn.tools[0].rawOutput, { text: '# PINVOU' });
  assert.equal(turn.permissions[0].resolved, true);
  assert.equal(turn.status, 'Completed');
  assert.equal(turn.usage.used, 120);
  assert.deepEqual(turn.blocks.map(block => block.type), ['thought', 'tool', 'permission', 'message']);
  assert.equal(turn.blocks[1].tool.status, 'completed', 'tool block must update in its original position');
  assert.equal(projected.thread.turns, projected.turns, 'thread must own the projected turns');
  assert.deepEqual(
    turn.items.map(item => item.type),
    ['reasoning', 'tool', 'permission', 'agent_message'],
    'ACP blocks must normalize to Codex Turn Items',
  );
  assert.deepEqual(
    turn.presentation.map(item => item.type),
    ['reasoning', 'tool_group', 'permission', 'agent_message'],
    'operation items must be grouped only in the presentation layer',
  );
  assert.equal(turn.items[0].status, 'completed', 'reasoning must close when the next item starts');
  assert.equal(turn.items[2].status, 'completed', 'resolved permission must be terminal');

  const commandEvents = [
    event(20, 'user_message', { content: [{ type: 'text', text: '检查 PR' }] }, 'turn-command'),
    event(21, 'turn_started', { status: 'running' }, 'turn-command'),
    event(22, 'agent_thought_chunk', { update: { content: { type: 'text', text: '先检查状态。' } } }, 'turn-command'),
    event(23, 'tool_call', { update: {
      toolCallId: 'command-1',
      title: 'gh pr view 219',
      kind: 'execute',
      status: 'in_progress',
      rawInput: {
        command: 'gh pr view 219\ngit worktree list --porcelain',
        cwd: '/workspace/pinvou3',
      },
    } }, 'turn-command'),
    event(24, 'tool_call_update', { update: {
      toolCallId: 'command-1',
      status: 'completed',
      rawOutput: {
        formatted_output: '\u001b[31mUnknown JSON field: \"baseRefOid\"\u001b[0m\n'
          + '\u001b]8;;https://example.com\u0007worktree /workspace/pinvou3\u001b]8;;\u0007\n',
        exit_code: 0,
      },
    } }, 'turn-command'),
    event(25, 'turn_completed', { status: 'Completed', error: null }, 'turn-command'),
  ];
  const commandTurn = projectAcpTimeline(commandEvents).turns[0];
  assert.deepEqual(commandTurn.items.map(item => item.type), ['reasoning', 'command_execution']);
  const command = commandExecutionDetails(commandTurn.items[1].tool);
  assert.equal(command.cwd, '/workspace/pinvou3');
  assert.equal(command.exitCode, 0);
  assert.equal(command.commandCount, 2);
  assert.ok(command.output.includes('Unknown JSON field'));
  assert.equal(
    command.output,
    'Unknown JSON field: \"baseRefOid\"\nworktree /workspace/pinvou3\n',
    'command output must not render ANSI colors or OSC hyperlinks as garbage',
  );
  assert.equal(
    stripTerminalControlSequences('\u009b32m✓ passed\u009b0m'),
    '✓ passed',
    '8-bit CSI sequences must also be stripped',
  );

  assert.equal(appendAcpEvent(events, events[0]).length, events.length, 'duplicate seq must be ignored');
  assert.equal(appendAcpEvent(events.slice(0, 2), events[2]).length, 3);

  const controls = resolveAcpSessionControls({
    models: [{ id: 'legacy-model' }],
    modes: { currentModeId: 'agent-full-access', availableModes: [{ id: 'agent-full-access' }] },
    config_options: [
      { id: 'model', type: 'select', currentValue: 'gpt-5.6-sol', options: [] },
      { id: 'mode', type: 'select', currentValue: 'agent', options: [] },
      { id: 'collaboration_mode', type: 'select', currentValue: 'default', options: [] },
    ],
  });
  assert.deepEqual(controls.fallbackModels, [], 'config model must replace the legacy model selector');
  assert.equal(controls.fallbackModes, null, 'config mode must replace the legacy mode selector');
  assert.equal(controls.effectiveMode, 'agent', 'config mode must be the canonical observed mode');
  assert.deepEqual(
    controls.configOptions.map(option => option.id),
    ['model', 'mode', 'collaboration_mode'],
    'collaboration remains a separate control',
  );

  const legacyControls = resolveAcpSessionControls({
    models: [{ id: 'legacy-model' }],
    modes: { currentModeId: 'read-only', availableModes: [{ id: 'read-only' }] },
  });
  assert.equal(legacyControls.fallbackModels.length, 1);
  assert.equal(legacyControls.fallbackModes.currentModeId, 'read-only');
  assert.equal(legacyControls.effectiveMode, 'read-only');

  const chatView = readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
  assert.ok(!chatView.includes('ComposerAgentSelector'), 'DeepSeek composer must not expose backend switching');
  assert.ok(!chatView.includes('sessionAgentBackend'), 'DeepSeek ChatView must not branch on Codex state');

  const main = readFileSync(path.join(root, 'src', 'app', 'main.jsx'), 'utf8');
  assert.ok(main.includes("currentView === 'codex'"));
  assert.ok(main.includes('<CodexAcpView'));
  assert.ok(main.includes('codexAcpSupported &&'), 'Codex entry must stay Linux capability-gated');

  const chatCommands = readFileSync(path.join(root, 'src-tauri', 'src', 'app', 'commands', 'chat.rs'), 'utf8');
  const codexCommands = readFileSync(path.join(root, 'src-tauri', 'src', 'app', 'commands', 'codex.rs'), 'utf8');
  assert.ok(chatCommands.includes('Codex ACP 会话必须通过独立 Codex 页面发送'));
  assert.ok(codexCommands.includes('pub async fn codex_acp_prompt'));
  assert.ok(codexCommands.includes('pub async fn set_codex_acp_mode'));
  assert.ok(codexCommands.includes('list_codex_acp_sessions'));
  assert.ok(codexCommands.includes('workspace_path: Option<String>'), 'Codex creation must accept an explicit project directory');
  assert.ok(codexCommands.includes('validate_codex_project_workspace'), 'project workspace must be validated before session creation');

  const runtime = readFileSync(path.join(root, 'src-tauri', 'src', 'features', 'codex_acp', 'mod.rs'), 'utf8');
  assert.ok(runtime.includes('LoadSessionRequest::new(saved_id.clone(), workspace.clone())'));
  assert.ok(runtime.includes('NewSessionRequest::new(workspace)'));
  assert.ok(runtime.includes('Codex 会话绑定的项目目录已不可用'), 'missing projects must not silently fall back');
  assert.ok(runtime.includes('apply_saved_mode('), 'saved Full Access mode must be restored after new/load');
  assert.ok(!runtime.includes('runtime.prompt(content, mode_id)'), 'prompt must not overwrite acknowledged config with local UI mode');

  const codexView = readFileSync(path.join(root, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
  const baseStyles = readFileSync(path.join(root, 'src', 'styles', 'base.css'), 'utf8');
  assert.ok(codexView.includes("directory: true"), 'new Codex sessions must expose a native directory picker');
  assert.ok(codexView.includes('workspacePath'), 'selected project directory must reach the Tauri command');
  assert.ok(codexView.includes('临时会话'), 'temporary sessions must remain an explicit choice');
  assert.ok(codexView.includes('思考中'), 'running reasoning must expose a timer label');
  assert.ok(codexView.includes('执行步骤'), 'tool items must use a compact presentation group');
  assert.ok(!codexView.includes("useState(state === 'failed')"),
    'failed operation details must stay collapsed until the user opens them');
  assert.ok(!codexView.includes('useState(running || failed)'),
    'operation groups must not expand automatically for running or failed items');
  assert.ok(!codexView.includes("if (state === 'running') setOpen(true)"),
    'running operation details must not interrupt the conversation by auto-expanding');
  assert.ok(!codexView.includes('if (running) setOpen(true)'),
    'running operation groups must remain compact by default');
  assert.ok(!codexView.includes('<JsonBlock'), 'raw ACP JSON must not leak into normal command UI');
  assert.ok(codexView.includes("invoke('codex_acp_prompt', { sessionId: activeId, message })"));
  assert.ok(codexView.includes('className="codex-markdown'), 'Codex Markdown must use an isolated style scope');
  assert.ok(baseStyles.includes('.codex-markdown ul { list-style:disc outside; }'),
    'Codex unordered lists must retain bullets after Tailwind preflight');
  assert.ok(baseStyles.includes('.codex-markdown ol { list-style:decimal outside; }'),
    'Codex ordered lists must retain numbering after Tailwind preflight');

  console.log('codex_acp_timeline: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
