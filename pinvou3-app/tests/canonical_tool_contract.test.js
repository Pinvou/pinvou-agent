import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BUNDLE = path.join(ROOT, 'src-tauri', 'resources', 'common', 'bundle');
// 退役工具名不得出现在运行时指导中：v0.9.12 文件/shell 模型可见面是
// read/write/edit/list_dir/file_search/grep_files/bash，旧名（write_file/exec_shell 等）与隐藏
// replay 别名（work_update/update_plan/checklist_write）都不该被教给模型。
// 唯一例外是 Windows 没有 terminal session 时的后台预览控制：lowercase
// `bash` 是 foreground-only，底座明确保留 hidden `Bash` 作为 job control surface。
const RETIRED = /\b(read_file|write_file|edit_file|exec_shell|exec_shell_wait|task_shell_start|web_search|fetch_url|checklist_write|work_update|update_plan|apply_patch)\b/;

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

test('runtime guidance does not teach retired or hidden replay tool names', () => {
  const leaks = [];
  const previewControlExceptions = [];
  for (const file of runtimeGuidanceFiles(BUNDLE)) {
    const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/);
    lines.forEach((line, index) => {
      const relative = path.relative(ROOT, file);
      const windowsPreviewControl = relative === 'src-tauri/resources/common/bundle/instructions-work.md'
        && line.includes('retained compatibility control surface')
        && line.includes('Bash(action="run", command="...", background=true)')
        && line.includes('Bash(action="cancel", task_id="...")')
        && (line.match(/Bash\(action=/g) || []).length === 2;
      if (windowsPreviewControl) previewControlExceptions.push(`${relative}:${index + 1}`);
      if (RETIRED.test(line) || line.includes('File(action=') || (line.includes('Bash(action=') && !windowsPreviewControl)) {
        leaks.push(`${path.relative(ROOT, file)}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  assert.deepEqual(leaks, []);
  assert.deepEqual(previewControlExceptions, [
    'src-tauri/resources/common/bundle/instructions-work.md:14',
  ], 'the hidden Bash exception must remain narrow and Windows-preview-only');
});

test('runtime guidance teaches the canonical model-visible tool families', () => {
  const rendered = runtimeGuidanceFiles(BUNDLE)
    .map((file) => fs.readFileSync(file, 'utf8'))
    .join('\n');
  for (const canonical of [
    'read(path=',
    'write(path=',
    'file_search(query=',
    'bash(command=',
    'terminal/run',
    'todo_write',
  ]) {
    assert.ok(rendered.includes(canonical), `canonical guidance missing: ${canonical}`);
  }
});
