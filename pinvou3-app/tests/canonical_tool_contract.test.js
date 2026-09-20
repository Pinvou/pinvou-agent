import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
// Both bundle/ and skill-marketplace/ ship model-facing guidance; a retired
// name is equally teachable from either tree, so scan all of resources/common.
const RESOURCES_COMMON = path.join(ROOT, 'src-tauri', 'resources', 'common');
const BUNDLE = path.join(RESOURCES_COMMON, 'bundle');
// Retired tool names must not leak into runtime guidance: the v0.9.12
// model-visible file/shell surface is read/write/edit/list_dir/file_search/
// grep_files/bash. Old names (write_file/exec_shell, ...) and hidden replay
// aliases (work_update/update_plan/checklist_write) must never be taught.
// The hidden `Bash`/`File` tools exist only to replay saved v0.9.x
// transcripts and are invisible to new model catalogs, so no runtime
// guidance may advertise a `Bash(action=...)`/`File(action=...)` control
// surface; the Windows loopback preview degrades honestly via
// `terminal/run` or the artifact card instead.
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
  // Guard against a vacuous green: if either shipped subtree disappears or
  // is renamed, the scan must fail loudly instead of silently covering less.
  const bundleFiles = runtimeGuidanceFiles(BUNDLE);
  const marketplaceFiles = runtimeGuidanceFiles(
    path.join(RESOURCES_COMMON, 'skill-marketplace'),
  );
  assert.ok(
    bundleFiles.length > 0,
    'resources/common/bundle contributed no files to the scan',
  );
  assert.ok(
    marketplaceFiles.length > 0,
    'resources/common/skill-marketplace contributed no files to the scan',
  );
  const leaks = [];
  for (const file of runtimeGuidanceFiles(RESOURCES_COMMON)) {
    const lines = fs.readFileSync(file, 'utf8').split(/\r?\n/);
    lines.forEach((line, index) => {
      // Normalize separators: path.relative yields backslashes on Windows,
      // which could never match the forward-slash exception key below (the
      // exception silently degraded into a leak on Windows hosts).
      const relative = path.relative(ROOT, file).split(path.sep).join('/');
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
