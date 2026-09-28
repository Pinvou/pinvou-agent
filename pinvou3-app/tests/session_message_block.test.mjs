import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

// Import in a window-less harness first: the module guards its global
// publication on `typeof window !== 'undefined'`.
const { MESSAGE_BLOCK_HEADER, splitSessionMessageBlock } = await import(
  '../src/features/chat/session-message-block.js'
);

const buildBlock = (senderJson, body) => `${MESSAGE_BLOCK_HEADER}\n${senderJson}\n\n${body}`;

test('message block contract: attributed sender round-trips; body extracted losslessly', () => {
  const delivered = buildBlock('{"sessionId":"src0001","title":"源会话"}', '请确认上次的结论\n第二行');
  const split = splitSessionMessageBlock(delivered);
  assert.deepEqual(split.sender, { sessionId: 'src0001', title: '源会话' });
  assert.equal(split.text, '请确认上次的结论\n第二行');
});

test('unattributed sender (null fields) still parses', () => {
  const delivered = buildBlock('{"sessionId":null,"title":null}', '交接正文');
  const split = splitSessionMessageBlock(delivered);
  assert.deepEqual(split.sender, { sessionId: null, title: null });
  assert.equal(split.text, '交接正文');
});

test('sender-only trimmed form (JSON line is the last line) yields an empty body', () => {
  const trimmed = `${MESSAGE_BLOCK_HEADER}\n{"sessionId":"a","title":"t"}`;
  const split = splitSessionMessageBlock(trimmed);
  assert.equal(split.text, '');
  assert.equal(split.sender.sessionId, 'a');
});

test('hand-written lookalikes are not swallowed', () => {
  for (const lookalike of [
    // Non-object / bad JSON line.
    `${MESSAGE_BLOCK_HEADER}\n[1,2]\n\n正文`,
    `${MESSAGE_BLOCK_HEADER}\nnot-json\n\n正文`,
    // Missing blank separator while a body follows.
    `${MESSAGE_BLOCK_HEADER}\n{"sessionId":"a","title":"t"}\n正文`,
    // Not at the very start.
    `普通消息\n${MESSAGE_BLOCK_HEADER}\n{"sessionId":"a"}\n\n正文`,
    // Plain messages.
    '普通消息',
  ]) {
    const split = splitSessionMessageBlock(lookalike);
    assert.equal(split.sender, null, lookalike);
    assert.equal(split.text, lookalike, lookalike);
  }
});

test('spoofed-block hardening: absurdly long JSON lines are skipped before JSON.parse; titles are capped', () => {
  const huge = `${MESSAGE_BLOCK_HEADER}\n{"sessionId":"a","title":"${'t'.repeat(70 * 1024)}"}\n\n正文`;
  const split = splitSessionMessageBlock(huge);
  assert.equal(split.sender, null);
  assert.equal(split.text, huge);
  const longTitle = splitSessionMessageBlock(buildBlock(`{"sessionId":"a","title":"${'x'.repeat(500)}"}`, '正文'));
  assert.equal(longTitle.sender.title.length, 200);
});

test('auto-title contract: both bridges strip the received-message block via the same window-global parser', () => {
  for (const rel of ['../src/platform/tauri/bridge.js', '../src/platform/web/bridge.js']) {
    const source = readFileSync(new URL(rel, import.meta.url), 'utf8');
    assert.match(source, /__PINVOU_SESSION_MESSAGE__/, rel);
    assert.match(source, /splitMessage\(titleText\)/, rel);
  }
  const moduleSource = readFileSync(
    new URL('../src/features/chat/session-message-block.js', import.meta.url), 'utf8');
  assert.match(moduleSource, /window\.__PINVOU_SESSION_MESSAGE__ = \{ splitSessionMessageBlock \}/);
});

test('JS↔Rust drift pin: the messaging module still carries the verbatim block contract', () => {
  const modRs = readFileSync(
    new URL('../src-tauri/src/features/messaging/mod.rs', import.meta.url), 'utf8');
  assert.match(modRs, /MESSAGE_BLOCK_HEADER: &str = "## Message from another session";/);
  assert.match(modRs, /fn build_session_message_block/);
});
