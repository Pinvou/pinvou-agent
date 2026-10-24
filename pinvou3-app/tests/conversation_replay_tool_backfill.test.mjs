import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const helperSource = fs.readFileSync(path.join(appRoot, 'src/shared/bridge-shared-helpers.js'), 'utf8');
const tool = (id, name = 'read', input = {}) => ({ type: 'tool_use', id, name, input });
const result = (id, content, is_error = false) => ({ type: 'tool_result', tool_use_id: id, content, is_error });
const assistant = (...content) => ({ role: 'assistant', content });
const user = (...content) => ({ role: 'user', content });
const plain = value => JSON.parse(JSON.stringify(value));

function createReplay(runtime, { busy = true, onAdd = () => {}, renderMarkdown = text => `<p>${text}</p>` } = {}) {
  const state = { messages: [], chatItems: [], activeSessionId: 'replay-session', busy, pinvouSceneEvents: [] };
  const itemIdSeq = { value: 0 };
  const window = {};
  const context = vm.createContext({ window, console, TextEncoder });
  vm.runInContext(helperSource, context, { filename: 'bridge-shared-helpers.js' });
  const deps = {
    state,
    addChatItem(item) {
      item.id = ++itemIdSeq.value;
      state.chatItems.push(item);
      onAdd(item);
    },
    bt: key => key,
    BT_TABLE: { zh: {}, en: {}, ja: {} },
    DELIVERABLE_EXTS: ['.md'],
  };
  const shared = window.PinvouBridgeShared.create(runtime === 'web' ? 'web' : 'tauriMain', deps);
  const terminal = window.PinvouBridgeShared.create(runtime === 'web' ? 'web' : 'tauriTerminal', deps);
  const persona = window.PinvouBridgeShared.create(runtime === 'web' ? 'web:189622' : 'tauriMain:99856', {
    ...deps, pe: [{ kind: 'card_creator_intro', pos: 0 }],
  });
  // Load the actual host hooks, including each host's metadata lifetime and
  // unsettled-tool policy, around the real shared factory/core.
  const hostSource = fs.readFileSync(path.join(appRoot, `src/platform/${runtime}/bridge.js`), 'utf8');
  const start = hostSource.indexOf('.rerenderFromMessagesCore({') + '.rerenderFromMessagesCore('.length;
  const end = hostSource.indexOf('}, opts);', start) + 1;
  assert.ok(start > 0 && end > start);
  Object.assign(context, {
    state, itemIdSeq: 0, toolMeta: {},
    emitPersonaAt: persona.emitPersonaAt,
    userMessageDisplayText: () => '',
    updateToolItem: terminal.updateToolItem,
    PLAN_TOOLS: new Set(['update_plan', 'checklist_write', 'todo_write']),
    renderMarkdown,
    isSteeredMessagePos: () => false,
  });
  const lane = vm.runInContext(`(${hostSource.slice(start, end)})`, context);
  // Both addChatItem and the host hook must share the sequence cell.
  Object.defineProperty(itemIdSeq, 'value', { get: () => context.itemIdSeq, set: value => { context.itemIdSeq = value; } });
  return {
    state, lane, terminal,
    replay(messages, opts) {
      state.messages = messages;
      shared.rerenderFromMessagesCore(lane, opts);
      return state.chatItems;
    },
  };
}

for (const runtime of ['tauri', 'web']) {
  test(`${runtime}: ordinary replay preserves complete cards, ordering, first duplicates and meta lifetime`, () => {
    const h = createReplay(runtime, { onAdd(item) { if (item.type === 'tool') item._terminalParser = {}; } });
    const output = [{ type: 'text', text: 'full output\nline two' }];
    const args = { path: 'sample.txt' };
    const rows = h.replay([
      assistant({ type: 'text', text: 'before' }, tool('same', 'read', args), tool('same', 'read', { second: true }), tool('bad'), tool('unfinished')),
      user(result('same', output), result('bad', 'failure', true), result('missing', 'orphan'), result('same', 'repeat')),
    ]);
    assert.deepEqual(plain(rows.map(row => row.type)), ['card_creator_intro', 'assistant', 'tool', 'tool', 'tool', 'tool']);
    assert.deepEqual(plain(rows[1]), { type: 'assistant', text: 'before', html: '<p>before</p>', time: '', streaming: false, id: 2 });
    assert.deepEqual(plain(rows[2]), { type: 'tool', toolId: 'same', name: 'read', args, output: runtime === 'web' ? 'repeat' : output, success: true, state: 'done', id: 3 });
    assert.equal(rows[2].args, args);
    assert.equal('_terminalParser' in rows[2], false);
    assert.equal(rows[3].output, null);
    assert.equal(rows[3].state, 'pending');
    assert.deepEqual(plain(rows[4]), { type: 'tool', toolId: 'bad', name: 'read', args: {}, output: 'failure', success: false, state: 'failed', id: 5 });
    assert.equal(rows[5].state, 'pending');
    assert.deepEqual(Object.keys(h.lane.toolMeta.value).sort((a, b) => a.localeCompare(b)), runtime === 'web' ? ['bad', 'same', 'unfinished'] : []);
    assert.equal(h.terminal.updateToolItem('missing', 'none', true), null);
    assert.equal(h.terminal.updateToolItem('same', 'live result', true), rows[2]);
    assert.equal(rows[2].output, 'live result');
  });

  test(`${runtime}: legacy ID strict equality, magic strings and missing results`, () => {
    const h = createReplay(runtime);
    const ids = [7, '7', NaN, '__proto__', 'constructor', 'toString', ''];
    const rows = h.replay([
      assistant(...ids.map(id => tool(id))),
      user(...ids.map((id, i) => result(id, `result ${i}`))),
    ]).filter(row => row.type === 'tool');
    assert.deepEqual(plain(rows.map(row => row.output)), runtime === 'web'
      ? ['result 0', 'result 1', null, 'result 3', 'result 4', 'result 5', 'result 6']
      : ['result 0', null, null, 'result 3', 'result 4', 'result 5', 'result 6']);
    assert.equal(h.terminal.updateToolItem(NaN, 'must not match', true), null);
    assert.equal(h.terminal.updateToolItem(7, 'numeric', true), rows[0]);
    assert.equal(h.terminal.updateToolItem('7', 'string', false), rows[1]);
  });

  test(`${runtime}: structured output retains identity and legacy IDs delegate to the scan`, () => {
    const h = createReplay(runtime);
    const output = [{ type: 'text', text: 'all output' }, { type: 'image', source: { type: 'base64', data: 'synthetic' } }];
    const scanned = [];
    const scan = h.lane.updateToolItem;
    h.lane.updateToolItem = (...args) => { scanned.push(args[0]); return scan(...args); };
    const rows = h.replay([
      assistant(tool('ordinary'), tool(4), tool(NaN)),
      user(result('ordinary', output), result(4, 'legacy'), result(NaN, 'no strict match')),
    ]);
    assert.deepEqual(plain(rows[1].output), output);
    assert.equal(rows[1].output[1], output[1]);
    assert.deepEqual(scanned, [4, NaN]);
    assert.equal(rows[2].output, 'legacy');
    assert.equal(rows[3].output, null);
  });

  test(`${runtime}: specialized cards stay excluded, last result restores choices, blocked and hidden output survive`, () => {
    const h = createReplay(runtime);
    const blocked = 'BLOCKED: This command was blocked for safety reasons\nReasons: restricted\nSuggestions: inspect';
    const rows = h.replay([
      assistant(
        tool('choice', 'request_user_input', { questions: [{ id: 'q', question: 'Pick' }] }),
        tool('plan', 'update_plan'),
        tool('artifact', 'present_artifact', { path: '/tmp/report.md', title: 'Report' }),
        tool('failed-artifact', 'present_artifact', { path: '/tmp/failed.md' }),
        tool('blocked'), tool('skill', 'load_skill'),
      ),
      user(result('choice', '{"answers":[{"id":"q","value":"old"}]}'), result('choice', '{"answers":[{"id":"q","value":"last"}]}'), result('plan', 'plan\n{"items":[{"step":"Inspect","status":"completed"}]}'), result('artifact', 'ok'), result('failed-artifact', 'failed', true), result('blocked', blocked), result('skill', 'private skill body')),
    ]);
    assert.deepEqual(plain(rows.map(row => row.type)), ['card_creator_intro', 'user_input', 'artifact_card', 'tool', 'tool', 'tool', 'plan_card', 'careful_blocked']);
    assert.equal(rows[1].restoredAnswers[0].value, 'last');
    assert.equal('output' in rows[1], false);
    assert.equal('output' in rows[2], false);
    assert.equal('output' in rows[6], false);
    assert.equal(rows[3].output, 'failed');
    assert.equal(rows[3].state, 'failed');
    assert.equal(rows[4].output, blocked);
    assert.equal(rows[4].state, 'failed');
    assert.equal(rows[5].output, 'skillContentHidden');
    assert.equal(rows[7].metadata.blocked, true);
    assert.equal(rows[7].toolCallId, runtime === 'web' ? 'blocked' : undefined);
  });

  test(`${runtime}: each replay and renderer failure get a fresh lookup`, () => {
    let throwRender = false;
    const h = createReplay(runtime, { renderMarkdown(text) { if (throwRender) throw new Error('renderer failed'); return text; } });
    const transcript = output => [assistant(tool('same')), user(result('same', output))];
    const old = h.replay(transcript('old'))[1];
    throwRender = true;
    assert.throws(() => h.replay([assistant(tool('same'), { type: 'text', text: 'throw' })]), /renderer failed/);
    const interrupted = h.state.chatItems[1];
    throwRender = false;
    const current = h.replay(transcript('new'))[1];
    assert.equal(old.output, 'old');
    assert.equal(interrupted.output, null);
    assert.equal(current.output, 'new');
    assert.notEqual(current, old);
    assert.notEqual(current, interrupted);
  });

  test(`${runtime}: string result backfill does not revisit prior tool predicates`, () => {
    let typeReads = 0;
    const h = createReplay(runtime, { onAdd(item) {
      if (item.type === 'tool') Object.defineProperty(item, 'type', { enumerable: true, get() { typeReads += 1; return 'tool'; } });
    } });
    const count = 80;
    h.replay([assistant(...Array.from({ length: count }, (_, i) => tool(`tool-${i}`))), user(...Array.from({ length: count }, (_, i) => result(`tool-${i}`, 'done')))]);
    assert.equal(typeReads, 0, 'ordinary string-ID result lookup must not scan earlier cards');
    assert.equal(h.state.chatItems.length, count + 1);
    assert.equal(h.state.chatItems.at(-1).output, 'done');
  });

  test(`${runtime}: durable unsettled sweep and live metadata hooks remain host-specific`, () => {
    const h = createReplay(runtime, { busy: false });
    h.lane.toolMeta.value = { live: { name: 'read', args: {} } };
    const row = h.replay([assistant(tool('pending'))], { keepLiveToolMeta: true })[1];
    assert.equal(row.state, runtime === 'tauri' ? 'failed' : 'pending');
    assert.equal(h.lane.toolMeta.value.live.name, 'read');
    assert.equal(h.lane.toolMeta.value.pending.name, 'read');
    h.replay([]);
    assert.deepEqual(Object.keys(h.lane.toolMeta.value), []);
  });

  test(`${runtime}: replay joins adjacent assistant text blocks with a blank line and drops textless or null blocks`, () => {
    // Adjacent text blocks are separate markdown blocks: raw concatenation glued
    // a following table onto the previous line and it never parsed; a textless
    // block used to render the literal string "undefined", and a null entry in
    // a persisted content array threw mid-replay (the native lane already
    // skipped it) — it is skipped here too, so its neighbours stay adjacent.
    const h = createReplay(runtime, { busy: false });
    const rows = h.replay([
      assistant({ type: 'text', text: 'Summary:' }, null, { type: 'text', text: '| a | b |\n|---|---|' }),
      assistant({ type: 'text' }),
    ]);
    assert.deepEqual(
      plain(rows.filter(row => row.type === 'assistant').map(row => row.text)),
      ['Summary:\n\n| a | b |\n|---|---|'],
    );
  });
}
