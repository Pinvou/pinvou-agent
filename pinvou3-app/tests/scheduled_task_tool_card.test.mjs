/**
 * app-automations scheduled-task tool card logic (E1, execution visibility):
 * - toolSummary wiring exists for the create/update/delete/list tool names
 *   (the timeline header must show name + rrule / target id without opening
 *   the card);
 * - the output parser maps the server's payload shapes (created / updated /
 *   deleted / pending / duplicate) and degrades to null on anything unparseable
 *   (drift defense — the card must never take over on unknown output);
 * - the prompt excerpt bounds a 32k prompt to a timeline-safe prefix.
 *
 * The renderer wiring itself is pinned by a source-shape check on
 * tool-renderers.jsx (the card is JSX; logic lives in the pure module).
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
  SCHEDULED_TASK_CREATE_TOOL,
  SCHEDULED_TASK_DELETE_TOOL,
  SCHEDULED_TASK_LIST_TOOL,
  SCHEDULED_TASK_READ_TOOL,
  SCHEDULED_TASK_UPDATE_TOOL,
  scheduledTaskCreateSummary,
  scheduledTaskDeleteSummary,
  scheduledTaskListSummary,
  scheduledTaskUpdateSummary,
} from '../src/features/tools/scheduled-task-tool-logic.js';
import {
  parseScheduledTaskToolOutput,
  scheduledTaskPromptExcerpt,
} from '../src/features/tools/scheduled-task-card-logic.js';

const read = rel => readFileSync(new URL(`../src/${rel}`, import.meta.url), 'utf8');

// Round-8 minor 14: the mismatch and updatedName rows are pinned by
// testid at the card source (deleting either div left the suite
// green), the card uses the localized copy, and the surrogate back-off is
// pinned by execution. The card source is the lazy scheduled-task chunk's
// module (features/tools/scheduled-task-card.jsx); the startup entry only
// routes to it (pinned in the renderer wiring test below).
const cardSource = readFileSync(
  new URL('../src/features/tools/scheduled-task-card.jsx', import.meta.url),
  'utf8',
);
assert.match(cardSource, /data-testid="scheduled-task-mismatch-note"/, 'the mismatch row renders');
// Round-15 REQUIRED-5: the production call site parses THROUGH the unwrap —
// deleting unwrapMcpTextEnvelope from this call re-dead-cards the feature
// while every composed-in-test fixture stays green.
assert.match(
  cardSource,
  /parseScheduledTaskToolOutput\(unwrapMcpTextEnvelope\(out\)\)/,
  'the production call must unwrap the MCP envelope before parsing',
);
// A parse miss must keep the old fall-through behavior (structural drift
// renders the default raw output view, not a broken or empty card).
assert.match(
  cardSource,
  /if \(!parsed\) return fallback;/,
  'a parse miss must render the caller-provided raw-output fallback',
);
assert.match(cardSource, /data-testid="scheduled-task-updated-name"/, 'the updatedName row renders');
assert.match(cardSource, /\{copy\.mismatchNote\}/, 'the card uses the localized copy, not the server note');
const clippedSurrogate = scheduledTaskPromptExcerpt({ prompt: '\u{1F600}'.repeat(200) });
assert.ok(!clippedSurrogate.includes('\uFFFD'), 'no replacement char at the clip boundary');

test('create summary shows task name and rrule with localized quoting', () => {
  // Round-6 minor 9: the quoting is localized copy — the summary composes
  // the caller's formatter; the default is bare (no hardcoded brackets).
  assert.equal(
    scheduledTaskCreateSummary({ name: 'AI 早报', rrule: 'FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30' }, n=>`「${n}」`),
    '「AI 早报」 · FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30',
  );
  assert.equal(
    scheduledTaskCreateSummary({ name: 'Daily brief', rrule: 'FREQ=DAILY' }, n=>`“${n}”`),
    '“Daily brief” · FREQ=DAILY',
  );
  assert.equal(scheduledTaskCreateSummary({ name: '  仅名字  ' }), '仅名字');
  assert.equal(scheduledTaskCreateSummary({}), '');
  assert.equal(scheduledTaskCreateSummary(null), '');
});

test('update summary shows target id and changed fields', () => {
  assert.equal(
    scheduledTaskUpdateSummary({ task_id: 't-1', name: '新名', paused: true }),
    't-1 · name/paused',
  );
  assert.equal(scheduledTaskUpdateSummary({ task_id: 't-1' }), 't-1');
  assert.equal(scheduledTaskUpdateSummary({}), '');
  assert.equal(scheduledTaskUpdateSummary(null), '');
});

test('delete summary shows the target id', () => {
  assert.equal(scheduledTaskDeleteSummary({ task_id: 't-9' }), 't-9');
  assert.equal(scheduledTaskDeleteSummary({}), '');
});

test('list summary names the tool with the localized verb and optional localized limit', () => {
  // The limit suffix is localized copy (review R4 minor: no hardcoded
  // English token) — the summary composes the caller's formatter.
  assert.equal(scheduledTaskListSummary({ limit: 5 }, 'list', n=>` · limit ${n}`), 'list · limit 5');
  assert.equal(scheduledTaskListSummary({ limit: 5 }, '列表', n=>` · 限 ${n} 条`), '列表 · 限 5 条');
  assert.equal(scheduledTaskListSummary({}, 'list', n=>` · limit ${n}`), 'list');
});

test('output parser maps the CRUD payload shapes', () => {
  assert.deepEqual(
    parseScheduledTaskToolOutput(JSON.stringify({ ok: true, kind: 'create', taskId: 't-1', taskName: '早报' })),
    { kind: 'created', taskId: 't-1', taskName: '早报', duplicate: false, payloadMismatch: false, mismatchNote: '' },
  );
  assert.deepEqual(
    parseScheduledTaskToolOutput(JSON.stringify({ ok: true, kind: 'update', taskId: 't-1', taskName: '早报' })),
    { kind: 'updated', taskId: 't-1', taskName: '早报', duplicate: false, payloadMismatch: false, mismatchNote: '' },
  );
  assert.deepEqual(
    parseScheduledTaskToolOutput(JSON.stringify({ ok: true, kind: 'delete', taskId: 't-1', taskName: '早报' })),
    { kind: 'deleted', taskId: 't-1', taskName: '早报', duplicate: false, payloadMismatch: false, mismatchNote: '' },
  );
  assert.deepEqual(
    parseScheduledTaskToolOutput(JSON.stringify({ ok: true, kind: 'create', taskId: null, delivery: 'pending', taskName: '早报' })),
    { kind: 'pending', taskName: '早报' },
  );
  // Idempotent replay: the recorded result is surfaced as a duplicate so the
  // card does not claim a fresh apply.
  assert.deepEqual(
    parseScheduledTaskToolOutput(JSON.stringify({ ok: true, kind: 'update', taskId: 't-1', taskName: '早报', duplicate: true })),
    { kind: 'updated', taskId: 't-1', taskName: '早报', duplicate: true, payloadMismatch: false, mismatchNote: '' },
  );
});

test('output parser degrades to null on drift', () => {
  assert.equal(parseScheduledTaskToolOutput('not json'), null);
  assert.equal(parseScheduledTaskToolOutput(''), null);
  assert.equal(parseScheduledTaskToolOutput(null), null);
  assert.equal(parseScheduledTaskToolOutput(JSON.stringify({ hello: 1 })), null);
  // ok:true without taskId and without delivery:"pending" is not a shape the
  // server ever produces — the card must not guess.
  assert.equal(parseScheduledTaskToolOutput(JSON.stringify({ ok: true })), null);
  // An unknown kind is drift: never render it as an affirmative "created".
  assert.equal(parseScheduledTaskToolOutput(JSON.stringify({ ok: true, kind: 'mystery', taskId: 't-1' })), null);
  // Failures flow through the is_error path (OutputError), never this card.
  assert.equal(parseScheduledTaskToolOutput(JSON.stringify({ ok: false, error: 'invalid rrule' })), null);
});

test('prompt excerpt bounds long prompts', () => {
  assert.equal(scheduledTaskPromptExcerpt({ prompt: '  汇总新闻  ' }), '汇总新闻');
  assert.equal(scheduledTaskPromptExcerpt({}), '');
  const long = 'x'.repeat(500);
  const excerpt = scheduledTaskPromptExcerpt({ prompt: long });
  assert.ok(excerpt.length < 500, 'the excerpt must be clipped');
  assert.ok(excerpt.endsWith('…'), 'the excerpt marks the cut');
});

test('renderer wiring is present (timeline card + summary routing)', () => {
  const common = read('features/tools/tool-common.jsx');
  assert.match(
    common,
    new RegExp(`case ${'SCHEDULED_TASK_CREATE_TOOL'}:`),
    'toolSummary must route the create tool through the logic module',
  );
  assert.match(
    common,
    new RegExp(`case ${'SCHEDULED_TASK_UPDATE_TOOL'}:`),
    'toolSummary must route the update tool through the logic module',
  );
  assert.match(
    common,
    new RegExp(`case ${'SCHEDULED_TASK_DELETE_TOOL'}:`),
    'toolSummary must route the delete tool through the logic module',
  );
  const renderers = read('features/tools/tool-renderers.jsx');
  assert.match(renderers, /SCHEDULED_TASK_TOOL_OPS = \{[\s\S]*SCHEDULED_TASK_CREATE_TOOL[\s\S]*SCHEDULED_TASK_UPDATE_TOOL[\s\S]*SCHEDULED_TASK_DELETE_TOOL/,
    'create/update/delete must route to the result card');
  // Startup-budget lazy route: the card and its parser live in the lazy
  // scheduled-task chunk, and the entry routes done items through the
  // dynamic import below (re-inlining the card re-trips the main-entry
  // budget; this pin catches the route disappearing).
  assert.match(
    renderers,
    /import\('\.\/scheduled-task-card\.jsx'\)/,
    'the renderer must route done items through the lazy card chunk',
  );
  const cardModule = read('features/tools/scheduled-task-card.jsx');
  assert.match(cardModule, /ScheduledTaskToolCard/, 'the result card must be defined');
  assert.match(cardModule, /data-testid="scheduled-task-tool-card"/, 'the card must be testable');
  const cardLogic = read('features/tools/scheduled-task-card-logic.js');
  assert.match(cardLogic, /export const parseScheduledTaskToolOutput/, 'the parser must live in the lazy card logic module');
  // The L1 write tools must never join the quiet-tool set (execution
  // visibility is mandatory).
  const quietList = common.slice(
    common.indexOf('QUIET_TOOLS'),
    common.indexOf(']);', common.indexOf('QUIET_TOOLS')),
  );
  assert.ok(!quietList.includes('app-automations'), 'the write tools must not be quiet tools');
});

test('tool names match the manifest registration', () => {
  const manifest = JSON.parse(
    readFileSync(new URL('../resources/mcp-servers/app-automations/manifest.json', import.meta.url), 'utf8'),
  );
  for (const name of [
    SCHEDULED_TASK_CREATE_TOOL,
    SCHEDULED_TASK_READ_TOOL,
    SCHEDULED_TASK_LIST_TOOL,
    SCHEDULED_TASK_UPDATE_TOOL,
    SCHEDULED_TASK_DELETE_TOOL,
  ]) {
    assert.ok(manifest.mcp_tools.includes(name), `${name} must be registered`);
  }
});
