#!/usr/bin/env node
// ScheduledTaskToolCard render fixture (round-9 BLOCKER 1): the production
// tool-result text carries the serialized MCP envelope
// {"content":[{"type":"text","text":"{...}"}],"isError":false} — the card's
// parse path must unwrap first (the round-8 shape parsed the raw output and
// returned null for EVERY successful call, dead-carding the feature), and
// the rendered card must show the payload's rows (renderToStaticMarkup, the
// stock-quote fixture's approach).
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const hadWindow = Object.prototype.hasOwnProperty.call(globalThis, 'window');
globalThis.window = globalThis.window || { TauriBridge: undefined };

const vite = await createServer({
  configFile: false,
  root: fileURLToPath(new URL('..', import.meta.url)),
  logLevel: 'error',
  server: { middlewareMode: true, watch: null },
  optimizeDeps: { noDiscovery: true },
});
const { ScheduledTaskToolCard } = await vite.ssrLoadModule('/src/features/tools/scheduled-task-card.jsx');
const { ScheduledTaskToolResult } = await vite.ssrLoadModule('/src/features/tools/scheduled-task-card.jsx');
const { parseScheduledTaskToolOutput } = await vite.ssrLoadModule('/src/features/tools/scheduled-task-card-logic.js');
const { unwrapMcpTextEnvelope } = await vite.ssrLoadModule('/src/features/tools/tool-common.jsx');
const { OutputPre } = await vite.ssrLoadModule('/src/features/tools/tool-common.jsx');

after(async () => {
  await vite.close();
  if (!hadWindow) delete globalThis.window;
});

const envelope = payload =>
  JSON.stringify({ content: [{ type: 'text', text: JSON.stringify(payload) }], isError: false });

const t = {
  uiScheduledTaskTool: {
    listLabel: 'List', duplicateNote: 'Duplicate', mismatchNote: 'Different payload; not applied.',
    created: 'Created', updated: 'Updated', deleted: 'Deleted', deletedNote: 'Archived',
    pending: 'Pending', promptLabel: 'Prompt', updatedName: 'Renamed',
  },
};

test('the production MCP envelope parses after the unwrap (round-9 BLOCKER 1)', () => {
  const payload = { ok: true, kind: 'create', taskId: 't-1', taskName: '早报', duplicate: false };
  assert.equal(parseScheduledTaskToolOutput(envelope(payload)), null, 'fixture guard: the raw envelope does NOT parse');
  const parsed = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(envelope(payload)));
  assert.ok(parsed, 'the unwrapped envelope parses');
  assert.equal(parsed.taskName, '早报');
  assert.equal(parsed.kind, 'created');
});

test('the created card renders the payload rows', () => {
  const parsed = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(envelope({
    ok: true, kind: 'create', taskId: 't-1', taskName: '早报', duplicate: false,
  })));
  const html = renderToStaticMarkup(React.createElement(ScheduledTaskToolCard, {
    op: 'create', parsed, args: { name: '早报', prompt: '汇总今天新闻', rrule: 'FREQ=DAILY;BYHOUR=8' }, t,
  }));
  assert.ok(html.includes('data-testid="scheduled-task-tool-card"'), 'the card shell renders');
  assert.ok(html.includes('t-1'), 'the created row names the task id');
  assert.ok(html.includes('FREQ=DAILY;BYHOUR=8'), 'the rrule row renders');
  assert.ok(html.includes('汇总今天新闻'), 'the prompt excerpt renders');
});

test('the mismatch row renders the localized note and the updatedName row', () => {
  const parsed = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(envelope({
    ok: true, kind: 'update', taskId: 't-1', taskName: '新名', duplicate: true, payload_mismatch: true,
  })));
  const html = renderToStaticMarkup(React.createElement(ScheduledTaskToolCard, {
    op: 'update', parsed, args: { name: '新名' }, t,
  }));
  assert.ok(html.includes('data-testid="scheduled-task-mismatch-note"'), 'the mismatch row renders');
  assert.ok(html.includes('Different payload; not applied.'), 'the LOCALIZED copy renders, not the server note');
  assert.ok(html.includes('data-testid="scheduled-task-updated-name"'), 'the updatedName row renders');
});

test('the pending and duplicate rows render', () => {
  const pending = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(envelope({
    ok: true, sessionId: null, taskId: null, taskName: null, delivery: 'pending', duplicate: false,
  })));
  assert.equal(pending.kind, 'pending');
  const html = renderToStaticMarkup(React.createElement(ScheduledTaskToolCard, {
    op: 'create', parsed: pending, args: {}, t,
  }));
  assert.ok(html.includes('Pending'), 'the pending row renders');
  // Round-31 MAJOR-2: the duplicate-replay row (the only UI trace of the
  // "recorded result" contract) was deletable invisibly — the old fixture
  // asserted Pending on duplicate:false only. A duplicate:true replay must
  // surface the localized duplicate row.
  const replay = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(envelope({
    ok: true, kind: 'create', taskId: 't-1', taskName: '早报', duplicate: true,
  })));
  const replayHtml = renderToStaticMarkup(React.createElement(ScheduledTaskToolCard, {
    op: 'create', parsed: replay, args: { name: '早报' }, t,
  }));
  assert.ok(replayHtml.includes('Duplicate'), 'the duplicate row renders for an idempotent replay');
});

test('a name-less update does not claim "Renamed" (round-10 MAJOR-2)', () => {
  // The watcher writes task_name into the marker on EVERY successful update,
  // so the row must key on the REQUEST actually carrying a name — otherwise
  // a paused/rrule-only update renders a rename that never happened.
  const parsed = parseScheduledTaskToolOutput(unwrapMcpTextEnvelope(envelope({
    ok: true, kind: 'update', taskId: 't-1', taskName: '早报', duplicate: false,
  })));
  const html = renderToStaticMarkup(React.createElement(ScheduledTaskToolCard, {
    op: 'update', parsed, args: { paused: true }, t,
  }));
  assert.ok(!html.includes('data-testid="scheduled-task-updated-name"'), 'no rename claim without a name in the request');
  assert.ok(html.includes('t-1'), 'the updated row still names the task id');
  // A blank/whitespace name is equally name-less.
  const blank = renderToStaticMarkup(React.createElement(ScheduledTaskToolCard, {
    op: 'update', parsed, args: { name: '   ' }, t,
  }));
  assert.ok(!blank.includes('data-testid="scheduled-task-updated-name"'), 'a blank name is not a rename');
});

test('the real result branch renders the deleted row through unwrap+parse (round-26 M7)', () => {
  // The production branch is ScheduledTaskToolResult — the component
  // tool-renderers' lazy resolves to — not the hand-composed inner card the
  // older fixtures rendered. Masking the unwrap call or corrupting the
  // deleted-arm copy used to keep every suite green; this drives the real
  // branch with the exact props the production composition passes
  // (op/out/args/t/fallback). The lazy+Suspense layer itself cannot render
  // through renderToStaticMarkup (verified: the boundary catches the
  // suspension and renders its fallback), so the branch is rendered at its
  // synchronous seam with the same component identity.
  const html = renderToStaticMarkup(React.createElement(ScheduledTaskToolResult, {
    op: 'deleted',
    out: envelope({ ok: true, kind: 'delete', taskId: 't-9', taskName: '早报', duplicate: false }),
    args: { name: '早报' },
    t,
    fallback: React.createElement(OutputPre, { text: 'RAW-ENVELOPE-MARKER' }),
  }));
  assert.ok(html.includes('Archived'), 'the deletedNote row renders through the real branch');
  assert.ok(html.includes('Deleted'), 'the deleted heading renders');
  assert.ok(html.includes('t-9'), 'the deleted row names the task id');
  assert.ok(!html.includes('RAW-ENVELOPE-MARKER'), 'a parse hit must not fall back');
});

test('a parse miss renders the production fallback prop (round-26 M7)', () => {
  // The `fallback={<OutputPre text={out} />}` prop is what structural
  // drift degrades to; it is pinned at both ends — the composition site
  // must pass it (the shape pin in scheduled_task_tool_card.test.mjs,
  // round-26 M7) and, when passed, it must actually render on a parse
  // miss (this test).
  const html = renderToStaticMarkup(React.createElement(ScheduledTaskToolResult, {
    op: 'deleted',
    out: 'total drift, never parses',
    args: {},
    t,
    fallback: React.createElement(OutputPre, { text: 'RAW-ENVELOPE-MARKER' }),
  }));
  assert.ok(html.includes('RAW-ENVELOPE-MARKER'), 'a parse miss renders the production fallback prop');
  assert.ok(!html.includes('scheduled-task-tool-card'), 'a parse miss must not render the card');
});
