import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const helperSource = fs.readFileSync(process.env.ARTIFACT_REPLAY_SOURCE || path.join(appRoot, 'src/shared/bridge-shared-helpers.js'), 'utf8');
const tool = (id, name = 'read', input = {}) => ({ type: 'tool_use', id, name, input });
const result = (id, content, is_error = false) => ({ type: 'tool_result', tool_use_id: id, content, is_error });
const assistant = (...content) => ({ role: 'assistant', content });
const user = (...content) => ({ role: 'user', content });
const plain = value => JSON.parse(JSON.stringify(value));

function createReplay(runtime, { busy = true, onAdd = () => {}, renderMarkdown = text => `<p>${text}</p>`, source = helperSource, onSet = () => {}, buffer = null, internalAwareDisplay = false } = {}) {
  const state = { messages: [], chatItems: [], activeSessionId: 'replay-session', busy, pinvouSceneEvents: [] };
  const itemIdSeq = { value: 0 };
  const window = {};
  const context = vm.createContext({ window, console, TextEncoder, Set: class extends Set { constructor(...args) { super(...args); onSet(); } } });
  vm.runInContext(source, context, { filename: 'bridge-shared-helpers.js' });
  const deps = {
    state,
    addChatItem(item) {
      item.id = ++itemIdSeq.value;
      state.chatItems.push(item);
      onAdd(item);
    },
    bt: key => key,
    BT_TABLE: { zh: {}, en: {}, ja: {} },
    DELIVERABLE_EXTS: new Set(['md']),
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
    // internalAwareDisplay: 用真实分类器（bridge-messages.js 同 context 装载）
    // 还原 host 的 display-text 语义——完全内部的 user 消息（canonical
    // <context_update>）落下不渲染，走重放的「落下分支」计数。
    userMessageDisplayText: blocks => {
      const text = blocks.filter(b => b.type === 'text').map(b => String(b.text || '')).join('');
      if (!text) return '';
      if (internalAwareDisplay && window.PinvouBridgeMessages?.isInternalRuntimeUserMessage?.(text)) return '';
      return text;
    },
    updateToolItem: terminal.updateToolItem,
    PLAN_TOOLS: new Set(['update_plan', 'checklist_write', 'todo_write']),
    renderMarkdown,
    isSteeredMessagePos: () => false,
    // <context_update> 尾随计数回种钩子的 buffer 取面：无 buffer（返回
    // null）时钩子按空操作收敛（见 setRuntimeContextUpdatesPending）。
    getBuffer: () => buffer,
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

const present = (id, path, title = id) => tool(id, 'present_artifact', { path, title, description: `description ${id}` });
const write = (id, path) => tool(id, 'write', { path, content: 'synthetic' });
const producer = id => tool(id, 'mcp_report');
const output = (id, path) => result(id, JSON.stringify({ abs_path: path }));
const cards = rows => rows.filter(row => row.type === 'artifact_card');

for (const runtime of ['tauri', 'web']) {
  test(`${runtime}: artifact membership allocates lazily once per replay`, () => {
    let allocations = 0;
    const h = createReplay(runtime, { onSet() { allocations++; } });
    allocations = 0;
    h.replay([assistant(write('code', 'file.js'))]);
    assert.equal(allocations, 0);
    h.replay([assistant(present('one', 'one.md'), present('two', 'two.md'), present('three', 'three.md'))]);
    assert.equal(allocations, 1);
    h.replay([assistant(present('new', 'new.md'))]);
    assert.equal(allocations, 2);
  });

  test(`${runtime}: negative artifact lookups do not revisit ordinary cards`, () => {
    let reads = 0;
    const h = createReplay(runtime, { onAdd(item) {
      if (item.type === 'tool') Object.defineProperty(item, 'type', { enumerable: true, get() { reads++; return 'tool'; } });
    } });
    h.replay([assistant(...Array.from({ length: 40 }, (_, i) => write(`w${i}`, `src/file${i}.js`)))]);
    assert.equal(reads, 0, 'a known artifact basename miss must skip the legacy chat-item scan');
    assert.equal(h.state.chatItems.length, 41);
  });

  test(`${runtime}: known misses still skip scans after artifact cards have been appended`, () => {
    let reads = 0;
    const h = createReplay(runtime, { onAdd(item) {
      if (item.type === 'tool') Object.defineProperty(item, 'type', { enumerable: true, get() { reads++; return 'tool'; } });
    } });
    h.replay([assistant(present('initial', 'initial.md'), ...Array.from({ length: 40 }, (_, i) => write(`w${i}`, `file${i}.js`)), present('other', 'other.md'))]);
    assert.equal(reads, 0);
  });

  test(`${runtime}: explicit cards preserve complete values, stable IDs and relative upgrades`, () => {
    const messages = [assistant(present('p1', 'report.md'), present('p2', '/work/report.md'), present('p3', 'report.md'))];
    const before = plain(messages);
    const rows = createReplay(runtime).replay(messages);
    assert.deepEqual(plain(cards(rows)), [{ type: 'artifact_card', path: '/work/report.md', title: 'p3', description: 'description p3', time: '', sessionId: 'replay-session', id: 2 }]);
    assert.deepEqual(messages, before);
  });

  test(`${runtime}: exact normalized path beats newer basename and absolute collisions remain separate`, () => {
    const rows = createReplay(runtime).replay([assistant(
      present('a', 'C:\\one\\report.md'), present('b', 'C:/two/report.md'),
      present('exact', 'C:/one/report.md'), present('case', 'C:/one/Report.md'),
      present('unc', '\\\\server\\share\\report.md'), present('unc-again', '//server/share/report.md'),
    )]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title, c.id])), [
      ['C:/one/report.md', 'exact', 2], ['C:/two/report.md', 'b', 3],
      ['C:/one/Report.md', 'case', 4], ['//server/share/report.md', 'unc-again', 5],
    ]);
  });

  test(`${runtime}: user boundaries append while file mutations keep existing continuation semantics`, () => {
    const h = createReplay(runtime);
    const rows = h.replay([
      assistant(present('p', '/work/report.md')), user({ type: 'text', text: 'next' }),
      assistant(present('p2', '/work/report.md')), user({ type: 'text', text: 'edit it' }),
      assistant(write('w', '/work/report.md')), user(result('w', 'ok')),
    ]);
    assert.deepEqual(plain(cards(rows).map(c => [c.id, c.title])), [[2, 'p'], [4, 'p2']]);
    assert.deepEqual(plain(rows.map(c => c.type)), ['card_creator_intro', 'artifact_card', 'user', 'artifact_card', 'user', 'tool']);
  });

  test(`${runtime}: output fallback is registered before a same-basename output continuation`, () => {
    const h = createReplay(runtime);
    const rows = h.replay([
      assistant(producer('g1'), producer('g2'), producer('g3')),
      user(output('g1', '/one/report.md'), output('g2', '/two/report.md'), output('g3', 'report.md')),
    ]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title, c.id])), [['/one/report.md', 'report.md', 3]]);
    assert.equal(rows[4].output, JSON.stringify({ abs_path: 'report.md' }));
  });

  test(`${runtime}: mutation fallback registers cards for later mutations of another path`, () => {
    const rows = createReplay(runtime).replay([assistant(write('w1', '/one/report.md'), write('w2', '/two/report.md'), write('w3', 'report.md'))]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title, c.id])), [['/one/report.md', 'report.md', 3]]);
  });

  test(`${runtime}: explicit presentation suppresses output fallback and later mutations reuse its card`, () => {
    const rows = createReplay(runtime).replay([
      assistant(present('p', 'report.md')), user({ type: 'text', text: 'next' }),
      assistant(producer('g')), user(output('g', '/new/report.md')),
      assistant(write('w', 'report.md'), present('last', 'report.md')),
    ]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title])), [['report.md', 'last']]);
  });

  test(`${runtime}: output continuation appends after a user boundary`, () => {
    const rows = createReplay(runtime).replay([
      assistant(producer('first')), user(output('first', '/one/report.md'), { type: 'text', text: 'next' }),
      assistant(producer('second')), user(output('second', '/two/report.md')),
    ]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.id])), [['/one/report.md', 3], ['/one/report.md', 6]]);
    assert.deepEqual(plain(rows.map(c => c.type)), ['card_creator_intro', 'tool', 'artifact_card', 'user', 'tool', 'artifact_card']);
  });

  test(`${runtime}: the current mutation boundary permits continuation across a prior user message`, () => {
    const rows = createReplay(runtime).replay([
      assistant(present('first', '/one/report.md')), user({ type: 'text', text: 'next' }),
      assistant(tool('edit', 'edit', { path: '/two/report.md' })),
    ]);
    // A matching mutation is the existing update boundary: preserve its old card.
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.id])), [['/one/report.md', 2]]);
  });

  test(`${runtime}: explicit temporary/non-deliverable, missing and failed presents retain legacy behavior`, () => {
    const rows = createReplay(runtime).replay([
      assistant(present('tmp', '/tmp/a.js'), present('missing', ''), present('failed', '/work/no.md'), write('bad', '/work/bad.md'), producer('bad-g')),
      user(result('failed', 'failure', true), result('bad', 'failure', true), result('bad-g', '{"path":"bad.md"}', true)),
    ]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title])), [['/tmp/a.js', 'tmp'], ['', 'missing']]);
    assert.equal(rows.find(c => c.toolId === 'failed').state, 'failed');
    assert.equal(rows.find(c => c.toolId === 'bad').state, 'failed');
  });

  test(`${runtime}: multipath patches, repeated artifacts and prototype-like basenames preserve order`, () => {
    const rows = createReplay(runtime).replay([assistant(
      present('proto', '__proto__'), present('ctor', 'constructor'), present('slash', '/folder/'),
      tool('patch', 'File', { action: 'patch', patch: '*** Add File: one.md\n*** Add File: two.md\n*** Update File: one.md' }),
      write('one', 'one.md'), present('proto2', '__proto__'),
    )]);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title])), [['__proto__', 'proto2'], ['constructor', 'ctor'], ['/folder/', 'slash'], ['two.md', 'two.md'], ['one.md', 'one.md']]);
  });

  test(`${runtime}: unsupported card paths preserve coercion and disable negative filtering`, () => {
    let coercions = 0;
    const unusual = { toString() { coercions++; return 'report.md'; } };
    const h = createReplay(runtime);
    const rows = h.replay([assistant(present('odd', unusual), present('string', 'report.md'))]);
    assert.equal(cards(rows).length, 1);
    assert.equal(cards(rows)[0].title, 'string');
    assert.equal(coercions, 4, 'registering an unsupported path must not add coercions');
  });

  test(`${runtime}: unsupported successful updates disable membership before a path can change basename`, () => {
    let path = 'first.md';
    const unusual = { toString() { return path; } };
    const h = createReplay(runtime, { renderMarkdown(text) { path = 'second.md'; return text; } });
    const rows = h.replay([assistant(
      present('first', 'first.md'), present('unusual', unusual),
      { type: 'text', text: 'change path coercion' }, present('last', 'second.md'),
    )]);
    assert.deepEqual(plain(cards(rows)), [{ type: 'artifact_card', path: 'second.md', title: 'last', description: 'description last', time: '', sessionId: 'replay-session', id: 2 }]);
  });

  test(`${runtime}: changing coercion after a successful unsupported update preserves the legacy card and calls`, () => {
    let calls = 0;
    const unusual = { toString() { return ++calls <= 3 ? 'report.md' : 'other.md'; } };
    const rows = createReplay(runtime).replay([assistant(
      present('first', 'report.md'), present('odd', unusual), present('last', 'other.md'),
    )]);
    assert.deepEqual(plain(cards(rows)), [{ type: 'artifact_card', path: 'other.md', title: 'last', description: 'description last', time: '', sessionId: 'replay-session', id: 2 }]);
    assert.equal(calls, 5);
  });

  test(`${runtime}: unsupported query coercion can fail only at the legacy lookup`, () => {
    let coercions = 0;
    const unusual = { toString() { if (++coercions === 3) throw new Error('legacy coercion'); return 'report.md'; } };
    const h = createReplay(runtime);
    assert.throws(() => h.replay([assistant(present('odd', unusual), present('second', 'report.md'))]), /legacy coercion/);
    assert.equal(coercions, 3);
    assert.equal(h.state.chatItems.length, 2);
    assert.equal(h.state.chatItems[1].title, 'odd');
  });

  test(`${runtime}: separate replays and throwing-renderer recovery never reuse membership`, () => {
    let throwing = false;
    const h = createReplay(runtime, { renderMarkdown(text) { if (throwing) throw new Error('render failed'); return text; } });
    const old = h.replay([assistant(present('old', 'old.md'))])[1];
    throwing = true;
    assert.throws(() => h.replay([assistant(present('partial', 'partial.md'), { type: 'text', text: 'fail' })]), /render failed/);
    throwing = false;
    const messages = [assistant(present('fresh', 'fresh.md'), present('again', 'fresh.md'), write('write', 'old.md'))];
    const rows = h.replay(messages);
    assert.deepEqual(plain(cards(rows).map(c => [c.path, c.title, c.id])), [['fresh.md', 'again', 2], ['old.md', 'old.md', 4]]);
    assert.equal(old.title, 'old');
  });

  // <context_update> 序号计数的回放腿行为钉（r7 M3：此前只有 bridge.js 的
  // 出现性 pin——杀死「尾随余量回种」或「落下分支累加」，重启后的 fork/
  // rewind 序号整体低计，而全部套件保持绿）。装置形状：canonical 漂移增量
  // 夹在两个真实轮之间（随下一气泡下发）+ 一个尾随增量（回种 buffer）。
  test(`${runtime}: replay counts canonical context_update boundaries and seeds the trailing remainder`, () => {
    const canonicalDelta = () => ({ type: 'text', text: [
      '<context_update>',
      'Session context changed since it was pinned; the pinned system prompt is unchanged. Delta (+ added, - removed):',
      '',
      '- stale pinned line',
      '</context_update>',
    ].join('\n') });
    const buffer = { runtimeContextUpdatesPending: 0 };
    const h = createReplay(runtime, {
      buffer,
      internalAwareDisplay: true,
      source: helperSource + '\n' + fs.readFileSync(path.join(appRoot, 'src/shared/bridge-messages.js'), 'utf8'),
    });
    const rows = h.replay([
      user({ type: 'text', text: 'turn one' }),
      assistant({ type: 'text', text: 'reply one' }),
      user(canonicalDelta()),
      user({ type: 'text', text: 'turn two' }),
      assistant({ type: 'text', text: 'reply two' }),
      user(canonicalDelta()),
    ]);
    const turnTwo = rows.find(row => row.type === 'user' && row.text === 'turn two');
    assert.ok(turnTwo, 'the real turn must still render');
    assert.equal(
      turnTwo.runtimeContextUpdatesBefore, 1,
      'the mid-stream canonical delta must ride onto the next user bubble',
    );
    assert.ok(
      !rows.some(row => row.type === 'user' && typeof row.text === 'string' && row.text.includes('<context_update>')),
      'canonical deltas must not render as bubbles on replay',
    );
    // 本 harness 的 lane 不带 seed-back hook（web 车道形态，web 无此钩子是
    // 既定口径）——守卫因此合法跳过；hook 本身的在场由 context_update_counter
    // 的 bridge.js 源码钉看住（r8 核对：删除 hook 文本会使该钉转红）。
    if (typeof h.lane.setRuntimeContextUpdatesPending === 'function') {
      assert.equal(
        buffer.runtimeContextUpdatesPending, 1,
        'the trailing remainder must seed back into the session buffer (post-restart ordinals)',
      );
    }
  });
}
