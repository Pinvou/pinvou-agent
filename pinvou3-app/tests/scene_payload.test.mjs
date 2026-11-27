// 场景路由载荷 → 用户原文的互逆解析（features/chat/scene-payload.js）。
// 发送路径只给模型看路由文案、气泡显示原文；落盘的是包装后的全文，历史重放和
// 远程准入靠这里的解析还原。构造与解析必须逐字节互逆，且解析宁可少剥也不吞
// 用户内容。
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { splitScenePayload } from '../src/features/chat/scene-payload.js';
import {
  SUFFIX_SCENE_ROUTES,
  createDataVisualizationMessageMeta,
  createDocumentWritingMessageMeta,
  createPptDesignMessageMeta,
  scenePayloadSuffix,
} from '../src/features/chat/work-scene-routes.js';
import { buildSessionMentionBlock } from '../src/features/chat/session-mention.js';
import {
  VISUAL_POSTER_AUDIT,
  VISUAL_POSTER_CONTEXT,
  createVisualPosterMessageMeta,
} from '../src/features/chat/visual-poster-scene.js';
import {
  DEFAULT_PERSONAL_WORKBENCH_PROMPT,
  PERSONAL_WORKBENCH_REQUEST_MARK,
  PERSONAL_WORKBENCH_TEMPLATES,
  createPersonalWorkbenchMessageMeta,
} from '../src/features/chat/personal-workbench-scene.js';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const suffixRoutes = [
  ...SUFFIX_SCENE_ROUTES,
  { key: 'poster', context: VISUAL_POSTER_CONTEXT, audit: VISUAL_POSTER_AUDIT },
];
const suffixOf = route => scenePayloadSuffix(route.context, route.audit);
const documentRoute = SUFFIX_SCENE_ROUTES.find(route => route.key === 'document-writing');
const documentScene = createDocumentWritingMessageMeta('x').pinvouScene;
const workbenchScene = createPersonalWorkbenchMessageMeta('x').pinvouScene;
const builders = [
  ['公文写作', createDocumentWritingMessageMeta],
  ['数据可视化', createDataVisualizationMessageMeta],
  ['PPT 设计', createPptDesignMessageMeta],
  ['视觉海报', createVisualPosterMessageMeta],
  ['个人工作台（默认提示词）', text => createPersonalWorkbenchMessageMeta(text)],
];

const bodies = [
  '写一份通知',
  '多行正文\n第二行\n\n---\n分隔线之后还有内容\n\n---\n再来一条',
  '引用别的场景标题：\nPinvou 数据可视化场景路由：\n- 这是我粘贴的内容',
  '用户需求：\n这是我自己写的标题',
  '  两端带空白  ',
];

test('every scene builder inverts byte-exactly', () => {
  for (const [name, build] of builders) {
    for (const body of bodies) {
      const meta = build(body);
      assert.deepEqual(splitScenePayload(meta.pinvouPayloadText), { scene: meta.pinvouScene, text: body.trim() },
        `${name}: ${JSON.stringify(body)}`);
      // The recorded scene is a tie-breaker, never a requirement.
      assert.deepEqual(splitScenePayload(meta.pinvouPayloadText, meta.pinvouScene), { scene: meta.pinvouScene, text: body.trim() });
    }
  }
});

test('the builders still emit the documented payload shapes', () => {
  const context = 'CTX\nline';
  const audit = 'AUDIT\nline';
  assert.equal(scenePayloadSuffix(context, audit), `\n\n---\n${context}\n\n${audit}`);
  assert.equal(createDocumentWritingMessageMeta('  正文  ').pinvouPayloadText, `正文${scenePayloadSuffix(documentRoute.context, documentRoute.audit)}`);
  assert.equal(createPersonalWorkbenchMessageMeta('运动').pinvouPayloadText,
    `${DEFAULT_PERSONAL_WORKBENCH_PROMPT}${PERSONAL_WORKBENCH_REQUEST_MARK}运动`);
  // The stale fallback anchors on each route's first header line and takes the
  // FIRST request mark as the split point; that is only sound while the
  // prompt itself never carries the mark.
  assert.equal(DEFAULT_PERSONAL_WORKBENCH_PROMPT.includes(PERSONAL_WORKBENCH_REQUEST_MARK), false);
  const headers = suffixRoutes.map(route => route.context.split('\n')[0]);
  assert.ok(headers.every(header => header.length > 0));
  assert.equal(new Set(headers).size, headers.length, 'suffix scene header lines must stay pairwise distinct');
});

test('a personal-workbench template send is the template text, not a payload', () => {
  const template = PERSONAL_WORKBENCH_TEMPLATES[0];
  const meta = createPersonalWorkbenchMessageMeta(template.prompt, template.id);
  assert.equal(meta.pinvouPayloadText, undefined);
  assert.equal(splitScenePayload(template.prompt), null);
});

test('ordinary text is never mistaken for a payload', () => {
  for (const text of [
    '',
    '写一份通知',
    '第一段\n\n---\n第二段',
    `---\n${documentRoute.context.split('\n')[0]}\n顶格的标题，没有前面的空行`,
    `Pinvou 公文写作场景路由：\n只有标题行，没有分隔线`,
    `正文\n\n---\n${documentRoute.context.split('\n')[0]}尾随文字没有换行`,
    DEFAULT_PERSONAL_WORKBENCH_PROMPT.split('。')[0],
  ]) {
    assert.equal(splitScenePayload(text), null, JSON.stringify(text.slice(0, 40)));
    assert.equal(splitScenePayload(text, documentScene), null, 'a scene hint alone must not make text a payload');
    assert.equal(splitScenePayload(text, workbenchScene), null);
  }
});

test('a wrapper with an empty body is not a payload: a message never vanishes', () => {
  for (const route of suffixRoutes) {
    const wrapper = suffixOf(route);
    assert.equal(splitScenePayload(wrapper, `lane:${route.key}`), null, route.key);
    assert.equal(splitScenePayload(`${wrapper}\n`, `lane:${route.key}`), null, `${route.key} trailing newline`);
  }
  assert.equal(splitScenePayload(`${DEFAULT_PERSONAL_WORKBENCH_PROMPT}${PERSONAL_WORKBENCH_REQUEST_MARK}`, workbenchScene), null);
});

test('pasting an earlier payload only removes the outermost wrapper', () => {
  const earlier = createDocumentWritingMessageMeta('旧正文').pinvouPayloadText;
  const resent = createDocumentWritingMessageMeta(earlier);
  assert.deepEqual(splitScenePayload(resent.pinvouPayloadText), { scene: resent.pinvouScene, text: earlier });
});

test('a payload of another scene pasted into a scene: the recorded scene names the outer wrapper', () => {
  const workbench = createPersonalWorkbenchMessageMeta('理财').pinvouPayloadText;
  const both = `${workbench}${suffixOf(documentRoute)}`;
  // Workbench on record: its prefix wrapper is outermost, the rest is the
  // user's raw text ("理财" followed by the pasted document wrapper).
  assert.deepEqual(splitScenePayload(both, workbenchScene), { scene: workbenchScene, text: `理财${suffixOf(documentRoute)}` });
  // Document scene on record: the suffix wrapper is outermost, the user
  // pasted a whole workbench payload as their text.
  assert.deepEqual(splitScenePayload(both, documentScene), { scene: documentScene, text: workbench });
  // No record: both exact tiers hit, and the readings disagree. The workbench
  // (prefix) reading wins: for the common composition — a scene payload pasted
  // into the workbench composer — it restores the typed text byte-exactly,
  // while the suffix reading would leak the whole workbench prompt as "user
  // text". For the mirrored composition (a workbench payload pasted into a
  // scene composer) this keeps the prompt out of the bubble at the cost of
  // leaving the smaller scene wrapper in it.
  assert.deepEqual(splitScenePayload(both), { scene: workbenchScene, text: `理财${suffixOf(documentRoute)}` },
    'with nothing on record the workbench reading wins the double hit');
});

test('stale boilerplate is anchored on the stable header line, and only for the scene on record', () => {
  const staleSuffix = [
    '写一份通知',
    '',
    '---',
    documentRoute.context.split('\n')[0],
    '- 旧版本的路由正文。',
    '',
    '生成完成前执行旧版自检：',
    '1. 旧条目。',
  ].join('\n');
  // Suffix wrapper: the words precede the anchor.
  assert.deepEqual(splitScenePayload(staleSuffix, documentScene), { scene: documentScene, text: '写一份通知' });
  assert.equal(splitScenePayload(staleSuffix), null, 'the header anchor is a guess: no record, no strip');
  assert.equal(splitScenePayload(staleSuffix, 'work:ppt'), null, 'another scene on record does not vouch for this header');
  assert.equal(splitScenePayload(staleSuffix, workbenchScene), null);
  // A hand-typed lookalike after (or before) the real wrapper: the LAST
  // anchor occurrence fails the bullet guard, so the strip is refused and the
  // message stays whole — under-stripping beats swallowing typed content.
  const withRule = `${staleSuffix}\n\n---\n用户自己补的\n\n---\n${documentRoute.context.split('\n')[0]}\n别切这里`;
  assert.equal(splitScenePayload(withRule, documentScene), null,
    'a lookalike anchor without a route-body bullet keeps the message whole');

  const head = `${DEFAULT_PERSONAL_WORKBENCH_PROMPT.split('。')[0]}。`;
  const staleWorkbench = `${head}旧版本的提示词正文。\n\n用户需求：\n运动`;
  // Prefix wrapper: the words follow the mark.
  assert.deepEqual(splitScenePayload(staleWorkbench, workbenchScene), { scene: workbenchScene, text: '运动' });
  assert.equal(splitScenePayload(staleWorkbench), null);
  assert.equal(splitScenePayload(staleWorkbench, documentScene), null);
  assert.equal(splitScenePayload(`${head}旧正文里没有需求标记。`, workbenchScene), null);
  assert.equal(splitScenePayload(`不是工作台提示词。${PERSONAL_WORKBENCH_REQUEST_MARK}运动`, workbenchScene), null,
    'the known first sentence of the old head is required');
  const repeated = `${head}旧正文。${PERSONAL_WORKBENCH_REQUEST_MARK}理财${PERSONAL_WORKBENCH_REQUEST_MARK}自定义`;
  assert.equal(splitScenePayload(repeated, workbenchScene).text, `理财${PERSONAL_WORKBENCH_REQUEST_MARK}自定义`,
    'the first mark after the prompt head splits; later ones belong to the user');
});

test('a workbench payload behind a canonical mention block still inverts (the block survives)', () => {
  // ChatView.jsx 组装发送 meta 时把规范提及块拼在 pinvouPayloadText 头部
  // （buildSessionMentionBlock(refs) + payload）。后缀式包装对任意前缀免疫；
  // 前缀式工作台须先剥块再判载荷，块原样留在还原结果里交还气泡解析。
  const block = buildSessionMentionBlock([{ sessionId: 's-1', title: '上次会话' }]);
  const meta = createPersonalWorkbenchMessageMeta('运动');
  const composite = block + meta.pinvouPayloadText;
  const expected = { scene: workbenchScene, text: `${block}运动` };
  assert.deepEqual(splitScenePayload(composite), expected, 'no hint needed: the composite is byte-exact');
  assert.deepEqual(splitScenePayload(composite, workbenchScene), expected);
  // Suffix scenes tolerate the prefix already; pinned for symmetry.
  const docMeta = createDocumentWritingMessageMeta('写一份通知');
  assert.deepEqual(splitScenePayload(block + docMeta.pinvouPayloadText), { scene: documentScene, text: `${block}写一份通知` });
  // Stale workbench copy behind the block: hint tier still applies inside.
  const head = `${DEFAULT_PERSONAL_WORKBENCH_PROMPT.split('。')[0]}。`;
  const staleComposite = block + head + '旧版本的提示词正文。\n\n用户需求：\n运动';
  assert.deepEqual(splitScenePayload(staleComposite, workbenchScene), { scene: workbenchScene, text: `${block}运动` });
  assert.equal(splitScenePayload(staleComposite), null, 'stale tier stays hint-gated behind the block too');

  // Negative space: nothing here may strip or vanish.
  assert.equal(splitScenePayload(block), null, 'a refs-only block is not a payload: the message stays');
  assert.equal(splitScenePayload(block + '我自己写的话'), null, 'block + hand-typed text is not a payload');
  assert.equal(splitScenePayload(block + meta.pinvouPayloadText.slice(0, -2)), null,
    'block + bare wrapper (empty body): the message never vanishes');
  // A non-canonical lookalike (broken blank-separator contract) stays whole.
  const forged = block.slice(0, -1) + meta.pinvouPayloadText;
  assert.equal(splitScenePayload(forged), null, 'the parser must not split on a forged block prefix');
});

test('byte-level persistence wrinkles: trailing newline and a foreign hint on an exact payload', () => {
  const docMeta = createDocumentWritingMessageMeta('写一份通知');
  // A trailing newline (persistence/engine artifact) no longer hides the
  // byte-exact suffix tier: the trimmed copy strips too, record or not —
  // only wrapper-external whitespace is given up, never user content.
  assert.deepEqual(splitScenePayload(`${docMeta.pinvouPayloadText}\n`), { scene: documentScene, text: '写一份通知' });
  assert.deepEqual(splitScenePayload(`${docMeta.pinvouPayloadText}\n`, documentScene), { scene: documentScene, text: '写一份通知' });
  assert.deepEqual(splitScenePayload(`${docMeta.pinvouPayloadText}\r\n`), { scene: documentScene, text: '写一份通知' });
  // A bare wrapper plus artifact newline is still an empty body: the message
  // never vanishes.
  assert.equal(splitScenePayload(`${suffixOf(documentRoute)}\n`), null);
  // Workbench (prefix-shaped) keeps trailing bytes as part of the body.
  const wbMeta = createPersonalWorkbenchMessageMeta('运动');
  assert.deepEqual(splitScenePayload(`${wbMeta.pinvouPayloadText}\n`), { scene: workbenchScene, text: '运动\n' });
  // A byte-exact payload of scene A with scene B on record: still stripped —
  // the recorded scene is a tie-breaker for nested wrappers, never a gate.
  assert.deepEqual(splitScenePayload(docMeta.pinvouPayloadText, 'design:ppt'), { scene: documentScene, text: '写一份通知' });
});

test('stale-tier hardening: hand-typed lookalikes stay whole, quoted anchors keep their text', () => {
  // The header anchor + a same-scene record used to swallow everything after
  // the first anchor occurrence. Two guards close the swallow paths while
  // real stale payloads still strip:
  // - the anchor is taken at its LAST occurrence (the real wrapper sits at
  //   the tail of a persisted payload; a title quoted inside the user's text
  //   sits before it);
  // - the anchor must be followed by the route body's "- " bullet line.
  const documentHeader = documentRoute.context.split('\n')[0];
  // Drift onto a hand-typed message: the only anchor occurrence is the
  // user's own prose, and its continuation is not a bullet — no strip even
  // with this scene on record (this was accepted truncation before).
  const handTyped = `第一段讨论\n\n---\n${documentHeader}\n我手写的后续内容`;
  assert.equal(splitScenePayload(handTyped, documentScene), null);
  assert.equal(splitScenePayload(handTyped), null, 'no record, no strip');
  assert.equal(splitScenePayload(handTyped, 'design:poster'), null, 'another scene on record does not vouch');
  // Residual: a continuation that starts exactly like a route-body bullet
  // still strips under the record. Accepted: the bullet line is the strongest
  // shape signal short of byte equality, and under-stripping stays the
  // fallback for everything weaker.
  const bulletLookalike = `第一段讨论\n\n---\n${documentHeader}\n- 我手写的 bullet 行`;
  assert.deepEqual(splitScenePayload(bulletLookalike, documentScene), { scene: documentScene, text: '第一段讨论' });
  // Quoted anchor inside the user's own text + a real (stale) wrapper at the
  // tail: the last occurrence wins and everything the user typed survives —
  // first-occurrence matching used to drop the quoted wrapper and this tail.
  const quoted = `引用同款标题：\n\n---\n${documentHeader}\n- 我粘贴的路由正文\n后续问题`;
  const staleWrapper = `\n\n---\n${documentHeader}\n- 旧版本的路由正文。\n\n生成完成前执行旧版自检：\n1. 旧条目。`;
  assert.deepEqual(splitScenePayload(quoted + staleWrapper, documentScene), { scene: documentScene, text: quoted });
});

test('the module publishes the parser for the classic-script bridges', () => {
  const moduleUrl = pathToFileURL(path.join(appRoot, 'src', 'features', 'chat', 'scene-payload.js')).href;
  const script = [
    'globalThis.window = {};',
    `await import(${JSON.stringify(moduleUrl)});`,
    'const published = window.__PINVOU_SCENE_PAYLOAD__;',
    'console.log(JSON.stringify(published ? Object.keys(published).sort() : null));',
  ].join('\n');
  const stdout = execFileSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
  assert.equal(JSON.parse(stdout.trim())[0], 'splitScenePayload');
  // ChatView is the static entry of every window that replays a conversation:
  // without this import no publisher is loaded and the boilerplate stays.
  const chatView = fs.readFileSync(path.join(appRoot, 'src', 'features', 'chat', 'ChatView.jsx'), 'utf8');
  assert.match(chatView, /^import '\.\/scene-payload\.js';$/m);
});
