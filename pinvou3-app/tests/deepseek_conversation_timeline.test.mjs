#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-deepseek-conversation-'));
const conversationDir = path.join(temp, 'conversation');
mkdirSync(conversationDir, { recursive: true });
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
for (const file of ['conversation-model.js', 'deepseek-conversation.js']) {
  copyFileSync(
    path.join(root, 'src', 'features', 'conversation', file),
    path.join(conversationDir, file),
  );
}

try {
  const { projectDeepSeekConversation } = await import(
    `${pathToFileURL(path.join(conversationDir, 'deepseek-conversation.js')).href}?t=${Date.now()}`
  );
  const chatItems = [
    { id: 1, type: 'system', text: '会话已恢复' },
    { id: 2, type: 'user', text: '检查仓库' },
    { id: 3, type: 'assistant', html: '<p>先看状态。</p>', streaming: false },
    {
      id: 4,
      type: 'tool',
      toolId: 'shell-1',
      name: 'exec_shell',
      args: { command: 'git status', cwd: '/workspace/pinvou3' },
      output: 'clean',
      success: true,
      state: 'done',
    },
    {
      id: 5,
      type: 'tool',
      toolId: 'read-1',
      name: 'read_file',
      args: { path: 'README.md' },
      output: '# PINVOU',
      success: true,
      state: 'done',
    },
    { id: 6, type: 'artifact_card', path: '/tmp/report.md', title: '报告' },
    { id: 7, type: 'user', text: '继续' },
    { id: 8, type: 'assistant', html: '', streaming: true },
    { id: 9, type: 'user_input', resolved: false, questions: [] },
  ];
  const before = structuredClone(chatItems);
  const projected = projectDeepSeekConversation({
    chatItems,
    busy: true,
    thinking: { active: true, phase: 'thinking', startedAt: 123456 },
    tokens: { input: 320, max: 4096 },
    sessionId: 'session-1',
  });

  assert.deepEqual(chatItems, before, 'projection must never rewrite the DeepSeek chatItems fact source');
  assert.equal(projected.thread.id, 'session-1');
  assert.equal(projected.turns.length, 3, 'preamble and each user message must become stable turns');
  assert.equal(projected.turns[1].userText, '检查仓库');
  assert.deepEqual(
    projected.turns[1].items.map(item => item.type),
    ['agent_message', 'command_execution', 'tool', 'artifact'],
  );
  assert.deepEqual(
    projected.turns[1].presentation.map(item => item.type),
    ['agent_message', 'tool_group', 'artifact'],
    'consecutive operations must only be grouped in the presentation projection',
  );
  assert.equal(projected.turns[1].presentation[1].items.length, 2);
  assert.equal(projected.turns[1].items[1].legacyItem, chatItems[3], 'tool cards must retain the original item for provider rendering');
  assert.equal(projected.turns[2].status, 'running');
  assert.equal(projected.turns[2].startedAt, 123456);
  assert.equal(projected.turns[2].waitingPermission, true);
  assert.deepEqual(projected.turns[2].usage, { used: 320, size: 4096 });

  const history = projectDeepSeekConversation({ chatItems, busy: false, sessionId: 'session-1' });
  assert.equal(history.turns[2].status, 'completed');
  assert.equal(history.turns[2].startedAt, null, 'history must not invent unavailable timing data');

  const chatView = readFileSync(path.join(root, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
  assert.ok(chatView.includes('<ConversationTimeline'), 'DeepSeek must render through the shared timeline by default');
  assert.ok(chatView.includes('renderToolItem='), 'DeepSeek tools must remain delegated to the existing ToolCard');
  assert.ok(chatView.includes('<ThinkingBubble'), 'the original rendering path must remain available as a fallback');
  assert.ok(chatView.includes("pinvou_conversation_ui_v2"), 'the local rollback switch must be explicit');

  console.log('deepseek_conversation_timeline: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
