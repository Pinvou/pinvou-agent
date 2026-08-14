import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BUNDLE = path.join(ROOT, 'src-tauri', 'resources', 'common', 'bundle');
// 退役工具名不得出现在运行时指导中：v0.9.5 模型可见面是 canonical 家族
// （File/Bash/Web/todo_write/Git），旧名（write_file/exec_shell 等）与隐藏
// replay 别名（work_update/update_plan/checklist_write）都不该被教给模型。
const RETIRED = /\b(read_file|write_file|edit_file|list_dir|file_search|grep_files|exec_shell|exec_shell_wait|task_shell_start|web_search|fetch_url|checklist_write|work_update|update_plan|apply_patch)\b/;

function runtimeGuidanceFiles(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const absolute = path.join(dir, entry.name);
    if (entry.isDirectory()) return runtimeGuidanceFiles(absolute);
    if (!entry.isFile() || !/\.(md|json)$/i.test(entry.name) || /^NOTICE/i.test(entry.name)) {
      return [];
    }
    return [absolute];
  });
}

test('runtime guidance does not teach retired model-visible tool names', () => {
  const leaks = [];
  for (const file of runtimeGuidanceFiles(BUNDLE)) {
    const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/);
    lines.forEach((line, index) => {
      if (RETIRED.test(line)) {
        leaks.push(`${path.relative(ROOT, file)}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  assert.deepEqual(leaks, []);
});

test('runtime guidance teaches the canonical model-visible tool families', () => {
  const rendered = runtimeGuidanceFiles(BUNDLE)
    .map((file) => fs.readFileSync(file, 'utf8'))
    .join('\n');
  for (const canonical of ['File(action=', 'Bash(action=', 'todo_write']) {
    assert.ok(rendered.includes(canonical), `canonical guidance missing: ${canonical}`);
  }
});
