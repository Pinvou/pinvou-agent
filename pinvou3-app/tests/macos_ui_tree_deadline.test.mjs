#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(
  path.join(here, '..', 'src-tauri', 'src', 'features', 'computer_use', 'platform', 'macos.rs'),
  'utf8',
);

// The ui_tree deadline is a headline Loosened value (10s → 20s, Linux
// `A11Y_TREE_DEADLINE` parity). The Rust-side test builds its deadline FROM
// the const, so a silent revert to 10 ships green there; this source pin
// holds the literal value itself (the const lives in a macOS-gated module,
// so a Rust assert would never execute on Linux CI).
const definition = source.split('\n').find((line) => line.includes('const AX_TREE_TIME_LIMIT'));
assert.ok(definition, 'AX_TREE_TIME_LIMIT definition is missing');
assert.match(
  definition,
  /const AX_TREE_TIME_LIMIT: Duration = Duration::from_secs\(20\);/,
  'the ui_tree deadline must stay at the shipped 20s',
);

// The deadline must stay wired into the real traversal request, not just defined.
assert.match(source, /deadline: Instant::now\(\) \+ AX_TREE_TIME_LIMIT/);
assert.ok(
  source.split('deadline: Instant::now() + AX_TREE_TIME_LIMIT').length - 1 >= 2,
  'the deadline must cover both the production request and the test request sites',
);
