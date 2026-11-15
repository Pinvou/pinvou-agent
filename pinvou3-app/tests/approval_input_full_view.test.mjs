#!/usr/bin/env node
// Approval-card full-input disclosure (review P2: the 400-character input cap
// used to be permanent — neither the ordinary chat approval card nor the
// code-native approval card offered any way to see the rest of a long input,
// so a once/session decision could be made on an unseen prefix). Both input
// views now retain the COMPLETE text/JSON and both cards render it behind a
// disclosure <details> at the truncation point. This fixture pins the runtime
// behavior on both surfaces: the collapsed preview stays a prefix + ellipsis
// with NO tail marker, and the disclosed region carries the untruncated input.
//
// Render harness: renderToStaticMarkup over the real ApprovalCard (the
// scheduled-task render fixture's approach). The native card cannot be
// rendered standalone (not exported, 5k-line module graph), so its surface is
// pinned two ways: the pure nativeApprovalInputView helper is executed from
// source, and the card's disclosure JSX is shape-pinned on comment-stripped
// source (tool_card_keyboard_guard discipline: positive pins must not be
// satisfiable by comments).
import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const hadWindow = Object.prototype.hasOwnProperty.call(globalThis, 'window');
globalThis.window = globalThis.window || { TauriBridge: undefined };

const root = fileURLToPath(new URL('..', import.meta.url));
const read = (...segments) => readFileSync(`${root}/${segments.join('/')}`, 'utf8');

const vite = await createServer({
  configFile: false,
  root,
  logLevel: 'error',
  server: { middlewareMode: true, watch: null },
  optimizeDeps: { noDiscovery: true },
});
const { ApprovalCard } = await vite.ssrLoadModule('/src/features/tools/tool-renderers.jsx');

after(async () => {
  await vite.close();
  if (!hadWindow) delete globalThis.window;
});

// The codex-native copy table (nativeApprovalShowFullInput etc.) lives in the
// component, not in dictEn.ui; the chat card reads the flat keys from t.
const t = {
  approvalTitle: 'Tool call awaiting your approval', approvalIntent: 'Model intent',
  approvalAllow: 'Allow', approvalAllowSession: 'Allow for this session', approvalDeny: 'Deny',
  approvalShowFullInput: 'SHOW-FULL-INPUT-LABEL',
  uiCancelled: 'Cancelled', uiToolRender: { toolUnknown: 'unknown' },
  uiMultiAgent: { webActionHint: 'read-only' },
  approvalAllowed: 'Allowed', approvalAllowedSession: 'Allowed for session',
  approvalDenied: 'Denied', approvalSubmitting: 'Submitting…', approvalOutcomeUnknown: 'unknown outcome',
};

// >400 chars with the marker strictly inside the UNSEEN tail (the preview is
// exactly the first 400 characters + an ellipsis, so a marker starting at
// index 400 can never leak into the collapsed region).
const TAIL_MARKER = 'SUFFIX-MARKER-Z9';
const longString = 'A'.repeat(400) + TAIL_MARKER;
assert.ok(longString.length > 400);
assert.ok(longString.indexOf(TAIL_MARKER) >= 400);

// Long-object case: one scalar value past the 80-char shallow-row cap forces
// the JSON.stringify branch, and its marker also sits past index 400 of the
// serialized text.
const OBJ_MARKER = 'OBJ-TAIL-MARKER-Q5';
const longObject = { body: 'K'.repeat(500) + OBJ_MARKER };
const longObjectText = JSON.stringify(longObject, null, 2);
assert.ok(longObjectText.length > 400);
assert.ok(longObjectText.indexOf(OBJ_MARKER) >= 400);

const renderCard = input => renderToStaticMarkup(React.createElement(ApprovalCard, {
  item: { toolName: 'write_file', input, resolved: false, submitting: false },
  t,
  onDecide: () => {},
}));

// Split the static markup into what the user sees while collapsed (the
// preview + the disclosure summary — a closed <details> shows exactly that)
// and what expanding reveals (the content between </summary> and </details>).
const regions = html => {
  assert.equal((html.match(/<details/g) || []).length, 1, 'exactly one full-input disclosure entry');
  const openAt = html.indexOf('<details');
  const summaryEnd = html.indexOf('</summary>', openAt);
  assert.ok(summaryEnd > openAt, 'the disclosure has a summary');
  const detailsEnd = html.indexOf('</details>', summaryEnd);
  assert.ok(detailsEnd > summaryEnd, 'the disclosure is closed');
  return {
    collapsed: html.slice(0, summaryEnd + '</summary>'.length),
    disclosed: html.slice(summaryEnd + '</summary>'.length, detailsEnd),
  };
};

test('chat card: a >400-char string input keeps the full text behind a disclosure', () => {
  const html = renderCard(longString);
  const { collapsed, disclosed } = regions(html);
  // Expand entry at the truncation point, with the localized label.
  assert.ok(collapsed.includes('<summary'), 'the expand entry renders');
  assert.ok(collapsed.includes('SHOW-FULL-INPUT-LABEL'), 'the expand entry carries the localized label');
  // Collapsed: the truncated prefix only — the unseen tail must not leak.
  assert.ok(collapsed.includes(longString.slice(0, 400)), 'the collapsed preview shows the 400-char prefix');
  assert.ok(collapsed.includes('…'), 'the collapsed preview marks the truncation');
  assert.ok(!collapsed.includes(TAIL_MARKER), 'the collapsed region must NOT contain the unseen tail');
  // Expanded: the complete input, untruncated.
  assert.ok(disclosed.includes(longString), 'the disclosed region carries the FULL input');
  assert.ok(disclosed.includes(TAIL_MARKER), 'expanding reveals the previously unseen tail');
});

test('chat card: a long object input keeps the full JSON behind a disclosure', () => {
  const html = renderCard(longObject);
  const { collapsed, disclosed } = regions(html);
  assert.ok(collapsed.includes('SHOW-FULL-INPUT-LABEL'), 'the expand entry renders for the JSON branch too');
  assert.ok(!collapsed.includes(OBJ_MARKER), 'the collapsed JSON preview must NOT contain the unseen tail');
  // React escapes quotes in text nodes, so pin the escaping-immune payload:
  // the full 500-char value plus its tail marker must be present verbatim.
  assert.ok(disclosed.includes(longObject.body), 'the disclosed region carries the FULL serialized JSON body');
  assert.ok(disclosed.includes(OBJ_MARKER), 'expanding reveals the tail of the serialized JSON');
});

test('chat card: inputs within the cap render verbatim with no expand entry', () => {
  const short = 'rm -rf build/';
  const html = renderCard(short);
  assert.ok(!html.includes('<details'), 'no disclosure for a short input');
  assert.ok(!html.includes('SHOW-FULL-INPUT-LABEL'), 'no expand label for a short input');
  assert.ok(html.includes(short), 'the short input renders verbatim');
  const empty = renderCard(null);
  assert.ok(!empty.includes('<details'), 'no disclosure for a null input');
});

test('both disclosure labels are localized zh/en/ja', () => {
  for (const lang of ['zh', 'en', 'ja']) {
    const copy = read('src', 'shared', 'i18n', `${lang}.js`);
    assert.ok(copy.includes('approvalShowFullInput:'), `${lang}: the chat-card label must exist`);
    assert.ok(copy.includes('nativeApprovalShowFullInput:'), `${lang}: the native-card label must exist`);
  }
});

// ── code-native surface (NativeApprovalCard) ─────────────────────
const stripComments = text => text
  .replace(/\/\*[\s\S]*?\*\//g, comment => comment.replace(/[^\n]/g, ''))
  .split('\n')
  .map(line => line.replace(/\/\/.*$/, ''))
  .join('\n');

const executeNativeInputView = () => {
  const source = read('src', 'features', 'codex', 'CodexAcpView.jsx');
  const start = source.indexOf('function nativeApprovalInputView');
  const end = source.indexOf('function NativeApprovalCard');
  assert.ok(start >= 0 && end > start, 'the native input-view helper must exist before NativeApprovalCard');
  // The helper is pure (no imports, no JSX): execute the real source.
  return new Function(`${source.slice(start, end)}\nreturn nativeApprovalInputView;`)();
};

test('native card: the input view retains the full string/JSON past the cap', () => {
  const view = executeNativeInputView();
  const stringView = view(longString);
  assert.equal(stringView.full, longString, 'the FULL string is retained');
  assert.equal(stringView.pre, `${longString.slice(0, 400)}…`, 'the preview is the truncated prefix');
  assert.ok(!stringView.pre.includes(TAIL_MARKER), 'the preview must NOT contain the unseen tail');

  const jsonObject = { body: 'K'.repeat(500) + OBJ_MARKER };
  const objectView = view(jsonObject);
  assert.equal(objectView.full, longObjectText, 'the FULL serialized JSON is retained');
  assert.ok(!objectView.pre.includes(OBJ_MARKER), 'the JSON preview must NOT contain the unseen tail');

  const shortView = view('rm -rf build/');
  assert.equal(shortView.full, undefined, 'no disclosure payload within the cap');
  assert.equal(shortView.pre, 'rm -rf build/');
});

test('native card: the disclosure JSX shape is pinned (summary label + full pre)', () => {
  const stripped = stripComments(read('src', 'features', 'codex', 'CodexAcpView.jsx'));
  const start = stripped.indexOf('function NativeApprovalCard');
  const next = stripped.indexOf('\nfunction ', start + 1);
  const slice = stripped.slice(start, next > start ? next : undefined);
  assert.ok(slice.includes('inputView && inputView.full ? ('), 'the disclosure renders only past the cap');
  assert.ok(slice.includes('{copy.nativeApprovalShowFullInput}'), 'the summary reads the localized label');
  assert.ok(slice.includes('{inputView.full}'), 'the disclosed pre renders the FULL input');
});
