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
const { ScheduledTaskToolCard } = await vite.ssrLoadModule('/src/features/tools/tool-renderers.jsx');
const { parseScheduledTaskToolOutput } = await vite.ssrLoadModule('/src/features/tools/scheduled-task-tool-logic.js');
const { unwrapMcpTextEnvelope } = await vite.ssrLoadModule('/src/features/tools/tool-common.jsx');

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
