#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(
  path.join(here, '..', 'src', 'features', 'codex', 'RewindChip.jsx'),
  'utf8',
);

// The rewind dialog's file list is the frontend half of the r2 fix wave:
// the backend changes list is deliberately unbounded (one turn can scaffold
// tens of thousands of entries), so the initial render must stay windowed
// and the expander must grow it in batches — mounting the whole list froze
// the webview. RewindChip has no component-test harness, so this holds the
// shape in source.
assert.match(
  source,
  /const REWIND_FILE_LIST_INITIAL_ROWS = 100;/,
  'the initial bounded window must stay at 100 rows',
);
assert.match(
  source,
  /const REWIND_FILE_LIST_EXPAND_STEP = 200;/,
  'the expand step must stay batched at 200 rows',
);
assert.match(
  source,
  /const visible = changes\.slice\(0, visibleCount\);/,
  'the rendered rows must stay sliced to the bounded window (deleting the slice remounts the whole list)',
);
assert.match(
  source,
  /setVisibleCount\(\(count\) => count \+ REWIND_FILE_LIST_EXPAND_STEP\)/,
  'one expander click must append exactly one batch',
);
