/**
 * app-automations scheduled-task tool card logic (E1, execution visibility):
 * - toolSummary wiring exists for the create/list tool names (the timeline
 *   header must show name + rrule without opening the card);
 * - the create output parser maps the server's three payload shapes
 *   (created / pending / failed) and degrades to null on anything unparseable
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
  SCHEDULED_TASK_LIST_TOOL,
  parseScheduledTaskCreateOutput,
  scheduledTaskCreateSummary,
  scheduledTaskListSummary,
  scheduledTaskPromptExcerpt,
} from '../src/features/tools/scheduled-task-tool-logic.js';

const read = rel => readFileSync(new URL(`../src/${rel}`, import.meta.url), 'utf8');

test('create summary shows task name and rrule', () => {
  assert.equal(
    scheduledTaskCreateSummary({ name: 'AI 早报', rrule: 'FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30' }),
    '「AI 早报」 · FREQ=WEEKLY;BYDAY=MO;BYHOUR=8;BYMINUTE=30',
  );
  assert.equal(scheduledTaskCreateSummary({ name: '  仅名字  ' }), '仅名字');
  assert.equal(scheduledTaskCreateSummary({}), '');
  assert.equal(scheduledTaskCreateSummary(null), '');
});

test('list summary names the tool with optional limit', () => {
  assert.equal(scheduledTaskListSummary({ limit: 5 }), 'list · limit 5');
  assert.equal(scheduledTaskListSummary(undefined), 'list');
});

test('create output parser maps the three payload shapes', () => {
  assert.deepEqual(
    parseScheduledTaskCreateOutput(JSON.stringify({ ok: true, taskId: 't-1', taskName: '早报' })),
    { kind: 'created', taskId: 't-1', taskName: '早报' },
  );
  assert.deepEqual(
    parseScheduledTaskCreateOutput(JSON.stringify({ ok: true, taskId: null, delivery: 'pending', taskName: '早报' })),
    { kind: 'pending', taskName: '早报' },
  );
  assert.deepEqual(
    parseScheduledTaskCreateOutput(JSON.stringify({ ok: false, error: 'invalid rrule' })),
    { kind: 'failed', error: 'invalid rrule' },
  );
});

test('create output parser degrades to null on drift', () => {
  assert.equal(parseScheduledTaskCreateOutput('not json'), null);
  assert.equal(parseScheduledTaskCreateOutput(''), null);
  assert.equal(parseScheduledTaskCreateOutput(null), null);
  assert.equal(parseScheduledTaskCreateOutput(JSON.stringify({ hello: 1 })), null);
  // ok:true without taskId and without delivery:"pending" is not a shape the
  // server ever produces — the card must not guess.
  assert.equal(parseScheduledTaskCreateOutput(JSON.stringify({ ok: true })), null);
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
  const renderers = read('features/tools/tool-renderers.jsx');
  assert.match(renderers, /ScheduledTaskCreateCard/, 'the create card must be defined');
  assert.match(renderers, /data-testid="scheduled-task-create-card"/, 'the card must be testable');
  assert.match(
    renderers,
    /import \{[\s\S]*SCHEDULED_TASK_CREATE_TOOL[\s\S]*\} from '\.\/scheduled-task-tool-logic\.js'/,
    'the renderer must import the shared tool names (no literal drift)',
  );
  // The L1 create tool must never join the quiet-tool set (execution
  // visibility is mandatory).
  const quietList = common.slice(common.indexOf('QUIET_TOOLS'), common.indexOf(']);'));
  assert.ok(!quietList.includes('app-automations'), 'the create tool must not be a quiet tool');
});

test('tool names match the manifest registration', () => {
  const manifest = JSON.parse(
    readFileSync(new URL('../resources/mcp-servers/app-automations/manifest.json', import.meta.url), 'utf8'),
  );
  assert.ok(manifest.mcp_tools.includes(SCHEDULED_TASK_CREATE_TOOL));
  assert.ok(manifest.mcp_tools.includes(SCHEDULED_TASK_LIST_TOOL));
});
