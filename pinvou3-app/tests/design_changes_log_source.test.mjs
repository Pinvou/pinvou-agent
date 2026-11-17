#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = readFileSync(
  path.join(here, '..', 'src', 'features', 'artifacts', 'DesignInspectorPanel.jsx'),
  'utf8',
);

// The r3 wave made the design changes log render every group/item (the old
// `slice(-8)`/`slice(-6)` windows made earlier changes unreachable — header
// count still grew, content scrolled away) and put the full-length values in
// each row's `title`. `design_changes_logic.test.js` pins only the record
// mechanics, so nothing held this rendering (r16 review m3: deleting the
// slices' absence or the title/aria attributes shipped every suite green).
// Pin the shipped shape over the changes-log JSX region, reflow-tolerantly.
const start = source.indexOf('data-testid="design-changes-log"');
assert.ok(start >= 0, 'the changes log region is missing');
const region = source.slice(start, source.indexOf('export { DesignInspectorPanel }'));

// Every group and every item renders — no trailing-window slices.
assert.match(region, /Object\.values\(groupedChanges\)\.map/, 'groups must render in full');
assert.match(region, /group\.items\.map/, 'items must render in full');
assert.doesNotMatch(region, /\.slice\(/, 'the old slice(-8)/slice(-6) windows must not return');

// The toggle stays screen-reader wired to the collapsible body it drives.
assert.match(region, /aria-expanded=\{changesExpanded\}/, 'toggle must expose aria-expanded');
assert.match(region, /aria-controls="design-changes-log-body"/, 'toggle must name its body');
assert.match(region, /id="design-changes-log-body"/, 'collapsible body id must match');

// Full-length values ride in title (CSS truncate alone leaves long edits
// reachable-but-unreadable now the slices are gone).
assert.match(
  region,
  /title=\{`\$\{change\.oldValue \|\| ''\} -> \$\{change\.newValue \|\| ''\}`\}/,
  'each row must carry the full old -> new values in its title',
);
