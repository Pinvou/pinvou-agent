import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BUNDLE = path.join(ROOT, 'src-tauri', 'resources', 'common', 'bundle');
const WORKFLOW_SOURCE = path.resolve(ROOT, '..', 'workflows', 'sansheng-liubu');
const RETIRED = /\b(read_file|write_file|edit_file|list_dir|file_search|grep_files|exec_shell|exec_shell_wait|web_search|fetch_url|checklist_write)\b/;

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
  for (const file of [BUNDLE, WORKFLOW_SOURCE].flatMap(runtimeGuidanceFiles)) {
    const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/);
    lines.forEach((line, index) => {
      if (RETIRED.test(line)) {
        leaks.push(`${path.relative(ROOT, file)}:${index + 1}: ${line.trim()}`);
      }
    });
  }
  assert.deepEqual(leaks, []);
});
