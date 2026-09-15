#!/usr/bin/env node
// features/chat/background-tasks.js 的纯逻辑单测（vm 拼接执行，与
// chat_input_limit_logic.test.js 同约定）。
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'background-tasks.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');
const context = {};
vm.createContext(context);
vm.runInContext(
  `${code}\nthis.deriveRunningShellTasks = deriveRunningShellTasks; this.formatElapsedMs = formatElapsedMs; this.tailOutputLines = tailOutputLines; this.COMMAND_SUMMARY_MAX = COMMAND_SUMMARY_MAX;`,
  context,
  { filename: logicPath },
);

const { COMMAND_SUMMARY_MAX, deriveRunningShellTasks, formatElapsedMs, tailOutputLines } = context;

// deriveRunningShellTasks：只保留带后台标记（background / shellSnapshot）、
// taskId 且 running 的 tool 卡
const items = [
  // 轮询补建的 shell 快照卡（子 agent detached job）
  { type: 'tool', toolId: 'shell-task:job-1', name: 'exec_shell', taskId: 'job-1', sessionId: 's1', state: 'running', shellSnapshot: true, args: { command: 'cargo build' }, elapsedMs: 61000, output: 'line1\nline2' },
  // 被标记为后台的工具卡
  { type: 'tool', toolId: 'tool-2', name: 'exec_shell', taskId: 'job-2', sessionId: 's1', state: 'running', background: true, args: { command: 'npm run dev' }, elapsedMs: 5000 },
  // 已完成 → 排除
  { type: 'tool', toolId: 'shell-task:job-3', name: 'exec_shell', taskId: 'job-3', sessionId: 's1', state: 'done', shellSnapshot: true, args: { command: 'ls' } },
  // 失败 → 排除
  { type: 'tool', toolId: 'shell-task:job-4', name: 'exec_shell', taskId: 'job-4', sessionId: 's1', state: 'failed', shellSnapshot: true, args: { command: 'make' } },
  // 无 taskId 的前台工具卡 → 排除
  { type: 'tool', toolId: 'tool-5', name: 'exec_shell', state: 'running', args: { command: 'pwd' } },
  // 前台工具卡被轮询的命令匹配回退挂上 taskId（无后台标记）→ 排除，
  // 不能把前台命令误标成"后台任务"
  { type: 'tool', toolId: 'tool-5b', name: 'exec_shell', taskId: 'job-5', sessionId: 's1', state: 'running', args: { command: 'sleep 30' } },
  // 非 tool 条目 → 排除
  { type: 'user', text: 'hi' },
  null,
  undefined,
];
const tasks = deriveRunningShellTasks(items);
assert.deepStrictEqual(tasks.map(t => t.taskId), ['job-1', 'job-2']);
assert.strictEqual(tasks[0].command, 'cargo build');
assert.strictEqual(tasks[0].elapsedMs, 61000);
assert.strictEqual(tasks[0].sessionId, 's1');
assert.strictEqual(tasks[1].output, '');

// 非数组输入安全返回空（vm realm 的数组原型不同，用 JSON 比较）
assert.strictEqual(JSON.stringify(deriveRunningShellTasks(null)), '[]');
assert.strictEqual(JSON.stringify(deriveRunningShellTasks()), '[]');

// 同一 taskId 的重复卡片（轮询快照卡 + tool_end 快速通道打标的原卡）只计一次，
// 先出现的原卡（真实命令参数）优先
const duplicated = deriveRunningShellTasks([
  { type: 'tool', toolId: 'tool-dup-real', name: 'exec_shell', taskId: 'job-dup', sessionId: 's1', state: 'running', background: true, args: { command: 'real card' }, elapsedMs: 1000 },
  { type: 'tool', toolId: 'shell-task:job-dup', name: 'exec_shell', taskId: 'job-dup', sessionId: 's1', state: 'running', shellSnapshot: true, args: { command: 'synthetic card' }, elapsedMs: 900 },
]);
assert.strictEqual(duplicated.length, 1);
assert.strictEqual(duplicated[0].command, 'real card');
assert.strictEqual(duplicated[0].taskId, 'job-dup');

// 超长命令截断
const longCommand = 'x'.repeat(COMMAND_SUMMARY_MAX + 10);
const [truncated] = deriveRunningShellTasks([
  { type: 'tool', taskId: 'job-9', sessionId: 's1', state: 'running', shellSnapshot: true, args: { command: longCommand } },
]);
assert.strictEqual(truncated.command.length, COMMAND_SUMMARY_MAX + 1);
assert.ok(truncated.command.endsWith('…'));

// formatElapsedMs
assert.strictEqual(formatElapsedMs(0), '0s');
assert.strictEqual(formatElapsedMs(999), '0s');
assert.strictEqual(formatElapsedMs(61000), '1m 1s');
assert.strictEqual(formatElapsedMs(3723000), '1h 2m');
assert.strictEqual(formatElapsedMs(NaN), '0s');

// tailOutputLines：取最后 n 行、忽略空行
assert.strictEqual(tailOutputLines('a\nb\nc\nd', 3), 'b\nc\nd');
assert.strictEqual(tailOutputLines('a\n\nb\n', 3), 'a\nb');
assert.strictEqual(tailOutputLines('', 3), '');
assert.strictEqual(tailOutputLines(null, 3), '');
assert.strictEqual(tailOutputLines('only', 3), 'only');

console.log('background_tasks_logic.test.js: all assertions passed');
