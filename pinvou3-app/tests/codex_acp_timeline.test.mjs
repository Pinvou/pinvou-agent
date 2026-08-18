#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const source = path.join(root, 'src', 'features', 'codex', 'acp-state.js');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-codex-acp-'));
const moduleDir = path.join(temp, 'codex');
const conversationDir = path.join(temp, 'conversation');
mkdirSync(moduleDir, { recursive: true });
mkdirSync(conversationDir, { recursive: true });
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
const modulePath = path.join(moduleDir, 'acp-state.js');
copyFileSync(source, modulePath);
copyFileSync(
  path.join(root, 'src', 'features', 'conversation', 'conversation-model.js'),
  path.join(conversationDir, 'conversation-model.js'),
);

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
    buildElicitationContent,
    commandExecutionDetails,
    mergeAcpTimelineSnapshot,
    projectAcpTimeline,
    resolveAcpSessionControls,
    stripTerminalControlSequences,
    updateAcpAttachmentDraft,
  } = await import(`${pathToFileURL(modulePath).href}?t=${Date.now()}`);
  const {
    collectToolWorkspaceResources,
    toolWorkspaceResources,
    workspaceMarkdownResource,
  } = await import(`${pathToFileURL(path.join(conversationDir, 'conversation-model.js')).href}?t=${Date.now()}`);
  const movedAttachment = { id: 'attachment-1', status: 'uploading' };
  const movedDrafts = updateAcpAttachmentDraft(
    { draft: [movedAttachment] },
    movedAttachment.id,
    attachment => ({ ...attachment, status: 'ready' }),
  );
  assert.equal(movedDrafts.draft[0].status, 'ready');
  assert.equal(updateAcpAttachmentDraft(movedDrafts, 'missing', () => null), movedDrafts);
  const events = [
    event(1, 'user_message', {
      content: [{ type: 'text', text: '修改 README' }],
      attachments: [{ name: 'README.md', kind: 'text', size: 1024 }],
    }),
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
    event(8, 'elicitation_requested', { elicitationId: 'input-1', request: {
      mode: 'form',
      message: '请选择实现方式',
      requestedSchema: {
        type: 'object',
        properties: {
          stack: { type: 'string', title: '技术栈', oneOf: [{ const: '原生', title: '原生' }] },
        },
        required: ['stack'],
      },
    } }),
    event(9, 'elicitation_resolved', { elicitationId: 'input-1', action: 'accept' }),
    event(10, 'agent_message_chunk', { update: { content: { type: 'text', text: '已经完成' } } }),
    event(11, 'agent_message_chunk', { update: { content: { type: 'text', text: '修改。' } } }),
    event(13, 'usage', { update: { used: 120, size: 1000 } }),
    event(12, 'turn_completed', { status: 'Completed', error: null }),
  ];

  const projected = projectAcpTimeline([events[4], ...events, events[4]]);
  assert.equal(projected.turns.length, 1);
  const turn = projected.turns[0];
  assert.equal(turn.userText, '修改 README');
  assert.deepEqual(turn.userAttachments, [{ name: 'README.md', kind: 'text', size: 1024 }]);
  assert.equal(turn.thoughtText, '先检查文件。');
  assert.equal(turn.assistantText, '已经完成修改。');
  assert.equal(turn.tools.length, 1, 'tool updates must be merged in place');
  assert.equal(turn.tools[0].status, 'completed');
  assert.deepEqual(turn.tools[0].rawInput, { path: 'README.md' });
  assert.deepEqual(turn.tools[0].rawOutput, { text: '# PINVOU' });
  assert.equal(turn.permissions[0].resolved, true);
  assert.equal(turn.elicitations[0].resolved, true);
  assert.equal(turn.elicitations[0].action, 'accept');
  assert.equal(turn.waitingInput, false);
  assert.equal(turn.status, 'Completed');
  assert.equal(turn.usage.used, 120);
  assert.deepEqual(
    turn.blocks.map(block => block.type),
    ['thought', 'tool', 'permission', 'elicitation', 'message'],
  );
  assert.equal(turn.blocks[1].tool.status, 'completed', 'tool block must update in its original position');
  assert.equal(projected.thread.turns, projected.turns, 'thread must own the projected turns');
  assert.deepEqual(
    turn.items.map(item => item.type),
    ['reasoning', 'tool', 'permission', 'elicitation', 'agent_message'],
    'ACP blocks must normalize to Codex Turn Items',
  );
  assert.deepEqual(
    turn.presentation.map(item => item.type),
    ['reasoning', 'tool_group', 'permission', 'elicitation', 'agent_message'],
    'operation items must be grouped only in the presentation layer',
  );
  assert.equal(turn.operationCount, 1);
  assert.equal(turn.failedOperationCount, 0);
  assert.equal(turn.items[0].status, 'completed', 'reasoning must close when the next item starts');
  assert.equal(turn.items[2].status, 'completed', 'resolved permission must be terminal');
  assert.equal(turn.items[3].status, 'completed', 'resolved elicitation must be terminal');

  const pendingInputTurn = projectAcpTimeline([
    event(14, 'turn_started', { status: 'running' }, 'turn-input'),
    event(15, 'elicitation_requested', {
      elicitationId: 'input-pending',
      request: { mode: 'form', requestedSchema: { type: 'object', properties: {} } },
    }, 'turn-input'),
  ]).turns[0];
  assert.equal(pendingInputTurn.waitingInput, true);
  assert.equal(pendingInputTurn.items[0].status, 'waiting');

  const interruptedTurn = projectAcpTimeline([
    event(16, 'turn_started', { status: 'running' }, 'turn-interrupted'),
    event(17, 'tool_call', { update: {
      toolCallId: 'tool-interrupted',
      title: '执行长任务',
      kind: 'execute',
      status: 'in_progress',
    } }, 'turn-interrupted'),
    event(18, 'permission_requested', {
      toolCallId: 'permission-interrupted',
      request: { options: [] },
    }, 'turn-interrupted'),
    event(19, 'elicitation_requested', {
      elicitationId: 'input-interrupted',
      request: { mode: 'form', requestedSchema: { type: 'object', properties: {} } },
    }, 'turn-interrupted'),
    event(20, 'turn_completed', {
      status: 'Interrupted',
      error: null,
      recoveryReason: 'application_restarted',
    }, 'turn-interrupted'),
  ]).turns[0];
  assert.equal(interruptedTurn.status, 'Interrupted');
  assert.equal(interruptedTurn.waitingInput, false);
  assert.deepEqual(
    interruptedTurn.items.map(item => item.status),
    ['cancelled', 'cancelled', 'cancelled'],
    'terminal recovery must not leave tool, permission, or input items visually running',
  );

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
  assert.equal(commandTurn.operationCount, 1);
  assert.equal(commandTurn.failedOperationCount, 0);
  assert.ok(command.output.includes('Unknown JSON field'));
  assert.equal(
    command.output,
    'Unknown JSON field: \"baseRefOid\"\nworktree /workspace/pinvou3\n',
    'command output must not render ANSI colors or OSC hyperlinks as garbage',
  );

  assert.equal(workspaceMarkdownResource('docs/report.md'), 'docs/report.md');
  assert.equal(workspaceMarkdownResource('file:///workspace/report.md'), 'file:///workspace/report.md');
  assert.equal(workspaceMarkdownResource('https://example.com/report.md'), '');
  assert.deepEqual(toolWorkspaceResources({
    locations: [{ path: '/workspace/docs/report.md' }],
    content: [
      { type: 'content', content: { type: 'resource_link', name: '/workspace/docs/diagram.svg', uri: '/workspace/docs/diagram.svg' } },
      { type: 'diff', path: '/workspace/docs/generated.svg' },
    ],
  }), [
    { path: '/workspace/docs/report.md', name: 'report.md' },
    { path: '/workspace/docs/diagram.svg', name: 'diagram.svg' },
    { path: '/workspace/docs/generated.svg', name: 'generated.svg' },
  ]);
  assert.deepEqual(collectToolWorkspaceResources([
    { tool: { locations: [{ path: '/workspace/docs/report.md' }] } },
    { tool: { locations: [{ path: '/workspace/docs/report.md' }, { path: '/workspace/docs/diagram.svg' }] } },
  ]), [
    { path: '/workspace/docs/report.md', name: 'report.md' },
    { path: '/workspace/docs/diagram.svg', name: 'diagram.svg' },
  ]);
  assert.equal(
    stripTerminalControlSequences('\u009b32m✓ passed\u009b0m'),
    '✓ passed',
    '8-bit CSI sequences must also be stripped',
  );

  assert.equal(appendAcpEvent(events, events[0]).length, events.length, 'duplicate seq must be ignored');
  assert.equal(appendAcpEvent(events.slice(0, 2), events[2]).length, 3);
  const liveAfterSnapshot = event(14, 'agent_message_chunk', {
    update: { content: { type: 'text', text: '重连期间到达' } },
  });
  const otherSessionEvent = { ...event(99, 'turn_started', {}, 'other'), sessionId: 'session-2' };
  assert.deepEqual(
    mergeAcpTimelineSnapshot(events.slice(0, 2), [otherSessionEvent, liveAfterSnapshot], 'session-1')
      .map(item => item.seq),
    [1, 2, 14],
    'a paged snapshot must merge live events without retaining another Session',
  );

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
  const detachedShell = readFileSync(path.join(root, 'src', 'app', 'DetachedShell.jsx'), 'utf8');
  const lazyCodexView = readFileSync(
    path.join(root, 'src', 'features', 'codex', 'LazyCodexAcpView.jsx'),
    'utf8',
  );
  const i18n = readFileSync(path.join(root, 'src', 'shared', 'i18n.js'), 'utf8');
  const navigationComponents = readFileSync(path.join(root, 'src', 'components', 'layout', 'NavigationComponents.jsx'), 'utf8');
  assert.ok(main.includes("currentView === 'codex'"));
  assert.ok(main.includes('<CodexAcpView'));
  assert.match(
    lazyCodexView,
    /lazy\(\(\) => import\('\.\/CodexAcpView\.jsx'\)/,
    'the ACP workspace must stay out of the initial WebUI bundle',
  );
  assert.ok(
    detachedShell.includes("../features/codex/LazyCodexAcpView.jsx")
      && !detachedShell.includes("../features/codex/CodexAcpView.jsx"),
    'detached windows must not restore a static import of the lazy ACP workspace',
  );
  assert.match(
    lazyCodexView,
    /<Suspense fallback=\{\([\s\S]*?<CodexAcpWorkspace/,
    'the lazy ACP workspace must render a stable loading state',
  );
  assert.ok(main.includes('codexAcpSupported &&'), 'Codex entry must stay platform capability-gated');
  assert.ok(main.includes(".concat(codexHistory)"),
    'Codex sessions must share the global recent-session list');
  assert.ok(main.includes("taskKind: 'codex'") && main.includes("testId: 'codex-sidebar-item'"),
    'global sessions must visually identify Codex records');
  assert.ok(main.includes("useState('pinned_first')"),
    'pinned sessions float first by default; unpinned work and code sessions still mix by recent update time');
  assert.match(
    main,
    /if \(type === 'turn_started'\) \{[\s\S]*?refreshCodexSessions\(\)\.catch\(\(\) => \{\}\);[\s\S]*?\} else if \(type === 'turn_completed'\)/,
    'an accepted ACP turn must refresh the shared recent-session list while it is still running',
  );
  assert.ok(main.includes("{ id: 'code', label: t.sidebarTaskFilterCode }")
    && main.includes("if (taskListFilter === 'code') return chat.taskKind === 'codex';")
    && i18n.includes("sidebarTaskFilterCode: '代码'")
    && i18n.includes("sidebarTaskFilterCode: 'Code'")
    && i18n.includes("sidebarTaskFilterCode: 'コード'"),
  'the task-list Code filter must show only Codex sessions in every supported locale');
  assert.ok(main.includes('leadingIcon: <PinvouLogo')
    && main.includes('<AcpAgentLogo agentId={session.agent_id} className="h-[18px] w-[18px]"')
    && main.includes('<Clock size={18} />'),
  'work, Codex, and scheduled sessions must expose equally sized type icons');
  assert.ok(navigationComponents.includes('group flex h-11 items-center')
    && navigationComponents.includes('flex h-5 w-5 shrink-0 items-center justify-center'),
  'all recent-session rows and their icon canvases must keep a consistent size');
  assert.ok(!main.includes("w-[280px] bg-[#1E1F20]")
    && main.includes("activeTheme === 'light'")
    && main.includes("? 'bg-[#F0F4F9]'")
    && main.includes(": (isSidebarOpen ? 'bg-[#1E1F20]' : 'bg-[#131314]')"),
  'the sidebar must choose one theme background instead of emitting conflicting light and dark classes');
  assert.ok(!/<NavItem[\s\S]{0,180}label="Codex"/.test(main),
    'Codex must not occupy a standalone primary-navigation tab');

  const chatCommands = readFileSync(path.join(root, 'src-tauri', 'src', 'app', 'commands', 'chat.rs'), 'utf8');
  const codexCommands = readFileSync(path.join(root, 'src-tauri', 'src', 'app', 'commands', 'codex.rs'), 'utf8');
  assert.ok(chatCommands.includes('ACP 代码会话必须通过独立代码页面发送'));
  assert.ok(codexCommands.includes('pub async fn codex_acp_prompt'));
  assert.ok(codexCommands.includes('pub async fn set_codex_acp_mode'));
  assert.ok(codexCommands.includes('pub async fn get_codex_acp_pending_elicitations'));
  assert.ok(codexCommands.includes('pub async fn respond_codex_acp_elicitation'));
  assert.ok(codexCommands.includes('list_codex_acp_sessions'));
  assert.ok(codexCommands.includes('workspace_path: Option<String>'), 'Codex creation must accept an explicit project directory');
  assert.ok(codexCommands.includes('agent_id: Option<String>')
    && codexCommands.includes('set_acp_workspace(&session.metadata.id, backend'),
  'code-session creation must bind the selected ACP Agent for the lifetime of the session');
  assert.ok(codexCommands.includes('or(Some("pinvou"))'),
    'code-session creation without an explicit agent must default to the built-in Pinwu backend');
  assert.ok(codexCommands.includes('pub async fn login_acp_agent')
    && codexCommands.includes('pub fn open_acp_agent_login_url')
    && codexCommands.includes('pub async fn submit_acp_agent_login_code'),
  'all ACP Agents must expose the hosted login flow, including Claude authorization-code input');
  assert.ok(codexCommands.includes('validate_codex_project_workspace'), 'project workspace must be validated before session creation');

  const runtime = readFileSync(path.join(root, 'src-tauri', 'src', 'features', 'codex_acp', 'mod.rs'), 'utf8');
  // Wave 2 把登录态探测拆到 login.rs、Kimi 内省拆到 introspect.rs；auth/login 相关
  // 断言需读对应子模块。
  const loginMod = readFileSync(path.join(root, 'src-tauri', 'src', 'features', 'codex_acp', 'login.rs'), 'utf8');
  const introspectMod = readFileSync(path.join(root, 'src-tauri', 'src', 'features', 'codex_acp', 'introspect.rs'), 'utf8');
  assert.ok(runtime.includes('self.session_store.touch_activity(session_id)'),
    'an accepted ACP turn must persist the session activity timestamp before it starts');
  assert.ok(runtime.includes('interrupt_orphaned_turns("application_restarted")')
    && runtime.includes('cancel_without_active_prompt')
    && runtime.includes('runtime.busy.load(Ordering::Acquire)'),
  'app restart and stale stop must close orphaned ACP turns without cancelling an idle runtime');
  assert.ok(runtime.includes('LoadSessionRequest::new(saved_id.clone(), workspace.clone())'));
  assert.ok(runtime.includes('NewSessionRequest::new(workspace)'));
  assert.ok(runtime.includes('会话绑定的项目目录已不可用'), 'missing projects must not silently fall back');
  assert.ok(runtime.includes('apply_saved_mode('), 'saved Full Access mode must be restored after new/load');
  assert.ok(runtime.includes('cancel_pending_permissions_with_bridge(&session_id, Some(&runtime.bridge))')
    && runtime.includes('"outcome": "cancelled"'),
  'account switching must persist permission cancellation through the removed runtime bridge');
  assert.ok(runtime.includes('cancel_pending_elicitations_with_bridge(&session_id, Some(&runtime.bridge))'),
  'account switching must persist elicitation cancellation through the removed runtime bridge');
  assert.ok(runtime.includes('AgentBackend::ClaudeAcp')
    && runtime.includes('AgentBackend::KimiAcp')
    && runtime.includes('command.arg("acp")')
    && runtime.includes('CLAUDE_ACP_PACKAGE'),
  'the shared ACP runtime must launch Claude through its adapter and Kimi through kimi acp');
  assert.ok(loginMod.includes('cli_status_success(claude, &["auth", "status"])')
    && introspectMod.includes('kimi_authenticated')
    && runtime.includes('run_agent_login')
    && loginMod.includes('capture_agent_login_output')
    && runtime.includes('submit_agent_login_code'),
  'ACP auth status and hosted login must be driven by the real Agent CLIs instead of credential-file existence alone');
  assert.ok(!runtime.includes('runtime.prompt(content, mode_id)'), 'prompt must not overwrite acknowledged config with local UI mode');

  const codexView = readFileSync(path.join(root, 'src', 'features', 'codex', 'CodexAcpView.jsx'), 'utf8');
  const runtimeNotices = readFileSync(path.join(root, 'src', 'features', 'codex', 'AcpRuntimeNotices.jsx'), 'utf8');
  const acpClient = readFileSync(path.join(root, 'src', 'platform', 'acp', 'client.js'), 'utf8');
  const runtimeNoticeState = readFileSync(
    path.join(root, 'src', 'features', 'codex', 'runtimeNoticeState.js'),
    'utf8',
  );
  assert.ok(codexView.includes('copy.permissionRequest(agentName)')
    && codexView.includes('tool.title || copy.protectedOperation')
    && codexView.includes('label={copy.command}')
    && codexView.includes('copy.operationArguments')
    && codexView.includes('copy.allowOnce')
    && codexView.includes('copy.allowSession')
    && codexView.includes('copy.reject')
    && codexView.includes('copy.handled')
    && codexView.includes('copy.expired'),
  'the legacy ACP permission card must use the shared zh/en/ja conversation copy');
  const codexWorkspace = readFileSync(path.join(root, 'src', 'features', 'codex', 'CodexWorkspacePanel.jsx'), 'utf8');
  const runtimeStatus = readFileSync(path.join(root, 'src', 'features', 'codex', 'runtimeStatus.js'), 'utf8');
  const homeModeSwitcher = readFileSync(path.join(root, 'src', 'features', 'conversation', 'HomeModeSwitcher.jsx'), 'utf8');
  const iosControls = readFileSync(path.join(root, 'src', 'components', 'IosControls.jsx'), 'utf8');
  const codexLogo = readFileSync(path.join(root, 'src', 'components', 'CodexLogo.jsx'), 'utf8');
  const pinvouLogo = readFileSync(path.join(root, 'src', 'components', 'PinvouLogo.jsx'), 'utf8');
  const conversationView = readFileSync(path.join(root, 'src', 'features', 'conversation', 'ConversationTimeline.jsx'), 'utf8');
  const baseStyles = readFileSync(path.join(root, 'src', 'styles', 'base.css'), 'utf8');
  const boundedPermissionOptionClass = 'max-w-full min-w-0 whitespace-normal break-all';
  assert.ok(codexView.includes(boundedPermissionOptionClass)
    && conversationView.includes(boundedPermissionOptionClass),
  'long ACP permission option labels must wrap inside both unified and legacy permission cards');
  assert.ok(conversationView.includes('function runningToolLabel(item, copy)')
    && conversationView.includes("return copy.shellCommand;")
    && !conversationView.includes('runningItem.tool.name || runningItem.tool.title')
    && conversationView.includes('data-testid="conversation-tool-group-summary"')
    && conversationView.includes('min-w-0 flex-1 truncate'),
  'running tool groups must use bounded semantic labels instead of rendering raw command titles');
  const boundedLongTextClass = 'whitespace-pre-wrap break-words [overflow-wrap:anywhere]';
  assert.ok(codexView.includes(boundedLongTextClass)
    && conversationView.includes(boundedLongTextClass)
    && codexView.includes('max-h-80 max-w-full overflow-auto whitespace-pre')
    && conversationView.includes('max-h-80 max-w-full overflow-auto whitespace-pre'),
  'reasoning, plan, permission, and terminal content must stay within both timeline implementations');
  assert.ok(codexView.includes("open_codex_workspace_resource")
    && conversationView.includes('onOpenResource={onOpenResource}')
    && codexWorkspace.includes("const loadedDirectories = ['', ...expanded]")
    && codexWorkspace.includes("document.visibilityState !== 'visible'")
    && codexWorkspace.includes("sessionId && tab === 'changes'"),
  'ACP workspace resources must open in the preview and event refreshes must reload expanded directories');
  assert.ok(codexView.includes('workspacePath'), 'selected project directory must reach the Tauri command');
  assert.ok(acpClient.includes("directory: true")
    && codexView.includes('pickAcpWorkspace'),
  'new code sessions must expose the platform-specific directory picker');
  assert.ok(codexView.includes('workspaceHandle: draftWorkspaceHandle')
    && acpClient.includes("invokeTauri('create_codex_acp_session', { workspacePath, agentId })")
    && acpClient.includes("invokeRequiredWebCommand('web_access_create_codex_acp_session'"),
  'selected project directories must use native paths on desktop and opaque grants on Web');
  assert.ok(!codexView.includes('data-testid="acp-agent-selector"')
    && codexView.includes('onCodeAgentChange={selectDraftAgent}')
    && codexView.includes('agentId: draftAgentId')
    && runtimeStatus.includes('listAcpAgents()'),
  'the top code tabs must be the only Agent selector and bind the selected Agent on first send');
  assert.ok(codexView.includes("const AGENT_SELECTION_KEY = 'pinvou_codex_agent_selection'")
    && codexView.includes("useState(loadAgentSelection() || 'pinvou')")
    && codexView.includes('saveAgentSelection(agentId)'),
  'code draft agent must default to Pinwu and persist the user-selected agent across reopens');
  assert.ok(codexView.includes("invoke('login_acp_agent'")
    && codexView.includes("invoke('open_acp_agent_login_url'")
    && codexView.includes("invoke('submit_acp_agent_login_code'")
    && runtimeNotices.includes('status.login_code')
    && runtimeNotices.includes('status.login_input_required')
    && !runtimeNotices.includes("status.agent_id === 'claude'")
    && codexView.includes('if (!isNativeAgent && !activeStatus?.authenticated)')
    && codexView.includes('isAcpAuthenticationFailure(latest)'),
  'the code page must host browser/device-code login, block unauthenticated prompts, and refresh after token expiry');
  assert.ok(codexView.includes('codexCopy.temporarySession'), 'temporary sessions must remain an explicit choice');
  assert.ok(codexView.includes('DRAFT_ATTACHMENT_KEY')
    && codexView.includes('const created = await createSession()')
    && codexView.includes('workspacePath: draftWorkspacePath')
    && codexView.includes('workspaceHandle: draftWorkspaceHandle'),
  'the code home must keep a temporary draft and create its Codex session only on first send');
  assert.ok(!codexView.includes('createSession(null)'),
  'the native (pinvou) first-send path must also forward the selected draft workspace');
  assert.ok(codexView.includes('!activeId && (')
    && codexView.includes('data-testid="codex-workspace-selector"')
    && codexView.includes('codexCopy.recentProjects'),
  'only the draft composer must expose temporary, directory picker, and recent-project choices');
  assert.ok(codexView.includes('data-testid="codex-workspace-unavailable"')
    && codexView.includes('codexCopy.projectMissing')
    && codexView.includes('data-testid="codex-recreate-session"')
    && codexView.includes('recreateUnavailableWorkspaceSession')
    && codexView.includes('beginDraft(null)')
    && codexView.includes('setWorkspaceMenuOpen(true)'),
  'missing project sessions must keep their history and offer a link into the existing new-session workspace menu');
  const composerFooterIndex = codexView.indexOf('data-testid="codex-composer-footer"');
  const workspaceSelectorIndex = codexView.indexOf('data-testid="codex-workspace-selector"');
  const attachmentButtonIndex = codexView.indexOf('title={codexCopy.addAttachment}', composerFooterIndex);
  assert.ok(composerFooterIndex >= 0
    && workspaceSelectorIndex > composerFooterIndex
    && attachmentButtonIndex > workspaceSelectorIndex,
  'the draft workspace selector must live in the composer footer before the attachment control');
  const accountTriggerIndex = codexView.indexOf('data-testid="acp-account-menu-trigger"');
  const composerConfigsIndex = codexView.indexOf('data-testid="codex-composer-configs"');
  assert.ok(accountTriggerIndex > composerFooterIndex
    && composerConfigsIndex > accountTriggerIndex,
  'Codex session controls must live in the composer footer right of the connection status');
  assert.ok(codexView.includes('composerControlsVisible && !isNativeAgent && (')
    && codexView.includes('data-testid="codex-composer-configs"')
    && !codexView.includes('创建后同步'),
  'Codex controls must render from the session report or, in draft, the cached agent snapshot');
  const draftControlsModule = readFileSync(
    path.join(root, 'src', 'features', 'codex', 'acp-draft-controls.js'),
    'utf8'
  );
  assert.ok(draftControlsModule.includes('pinvou_codex_draft_controls')
    && codexView.includes('acp-draft-controls.js')
    && codexView.includes('resolveAcpSessionControls(sessionControlsInfo || draftControlsInfo)')
    && codexView.includes('stageDraftConfigSelection')
    && codexView.includes('applyDraftConfigSelections(targetId, created.info)'),
  'the draft composer must prefill model, mode and config controls from the agent cache and apply staged choices on first send');
  assert.ok(codexView.includes('function CodexComposerConfigSelect')
    && codexView.includes('data-testid={testId || `codex-config-${id}`}')
    && codexView.includes('<ComposerPopover')
    && codexView.includes('focus-within:ring-2 focus-within:ring-[#007AFF]/10'),
  'Codex session controls must use the unified visual selector with the app-styled ComposerPopover menu');
  assert.ok(!codexView.includes('<aside'),
    'Codex must use the app-wide session sidebar instead of rendering a second sidebar');
  assert.ok(homeModeSwitcher.includes("labelKey: 'work'") && homeModeSwitcher.includes("labelKey: 'code'")
    && homeModeSwitcher.includes("selectedAgentId || 'codex'"),
  'the home composer must expose Work/Code modes and the current Codex code agent');
  assert.ok(homeModeSwitcher.includes("key: 'design'")
    && homeModeSwitcher.includes('HOME_DESIGN_MODE_ENABLED = true'),
  'Design must share the real home mode entry with Work and Code');
  assert.ok(homeModeSwitcher.includes('function normalizeCodeAgents(codeAgents, selectedAgentId)')
    && homeModeSwitcher.includes("agent?.agent_id || agent?.id")
    && homeModeSwitcher.includes("agent?.agent_name || agent?.display_name || agent?.name")
    && homeModeSwitcher.includes('onCodeAgentChange ? codeAgents : undefined')
    && codexView.includes('codeAgents={agents}')
    && !homeModeSwitcher.includes('CODE_AGENT_OPTIONS'),
  'the code home must derive its ACP Agent selector from the target desktop inventory without a frontend whitelist');
  assert.ok(homeModeSwitcher.includes('prominent')
    && iosControls.includes('if (compact)')
    && iosControls.includes("const heightClass = prominent ? 'h-10' : 'h-9'")
    && iosControls.includes('transition-transform duration-200 ease-out'),
  'the home mode switcher must keep the PR #16 sliding segmented-control treatment');
  assert.ok(main.includes('function handleSwitchHomeMode(mode)')
    && main.includes("mode === 'code' && codexAcpSupported")
    && main.includes('setCodexDraftEpoch(value => value + 1)')
    && main.includes("setCurrentView('codex')"),
  'selecting Codex must continue to enter the existing Codex draft page');
  const acpAgentLogo = readFileSync(path.join(root, 'src', 'features', 'codex', 'AcpAgentLogo.jsx'), 'utf8');
  // 契约（2026-08-12 更新）：Design 入口必须回到 ChatView design 模式；从 code
  // 页切回时保留原工作会话（不强制 createNewSession，否则新建 plain 会话把
  // 用户切过的 Plan 顶成 Yolo），仅草稿态才新建会话。
  assert.match(main,
    /else if \(mode === 'design'\) \{[\s\S]*?savePinvouModeState\(\{ mode: 'design' \}[^;]*;[\s\S]*?createNewSession\(\);[\s\S]*?setCurrentView\('chat'\)/,
    'selecting Design from the shared mode entry must return to ChatView design mode');
  assert.ok(
    main.includes("if (bridge.available && !bridge.activeSessionId) bridge.sessions.createNewSession();"),
    '从 code 页切回 design 时保留原工作会话，仅草稿态新建');
  assert.ok(
    main.includes("createPinvouModeScopeKey(bridge.activeSessionId)"),
    '切回 design 时 pinvou 模式按会话 scope 保存，ChatView 挂载才能读回');
  assert.ok(codexLogo.includes("brand-icons/openai.svg")
    && acpAgentLogo.includes('<CodexLogo')
    && acpAgentLogo.includes("brand-icons/claude.png")
    && acpAgentLogo.includes("alt={title || 'Claude Code'}")
    && acpAgentLogo.includes("brand-icons/kimi-code.png")
    && acpAgentLogo.includes("alt={title || 'Kimi'}")
    && main.includes('<AcpAgentLogo')
    && codexView.includes('<AcpAgentLogo'),
  'ACP sessions must keep the Codex mark and expose distinct Claude/Kimi identities');
  assert.ok(pinvouLogo.includes("resolveAppAssetUrl('assets/brand/brand-blue.png')")
    && chatView.includes('assistantAvatar={(')
    && chatView.includes('<PinvouLogo className="h-5 w-5" title={chatViewCopy.agentName}')
    && codexView.includes('<AcpAgentLogo agentId={activeAgentId} className="h-5 w-5"'),
  'assistant avatars must use the Pinvou and selected ACP Agent identity marks');
  assert.ok(conversationView.includes('思考中'), 'running reasoning must expose a timer label');
  assert.ok(conversationView.includes('执行步骤'), 'tool items must use a compact presentation group');
  assert.ok(!codexView.includes("useState(state === 'failed')"),
    'failed operation details must stay collapsed until the user opens them');
  assert.ok(!codexView.includes('useState(running || failed)'),
    'operation groups must not expand automatically for running or failed items');
  assert.ok(!codexView.includes("if (state === 'running') setOpen(true)"),
    'running operation details must not interrupt the conversation by auto-expanding');
  assert.ok(!codexView.includes('if (running) setOpen(true)'),
    'running operation groups must remain compact by default');
  assert.ok(runtimeNoticeState.includes("HTTP\\s*402")
    && runtimeNoticeState.includes("kind = 'entitlement'")
    && runtimeNotices.includes('data-testid="acp-service-failure"'),
  'membership HTTP 402 failures must become a recoverable service card instead of a bare error');
  assert.ok(codexView.includes("invoke('switch_acp_agent_account'")
    && codexView.includes('data-testid="acp-account-menu-trigger"')
    && codexView.includes('data-testid="acp-account-menu"')
    && codexView.includes('switchAccountAffectsSessions'),
  'every ACP Agent must expose an account menu and a force account-switch action');
  assert.ok(codexView.includes('const movingUp = element.scrollTop < lastScrollTopRef.current - 1')
    && codexView.includes('if (movingUp) autoScrollRef.current = false')
    && codexView.includes('if (autoScrollRef.current)')
    && codexView.includes('scrollConversationToBottom')
    && codexView.includes('codexCopy.latest')
    && codexView.includes('bottom-full')
    && !codexView.includes('bottom-[106px]'),
  'Codex streaming must pause auto-follow and place the return action above, not over, the composer');
  assert.ok(!codexView.includes('<JsonBlock'), 'raw ACP JSON must not leak into normal command UI');
  assert.ok(codexView.includes('await submitAcpPrompt({')
    && codexView.includes('attachments: readyAttachments.map(attachment => attachment.result)')
    && codexView.includes('workspaceReferences')
    && acpClient.includes("invokeTauri('web_access_codex_acp_prompt'"),
  'Codex prompts must keep external attachments and workspace references as separate inputs');
  assert.ok(!codexView.includes('if (activeId && !sessionInfo)')
    && !codexView.includes('throw new Error(codexCopy.sessionSyncing)')
    && !codexView.includes('targetInfo')
    && !codexView.includes('(activeId && !sessionInfo) ||'),
  'Codex prompts must let the backend initialize or restore the ACP session instead of blocking forever on stale UI state');
  assert.ok(codexView.includes('<CodexWorkspacePanel')
    && codexView.includes('copy={t.uiCodexWorkspace}')
    && codexWorkspace.includes('copy.files')
    && codexWorkspace.includes('copy.changed'),
  'active Codex sessions must expose a right-side Files/Changes workspace panel');
  assert.ok(codexWorkspace.includes("WORKSPACE_WIDTH_KEY = 'pinvou_codex_workspace_width'")
    && codexWorkspace.includes('onMouseDown={startPanelResize}')
    && codexWorkspace.includes('onDoubleClick={resetPanelWidth}')
    && codexWorkspace.includes("document.body.style.cursor = 'col-resize'"),
  'the Codex workspace panel must support persisted drag resizing and double-click reset');
  assert.ok(codexWorkspace.includes('listAcpWorkspace({')
    && codexWorkspace.includes('previewAcpWorkspaceFile({')
    && codexWorkspace.includes("invoke('get_codex_workspace_changes'")
    && codexWorkspace.includes("invoke('get_codex_workspace_diff'"),
  'the workspace panel must use scoped file, preview, and read-only change commands');
  assert.ok(!codexWorkspace.includes('discard') && !codexWorkspace.includes('stage_codex'),
    'the first workspace panel must not expose destructive discard or staging actions');
  assert.ok(codexView.includes('function ElicitationCard'),
    'Codex request_user_input must have a first-class conversation item');
  assert.ok(codexView.includes('<QuestionChoiceCard'),
    'Codex and DeepSeek must share the same choice-card presentation');
  assert.ok(codexView.includes('loadAcpPendingElicitations(id)'),
    'pending Codex input requests must recover when a session is reopened');
  assert.ok(codexView.includes("invoke('respond_codex_acp_elicitation'"),
    'Codex input answers must be returned through the ACP request');
  assert.ok(conversationView.includes('className={`codex-markdown'), 'conversation Markdown must keep the isolated Codex style scope');
  assert.ok(codexView.includes('<ConversationTurn'), 'Codex must render through the shared Turn renderer by default');
  assert.ok(codexView.includes('<ConversationActivityIndicator')
    && codexView.includes('turn={activeConversationTurn}')
    && conversationView.includes("if (!turn || turn.status !== 'running') return null"),
  'Codex must show the shared composer timer only while the active turn is running');
  assert.ok(codexView.includes('data-testid="acp-session-loading"')
    && codexView.includes('const [sessionLoading, setSessionLoading] = useState(false)')
    && codexView.includes('disabled={!sessionReady')
    && codexView.includes('if (activeId && !sessionReady) return;')
    && !codexView.includes('setError(codexCopy.sessionSyncing)')
    && !codexView.includes('throw new Error(codexCopy.sessionSyncing)'),
  'ACP session restoration must show a loading state and suppress sending without reporting a red error');
  assert.ok(codexView.includes('const activeStatus = status?.agent_id === activeAgentId ? status : null')
    && codexView.includes('status={activeStatus}')
    && runtimeStatus.includes('requestSeqRef.current[agentId] !== sequence')
    && runtimeStatus.includes('agentId !== activeAgentIdRef.current')
    && codexView.includes('[activeAgentId, activeStatus?.login_in_progress]'),
  'switching ACP sessions must never render or keep polling the previous Agent status');
  assert.ok(codexView.includes('<ConversationMarkdown')
    && codexView.includes('openAcpExternalUrl(url)'),
  'both unified and fallback Codex messages must route links through the host opener');
  assert.ok(baseStyles.includes('.codex-markdown ul { list-style:disc outside; }'),
    'Codex unordered lists must retain bullets after Tailwind preflight');
  assert.ok(baseStyles.includes('.codex-markdown ol { list-style:decimal outside; }'),
    'Codex ordered lists must retain numbering after Tailwind preflight');

  // 原生（品悟）车道底栏控件契约：仅 isNativeAgent 渲染、与工作/设计页共用同一套
  // 共享 composer 控件（ComposerModeChip / ComposerModelSelector / ComposerKbSelector，
  // 显式会话态驱动 props 绕开 bridge 聊天 active 绑定）、直调 per-session 命令、
  // 并带与 ChatView 同款的语音输入按钮（bridge.voice 写回 draft）。
  const composerControls = readFileSync(path.join(root, 'src', 'features', 'chat', 'composer-controls.jsx'), 'utf8');
  assert.ok(codexView.includes('data-testid="native-composer-controls"')
    && codexView.includes('{isNativeAgent && (')
    && codexView.includes('<ComposerModeChip')
    && codexView.includes('<ComposerModelSelector')
    && codexView.includes('<ComposerKbSelector')
    && codexView.includes('<ComposerToolMenu')
    && codexView.includes('multiAgentEnabled={nativeMultiAgentEnabled}')
    && codexView.includes('multiAgentAvailable={nativeMultiAgentAvailable}')
    && codexView.includes('onToggleMultiAgent={switchNativeMultiAgent}')
    && codexView.includes('triggerTestId="native-tools"')
    && codexView.includes('scope="code"')
    && codexView.includes('mountedId={nativeMountedId}')
    && codexView.includes('data-testid="codex-voice-input"'),
  'the native lane must mount the shared composer controls (work/design style) plus the voice input button behind the native-agent gate');
  assert.ok(codexView.includes('renderToolItem={isNativeAgent')
    && !codexView.includes('renderToolItem={isNativeAgent && nativeMultiAgentEnabled')
    && codexView.includes('{subagentPanel && activeSession && isNativeAgent && (')
    && !codexView.includes('{subagentPanel && activeSession && isNativeAgent && nativeMultiAgentEnabled && (')
    && codexView.includes("if (typeof window === 'undefined' || !isNativeAgent) return undefined;")
    && codexView.includes('<SubagentTranscriptPanel')
    && codexView.includes("window.addEventListener('pinvou:open-subagent'")
    && codexView.includes('<ToolCard'),
  'the native lane must always expose factual delegated-agent cards and transcripts; product mode only controls the Pinvou roster and reminder');
  const interactionCommands = readFileSync(
    path.join(root, 'src-tauri', 'src', 'app', 'commands', 'interaction.rs'),
    'utf8',
  );
  assert.ok(interactionCommands.includes('multi_agent_available: pool.multi_agent_mode_available(&session_id)')
    && codexView.includes('multiAgentAvailable: Boolean(modeState && modeState.multi_agent_available)'),
  'the native multi-agent control must consume the backend SessionPolicy availability instead of a literal UI capability');
  // 语音输入生命周期契约：bridge.voice 的写回守卫只绑定聊天侧 activeSessionId，
  // 代码页卸载（切模式/视图）前必须取消进行中的录音/转写，否则识别结果可能写回
  // 已卸载组件（草稿态 null→null 时守卫放行并显示「已完成」但文本丢失）。
  assert.ok(codexView.includes('nativeVoiceInputRef = useRef(nativeVoiceInput)')
    && codexView.includes('bridge.voice.cancelVoiceInput()')
    && codexView.includes("voice.status === 'requesting_permission'"),
  'the code page must cancel an in-flight voice input before unmount so results cannot be written back to a detached composer');
  // 语音失败提示条须带 ChatView 同款「去依赖体检」入口（recognition_failed + 本地
  // ASR 可安装 + onGotoSettings 时渲染 voiceGotoDeps 按钮）。
  assert.ok(codexView.includes("can('localModelSetup') && can('dependencyInstall')")
    && codexView.includes('nativeVoiceInput.category === \'recognition_failed\'')
    && codexView.includes('t.voiceGotoDeps'),
  'the code page voice notice must offer the dependency-check shortcut on recognition failure like ChatView');
  // plain（非 native）车道仍走自绘 CodexComposerConfigSelect 配置组，不随 native 车道
  // 迁移到共享组件；共享 config select 保留 ACP testid 契约。
  assert.ok(codexView.includes('data-testid="codex-composer-configs"')
    && codexView.includes('{composerControlsVisible && !isNativeAgent && (')
    && codexView.includes('function CodexComposerConfigSelect')
    && codexView.includes('data-testid={testId || `codex-config-${id}`}'),
  'the plain lane must keep its self-drawn config select group while the shared config select keeps the ACP testid contract');
  assert.ok(codexView.includes("invoke('get_session_model_id'")
    && codexView.includes("invoke('set_session_model'")
    && codexView.includes("invoke('session_mount_collection'")
    && codexView.includes("invoke('session_unmount_collection'")
    && codexView.includes("invoke('session_mounted_collection'")
    && codexView.includes("invoke('get_mode_state'")
    && codexView.includes("invoke('set_plan_mode_next'")
    && codexView.includes("invoke('exit_plan_to_yolo'")
    && codexView.includes("invoke('set_multi_agent_mode'")
    && codexView.includes("invoke('cancel_generation'"),
  'native composer controls must switch via per-session commands with an explicit sessionId');
  assert.ok(!codexView.includes('bridge.models.')
    && !codexView.includes('bridge.knowledge.')
    && !codexView.includes('bridge.interaction.')
    && !codexView.includes('bridge.chat.'),
  'the code lane must never call bridge chat-active-bound methods for composer controls');
  assert.ok(codexView.includes('nativeDraftControls')
    && /applyNativeDraftControls\(sessionId\)[\s\S]{0,1400}set_multi_agent_mode[\s\S]{0,250}setNativeDraftControls\(\{\}\)/.test(codexView)
    && /const created = await createSession\(\)[\s\S]{0,180}await applyNativeDraftControls\(targetId\)/.test(codexView),
  'draft-state control selections, including multi-agent mode, must be applied after session creation and before first send');
  assert.ok(
    /await applyNativeDraftControls\(targetId\);[\s\S]{0,520}await refreshNativeControls\(targetId\);[\s\S]{0,1800}await invoke\('chat'/.test(codexView),
    'a newly created native session must refresh authoritative multi-agent state after applying draft controls and before its first turn',
  );
  assert.ok(codexView.includes('nativeControlsSessionRef.current === activeId'),
  'session control state must be scoped to its owning session to avoid cross-session flashes');
  assert.ok(/refreshNativeControls\(sessionId\)[\s\S]{0,900}sessionId !== activeIdRef\.current[\s\S]{0,100}return controls/.test(codexView),
  'an async control refresh from the previous native session must not overwrite the newly selected session');
  assert.ok(chatView.includes("from './composer-controls.jsx'")
    && !chatView.includes('const ComposerKbSelector = ')
    && !chatView.includes('const ComposerModeChip = ')
    && composerControls.includes('export { COMPOSER_ICON_BUTTON_CLASS, ComposerKbSelector, ComposerModeChip }'),
  'ChatView must consume the extracted composer controls module');
  assert.ok(composerControls.includes('mountedIdProp !== undefined')
    && composerControls.includes('modeProp != null')
    && composerControls.includes('busyProp !== undefined')
    && composerControls.includes('isTauriAvailable() && !explicitMountState')
    && composerControls.includes("if (collection.source === 'remote') {")
    && composerControls.includes('if (onMount) { onMount(collection.id); return; }')
    && composerControls.includes('if (onUnmount) { onUnmount(); return; }')
    && composerControls.includes('if (onSwitch) { onSwitch(target, { isPlan, busy }); return; }'),
  'extracted controls must keep explicit Code mounts local while preserving the bridge fallback');
  // 等值守卫：点击已激活模式必须早退，避免代码车道 onSwitch 路径每次点击都触发
  // 冗余 refreshNativeControls（3 次 invoke）；ChatView bridge 路径同样受益。
  assert.ok(composerControls.includes("(target === 'plan' && isPlan) || (target === 'yolo' && !isPlan)"),
  'ComposerModeChip must early-return when the clicked mode equals the active mode');

  // ── buildElicitationContent：保留属性 answerKey 不被 Object.prototype 吞掉 ──
  // requestedSchema 的 property key 后端仅校验非空，constructor/toString/__proto__ 是合法输入。
  // 普通 {} 会让 __proto__ 赋值触发 setter（字段在 JSON 序列化时静默丢失）、constructor/toString
  // 读/写命中 Object.prototype；无原型对象构造应保留全部字段。
  {
    const groups = [
      { questionId: 'constructor', answerKey: 'constructor', otherAnswerKey: null, multiSelect: false, answers: [{ label: 'A', value: 'A', other: false }] },
      { questionId: 'toString', answerKey: 'toString', otherAnswerKey: null, multiSelect: false, answers: [{ label: 'B', value: 'B', other: false }] },
      { questionId: '__proto__', answerKey: '__proto__', otherAnswerKey: null, multiSelect: false, answers: [{ label: 'X', value: 'X', other: false }] },
    ];
    const content = buildElicitationContent(groups);
    assert.equal(Object.prototype.hasOwnProperty.call(content, 'constructor'), true, 'constructor 作为 own property 保留');
    assert.equal(Object.prototype.hasOwnProperty.call(content, 'toString'), true, 'toString 作为 own property 保留');
    assert.equal(Object.prototype.hasOwnProperty.call(content, '__proto__'), true, '__proto__ 作为 own property 保留（普通 {} 会丢）');
    assert.equal(content['constructor'], 'A');
    assert.equal(content['toString'], 'B');
    assert.equal(content['__proto__'], 'X');
    assert.equal(JSON.stringify(content), '{"constructor":"A","toString":"B","__proto__":"X"}', 'JSON 序列化不得静默丢失保留键');
    assert.equal(Array.isArray(Object.getPrototypeOf(content)), false, 'content 保持无原型对象，不得被 __proto__ 赋值改原型');
  }

  console.log('codex_acp_timeline: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
